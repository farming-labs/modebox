//! An in-memory [`Store`]: scratch space, tests, and the reference
//! implementation of the contract (every precondition is atomic).

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

#[derive(Default)]
pub struct MemoryStore {
    objects: Mutex<BTreeMap<Key, (Bytes, Meta)>>,
    read_only: bool,
    /// Cap on the total bytes held; `None` is unlimited.
    max_bytes: Option<u64>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Refuse writes that would make the store hold more than `bytes`.
    pub fn with_limit(mut self, bytes: u64) -> Self {
        self.max_bytes = Some(bytes);
        self
    }

    /// Total bytes currently held.
    pub fn size(&self) -> u64 {
        self.objects.lock().unwrap().values().map(|(b, _)| b.len() as u64).sum()
    }

    /// Seed contents without going through preconditions.
    pub fn insert(&self, key: &str, body: impl Into<Bytes>) -> StoreResult<()> {
        let key = Key::new(key)?;
        let body = body.into();
        let meta = Self::meta(&key, &body);
        self.objects.lock().unwrap().insert(key, (body, meta));
        Ok(())
    }

    fn meta(key: &Key, body: &Bytes) -> Meta {
        Meta {
            key: key.clone(),
            size: body.len() as u64,
            modified_ms: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64() * 1000.0)
                    .unwrap_or(0.0),
            ),
            etag: Some(Etag(blake3::hash(body).to_hex()[..20].to_owned())),
            content_type: content_type_for(key.as_str()).map(str::to_owned),
        }
    }
}

impl Store for MemoryStore {
    fn describe(&self) -> String {
        "memory://".into()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            writable: !self.read_only,
            atomic_preconditions: true,
            atomic_create: true,
            range_reads: true,
            server_side_copy: true,
            consistent_list: true,
            content_etags: true,
            max_object_size: None,
        }
    }

    fn head<'a>(&'a self, key: &'a Key) -> StoreFuture<'a, Option<Meta>> {
        let r = self.objects.lock().unwrap().get(key).map(|(_, m)| m.clone());
        Box::pin(async move { Ok(r) })
    }

    fn get<'a>(&'a self, key: &'a Key, opts: GetOptions) -> StoreFuture<'a, Option<Object>> {
        let r = (|| {
            let objects = self.objects.lock().unwrap();
            let Some((body, meta)) = objects.get(key) else { return Ok(None) };
            if let Some(want) = &opts.if_match
                && meta.etag.as_ref() != Some(want)
            {
                return Err(StoreError::PreconditionFailed(key.to_string()));
            }
            let body = match &opts.range {
                Some(r) => {
                    let len = body.len() as u64;
                    let (s, e) = (r.start.min(len), r.end.min(len).max(r.start.min(len)));
                    body.slice(s as usize..e as usize)
                }
                None => body.clone(),
            };
            Ok(Some(Object { meta: meta.clone(), body }))
        })();
        Box::pin(async move { r })
    }

    fn put<'a>(&'a self, key: &'a Key, body: Bytes, opts: PutOptions) -> StoreFuture<'a, Meta> {
        let r = (|| {
            if self.read_only {
                return Err(StoreError::PermissionDenied(format!("{key}: store is read-only")));
            }
            let mut objects = self.objects.lock().unwrap();
            let current = objects.get(key).and_then(|(_, m)| m.etag.clone());
            match &opts.precondition {
                Precondition::IfNotExists if objects.contains_key(key) => {
                    return Err(StoreError::AlreadyExists(key.to_string()));
                }
                Precondition::IfMatch(want) if current.as_ref() != Some(want) => {
                    return Err(StoreError::PreconditionFailed(key.to_string()));
                }
                _ => {}
            }
            if let Some(max) = self.max_bytes {
                let others: u64 = objects.iter().filter(|(k, _)| *k != key).map(|(_, (b, _))| b.len() as u64).sum();
                if others + body.len() as u64 > max {
                    return Err(StoreError::QuotaExceeded(format!(
                        "{key}: memory store would hold {} bytes, over its {max} byte limit",
                        others + body.len() as u64
                    )));
                }
            }
            let mut meta = Self::meta(key, &body);
            if opts.content_type.is_some() {
                meta.content_type = opts.content_type.clone();
            }
            objects.insert(key.clone(), (body, meta.clone()));
            Ok(meta)
        })();
        Box::pin(async move { r })
    }

    fn delete<'a>(&'a self, key: &'a Key, if_match: Option<Etag>) -> StoreFuture<'a, bool> {
        let r = (|| {
            if self.read_only {
                return Err(StoreError::PermissionDenied(format!("{key}: store is read-only")));
            }
            let mut objects = self.objects.lock().unwrap();
            let Some((_, meta)) = objects.get(key) else { return Ok(false) };
            if let Some(want) = if_match
                && meta.etag.as_ref() != Some(&want)
            {
                return Err(StoreError::PreconditionFailed(key.to_string()));
            }
            objects.remove(key);
            Ok(true)
        })();
        Box::pin(async move { r })
    }

    fn list<'a>(&'a self, opts: ListOptions) -> StoreFuture<'a, ListPage> {
        let all: Vec<Meta> = self
            .objects
            .lock()
            .unwrap()
            .range(Key(opts.prefix.clone())..)
            .take_while(|(k, _)| k.as_str().starts_with(&opts.prefix))
            .map(|(_, (_, m))| m.clone())
            .collect();
        let page = paginate(all, &opts);
        Box::pin(async move { Ok(page) })
    }
}
