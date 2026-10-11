//! Which continuation tokens were already resumed.
//!
//! A token resumes once. The engine claims each token's nonce before it
//! resumes and refuses a nonce that was claimed before. [`MemoryContinuations`],
//! the default, only covers one process. Processes that resume each other's
//! tokens (`modebox run` then `modebox resume`, several MCP servers, a fleet
//! behind a load balancer) share a [`StoreContinuations`] over any [`Store`]
//! with atomic create: a local directory, or a bucket.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures::future::BoxFuture;

use crate::store::local::{LocalOptions, LocalStore};
use crate::store::{Key, Precondition, PutOptions, Store, StoreError, list_all};

/// Records used continuation nonces.
pub trait ContinuationStore: Send + Sync + 'static {
    /// Claim `nonce`, remembering it until `expires_ms` (Unix ms), after which
    /// the token is refused as expired anyway. `Ok(true)` if this call claimed
    /// it, `Ok(false)` if it was already used. An error fails the resume, so a
    /// token is never resumed twice because the record couldn't be written.
    fn claim<'a>(&'a self, nonce: &'a str, expires_ms: u64) -> BoxFuture<'a, Result<bool, String>>;

    /// Where the records live, for humans.
    fn describe(&self) -> String;
}

impl fmt::Debug for dyn ContinuationStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// Used nonces in memory: single-use within this process only.
#[derive(Default)]
pub struct MemoryContinuations {
    used: Mutex<HashMap<String, u64>>,
}

impl ContinuationStore for MemoryContinuations {
    fn claim<'a>(&'a self, nonce: &'a str, expires_ms: u64) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let now = now_ms();
            let mut used = self.used.lock().unwrap();
            used.retain(|_, exp| *exp >= now);
            Ok(used.insert(nonce.to_owned(), expires_ms).is_none())
        })
    }

    fn describe(&self) -> String {
        "memory (this process only)".into()
    }
}

/// Used nonces as objects in a [`Store`], created with `IfNotExists` so two
/// processes racing on the same token can't both win. Each object is named
/// `<expires_ms>-<nonce>` under a prefix, so expired records can be swept by
/// listing alone.
pub struct StoreContinuations {
    store: Arc<dyn Store>,
    prefix: String,
    last_sweep_ms: AtomicU64,
}

/// How often a process sweeps expired records, at most.
const SWEEP_EVERY_MS: u64 = 10 * 60 * 1000;

impl StoreContinuations {
    /// Records under `used/` in `store`. The store must enforce `IfNotExists`
    /// atomically (`capabilities().atomic_create`).
    pub fn new(store: Arc<dyn Store>) -> Result<StoreContinuations, String> {
        if !store.capabilities().atomic_create {
            return Err(format!("{} can't create objects atomically, so it can't track used continuations", store.describe()));
        }
        Ok(StoreContinuations { store, prefix: "used/".into(), last_sweep_ms: AtomicU64::new(0) })
    }

    /// Records in a local directory, created if missing.
    pub fn local(dir: impl AsRef<Path>) -> Result<StoreContinuations, String> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let opts = LocalOptions { respect_gitignore: false, skip_hidden: false, ..LocalOptions::default() };
        let store = LocalStore::new(dir, opts).map_err(|e| e.to_string())?;
        StoreContinuations::new(Arc::new(store))
    }

    /// Delete records whose tokens have expired. Returns how many were removed.
    pub async fn sweep(&self) -> Result<usize, String> {
        let now = now_ms();
        let mut removed = 0;
        for meta in list_all(self.store.as_ref(), &self.prefix, 100_000).await.map_err(|e| e.to_string())? {
            let expired = meta
                .key
                .name()
                .split_once('-')
                .and_then(|(exp, _)| exp.parse::<u64>().ok())
                .is_some_and(|exp| exp < now);
            if expired && self.store.delete(&meta.key, None).await.map_err(|e| e.to_string())? {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl ContinuationStore for StoreContinuations {
    fn claim<'a>(&'a self, nonce: &'a str, expires_ms: u64) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            if nonce.is_empty() || !nonce.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return Err(format!("malformed continuation nonce {nonce:?}"));
            }
            let key = Key::new(format!("{}{expires_ms}-{nonce}", self.prefix)).map_err(|e| e.to_string())?;
            let opts = PutOptions { precondition: Precondition::IfNotExists, content_type: None };
            let claimed = match self.store.put(&key, Bytes::new(), opts).await {
                Ok(_) => true,
                Err(StoreError::AlreadyExists(_) | StoreError::PreconditionFailed(_)) => false,
                Err(e) => return Err(format!("recording continuation use in {}: {e}", self.store.describe())),
            };
            let now = now_ms();
            let last = self.last_sweep_ms.load(Ordering::Relaxed);
            if now.saturating_sub(last) >= SWEEP_EVERY_MS
                && self.last_sweep_ms.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
            {
                // Best effort: a failed sweep only leaves stale records behind.
                let _ = self.sweep().await;
            }
            Ok(claimed)
        })
    }

    fn describe(&self) -> String {
        format!("{} ({})", self.store.describe(), self.prefix)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::MemoryStore;

    fn block<T>(f: impl std::future::Future<Output = T>) -> T {
        futures::executor::block_on(f)
    }

    #[test]
    fn memory_claims_once() {
        let s = MemoryContinuations::default();
        let exp = now_ms() + 60_000;
        assert_eq!(block(s.claim("abc", exp)), Ok(true));
        assert_eq!(block(s.claim("abc", exp)), Ok(false));
        assert_eq!(block(s.claim("abd", exp)), Ok(true));
    }

    #[test]
    fn store_claims_once_across_instances() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let a = StoreContinuations::new(store.clone()).unwrap();
        let b = StoreContinuations::new(store).unwrap();
        let exp = now_ms() + 60_000;
        assert_eq!(block(a.claim("n1", exp)), Ok(true));
        assert_eq!(block(b.claim("n1", exp)), Ok(false));
        assert_eq!(block(b.claim("n2", exp)), Ok(true));
    }

    #[test]
    fn store_rejects_malformed_nonces() {
        let s = StoreContinuations::new(Arc::new(MemoryStore::new())).unwrap();
        for bad in ["", "../x", "a/b", "a-b"] {
            assert!(block(s.claim(bad, now_ms() + 1000)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn sweep_removes_only_expired_records() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let s = StoreContinuations::new(store.clone()).unwrap();
        let now = now_ms();
        s.last_sweep_ms.store(now, Ordering::Relaxed); // no automatic sweep during this test
        block(s.claim("old", now - 1)).unwrap();
        block(s.claim("live", now + 60_000)).unwrap();
        assert_eq!(block(s.sweep()), Ok(1));
        let left: Vec<String> =
            block(list_all(store.as_ref(), "used/", 10)).unwrap().into_iter().map(|m| m.key.name().to_owned()).collect();
        assert_eq!(left, vec![format!("{}-live", now + 60_000)]);
    }

    #[tokio::test]
    async fn local_directory_claims_once() {
        let dir = std::env::temp_dir().join(format!("modebox-continuations-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = StoreContinuations::local(&dir).unwrap();
        let b = StoreContinuations::local(&dir).unwrap();
        let exp = now_ms() + 60_000;
        assert_eq!(a.claim("n1", exp).await, Ok(true));
        assert_eq!(b.claim("n1", exp).await, Ok(false));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
