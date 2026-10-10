//! Filesystem capability: any storage, exposed to sandboxed code as one object store.
//!
//! A [`Vfs`] is a namespace of mounts. Each mount maps a key prefix to a
//! [`Store`] (a local folder, memory, S3/R2/GCS/Azure, or anything else that
//! implements the trait) and a [`MountMode`]:
//!
//! - `ReadOnly`: reads only.
//! - `Staged`: writes land in a copy-on-write overlay of content-addressed
//!   blobs. `fs.commit()` applies them behind an approval, using conditional
//!   writes, so anything that changed since the code read it is reported as a
//!   conflict instead of being overwritten.
//! - `Direct`: writes go straight to the store (scratch space, output buckets).
//!
//! Code sees one API whatever the mounts are: `fs.list/get/head/put/delete/
//! copy/move/grep/changes/commit/discard/mounts`.

#![allow(clippy::result_large_err)] // ToolError carries messages; boxing it buys nothing here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use futures::future::BoxFuture;
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

use crate::store::{self, Bytes, Etag, GetOptions, Key, ListOptions, PutOptions, Precondition, Store, StoreError};
use crate::tools::{Approval, Call, Tool, ToolError};
use crate::value::Value;

type Hash = [u8; 32];
/// key → (size, modified ms, etag, staged status)
type Listing = BTreeMap<String, (u64, Option<f64>, Option<String>, Option<&'static str>)>;
type Op = fn(Vfs, Call) -> BoxFuture<'static, Result<Value, ToolError>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountMode {
    ReadOnly,
    Staged,
    Direct,
}

impl MountMode {
    fn as_str(self) -> &'static str {
        match self {
            MountMode::ReadOnly => "read-only",
            MountMode::Staged => "staged",
            MountMode::Direct => "direct",
        }
    }
}

#[derive(Clone)]
pub struct Mount {
    /// Key prefix this mount serves: `""` for the root, otherwise like `"docs/"`.
    pub prefix: String,
    pub store: Arc<dyn Store>,
    pub mode: MountMode,
}

impl Mount {
    pub fn new(prefix: impl Into<String>, store: Arc<dyn Store>, mode: MountMode) -> Mount {
        let mut prefix = prefix.into();
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        let mode = if store.capabilities().writable { mode } else { MountMode::ReadOnly };
        Mount { prefix, store, mode }
    }

    /// A local folder, listed .gitignore- and dotfile-aware.
    pub fn dir(prefix: impl Into<String>, path: impl AsRef<Path>, mode: MountMode) -> Result<Mount, String> {
        let opts = store::local::LocalOptions { writable: mode != MountMode::ReadOnly, ..Default::default() };
        let s = store::local::LocalStore::new(path, opts).map_err(|e| e.to_string())?;
        Ok(Mount::new(prefix, Arc::new(s), mode))
    }

    /// In-memory scratch space (64 MiB); writes are direct and vanish with the process.
    pub fn memory(prefix: impl Into<String>) -> Mount {
        let store = store::memory::MemoryStore::new().with_limit(64 << 20);
        Mount::new(prefix, Arc::new(store), MountMode::Direct)
    }

    /// Any `object_store` URL: `s3://bucket/prefix`, `gs://…`, `az://…`, `https://…`, `file://…`, `memory://`.
    #[cfg(feature = "object-store")]
    pub fn url(
        prefix: impl Into<String>,
        url: &str,
        options: impl IntoIterator<Item = (String, String)>,
        mode: MountMode,
    ) -> Result<Mount, String> {
        let s = store::objstore::ObjectStoreAdapter::from_url(url, options, mode == MountMode::ReadOnly)
            .map_err(|e| e.to_string())?;
        Ok(Mount::new(prefix, Arc::new(s), mode))
    }
}

#[derive(Clone)]
pub struct VfsConfig {
    pub mounts: Vec<Mount>,
    /// Globs (matched against full keys, ignoring case) that can't be read,
    /// listed or written.
    pub deny: Vec<String>,
    /// Largest object `get` returns whole; use a byte range beyond this.
    pub max_read_bytes: usize,
    /// Most objects one `list` returns.
    pub max_list: usize,
    /// Require approval for `commit` (recommended).
    pub commit_approval: bool,
    /// Most bytes of uncommitted changes held at once on staged mounts.
    pub max_staged_bytes: usize,
}

pub fn default_deny() -> Vec<String> {
    ["**/.git/**", "**/.env", "**/.env.*", "**/*.pem", "**/*.key", "**/id_rsa*", "**/id_ed25519*", "**/.npmrc", "**/.netrc"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

impl VfsConfig {
    pub fn new(mounts: Vec<Mount>) -> Self {
        VfsConfig {
            mounts,
            deny: default_deny(),
            max_read_bytes: 8 << 20,
            max_list: 5000,
            commit_approval: true,
            max_staged_bytes: 64 << 20,
        }
    }

    /// A staged workspace at `root` plus in-memory `tmp/` scratch space.
    pub fn workspace(root: impl AsRef<Path>) -> Result<Self, String> {
        Ok(VfsConfig::new(vec![Mount::dir("", root, MountMode::Staged)?, Mount::memory("tmp/")]))
    }
}

#[derive(Clone)]
enum Layer {
    Put { hash: Hash, modified_ms: f64 },
    Deleted,
}

/// What the store held the first time code touched a staged key.
#[derive(Clone, Debug)]
struct Seen {
    existed: bool,
    etag: Option<Etag>,
    /// Content hash, for stores without etags.
    hash: Option<Hash>,
}

#[derive(Default)]
struct State {
    overlay: BTreeMap<String, Layer>,
    blobs: HashMap<Hash, Bytes>,
    seen: HashMap<String, Seen>,
}

struct Inner {
    cfg: VfsConfig,
    deny: GlobSet,
    state: Mutex<State>,
}

/// The filesystem capability. Cheap to clone; clones share staged state.
#[derive(Clone)]
pub struct Vfs {
    inner: Arc<Inner>,
}

/// One pending change, as shown by `fs.changes()`.
#[derive(Clone, Debug)]
pub struct Change {
    pub key: String,
    /// `added`, `modified` or `deleted`.
    pub status: &'static str,
    pub diff: String,
    pub added_lines: usize,
    pub removed_lines: usize,
}

/// Sum of staged object sizes, optionally leaving one key out (it's being replaced).
fn staged_bytes(st: &State, except: Option<&str>) -> usize {
    st.overlay
        .iter()
        .filter(|(k, _)| Some(k.as_str()) != except)
        .map(|(_, l)| match l {
            Layer::Put { hash, .. } => st.blobs.get(hash).map(|b| b.len()).unwrap_or(0),
            Layer::Deleted => 0,
        })
        .sum()
}

/// Forget blob versions nothing in the overlay points at any more.
fn drop_unused_blobs(st: &mut State) {
    let used: std::collections::HashSet<Hash> = st
        .overlay
        .values()
        .filter_map(|l| if let Layer::Put { hash, .. } = l { Some(*hash) } else { None })
        .collect();
    st.blobs.retain(|h, _| used.contains(h));
}

fn err(msg: impl Into<String>, code: &str) -> ToolError {
    ToolError::new(msg).with_code(code)
}

fn serr(e: StoreError) -> ToolError {
    ToolError::new(e.to_string()).with_code(e.code())
}

fn hash(data: &[u8]) -> Hash {
    *blake3::hash(data).as_bytes()
}

fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn staged_etag(h: &Hash) -> String {
    format!("staged-{}", &hex(h)[..20])
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// `"./src//a.ts"` → `"src/a.ts"`. Rejects `..`. Keeps a trailing `/`.
fn normalize(key: &str) -> Result<String, ToolError> {
    let trailing = key.ends_with('/');
    let mut parts = Vec::new();
    for seg in key.trim().split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(err(format!("{key}: `..` is not allowed in keys"), "EACCES")),
            s => parts.push(s),
        }
    }
    let mut k = parts.join("/");
    if trailing && !k.is_empty() {
        k.push('/');
    }
    Ok(k)
}

impl Vfs {
    pub fn new(cfg: VfsConfig) -> Result<Vfs, String> {
        let mut gs = GlobSetBuilder::new();
        for d in &cfg.deny {
            // Case-insensitive: on macOS and Windows `.ENV` opens the same file as `.env`.
            let glob = GlobBuilder::new(d).case_insensitive(true).build();
            gs.add(glob.map_err(|e| format!("bad deny glob {d}: {e}"))?);
        }
        let deny = gs.build().map_err(|e| e.to_string())?;
        Ok(Vfs { inner: Arc::new(Inner { cfg, deny, state: Mutex::new(State::default()) }) })
    }

    fn cfg(&self) -> &VfsConfig {
        &self.inner.cfg
    }

    fn mount(&self, i: usize) -> &Mount {
        &self.inner.cfg.mounts[i]
    }

    /// The mount serving `key` (longest prefix) and the store key within it.
    fn route(&self, key: &str) -> Result<(usize, Key), ToolError> {
        let (i, m) = self
            .cfg()
            .mounts
            .iter()
            .enumerate()
            .filter(|(_, m)| key.starts_with(&m.prefix))
            .max_by_key(|(_, m)| m.prefix.len())
            .ok_or_else(|| err(format!("{key}: no mount serves this key"), "ENOENT"))?;
        let k = Key::new(&key[m.prefix.len()..]).map_err(serr)?;
        Ok((i, k))
    }

    fn check(&self, key: &str) -> Result<(), ToolError> {
        if key.is_empty() || key.ends_with('/') {
            return Err(err(format!("{key:?} is not an object key"), "EINVAL"));
        }
        if self.inner.deny.is_match(key) {
            return Err(err(format!("{key} is not accessible (denied by policy)"), "EACCES"));
        }
        Ok(())
    }

    /// Current content of `key` (overlay first, then the store), optionally a range.
    /// Returns (bytes, modified, etag, total size).
    async fn read(&self, key: &str, range: Option<Range<u64>>) -> Result<Option<(Bytes, Option<f64>, String, u64)>, ToolError> {
        self.check(key)?;
        {
            let st = self.inner.state.lock().unwrap();
            match st.overlay.get(key) {
                Some(Layer::Put { hash, modified_ms }) => {
                    let data = st.blobs[hash].clone();
                    let len = data.len() as u64;
                    let body = match &range {
                        Some(r) => {
                            let (s, e) = (r.start.min(len), r.end.min(len).max(r.start.min(len)));
                            data.slice(s as usize..e as usize)
                        }
                        None => data,
                    };
                    return Ok(Some((body, Some(*modified_ms), staged_etag(hash), len)));
                }
                Some(Layer::Deleted) => return Ok(None),
                None => {}
            }
        }
        let (i, k) = self.route(key)?;
        let m = self.mount(i);
        let max = self.cfg().max_read_bytes as u64;
        let want = range.clone().unwrap_or(0..max + 1);
        let opts = GetOptions { range: Some(want), if_match: None };
        let Some(obj) = m.store.get(&k, opts).await.map_err(serr)? else {
            if m.mode == MountMode::Staged {
                self.remember(key, Seen { existed: false, etag: None, hash: None });
            }
            return Ok(None);
        };
        if range.is_none() && obj.meta.size > max {
            return Err(err(
                format!("{key} is {} bytes; read at most {max} at a time with `range`", obj.meta.size),
                "EFBIG",
            ));
        }
        if m.mode == MountMode::Staged {
            let full = range.is_none();
            self.remember(
                key,
                Seen { existed: true, etag: obj.meta.etag.clone(), hash: full.then(|| hash(&obj.body)) },
            );
        }
        let etag = obj.meta.etag.map(|e| e.0).unwrap_or_default();
        Ok(Some((obj.body, obj.meta.modified_ms, etag, obj.meta.size)))
    }

    fn remember(&self, key: &str, seen: Seen) {
        self.inner.state.lock().unwrap().seen.entry(key.to_owned()).or_insert(seen);
    }

    /// Write (`Some`) or delete (`None`) according to the mount's mode.
    async fn write(&self, key: &str, data: Option<Bytes>, if_match: Option<String>) -> Result<Option<String>, ToolError> {
        self.check(key)?;
        let (i, k) = self.route(key)?;
        let m = self.mount(i);
        match m.mode {
            MountMode::ReadOnly => Err(err(format!("{key} is on a read-only mount ({})", m.store.describe()), "EROFS")),
            MountMode::Direct => {
                let precondition = match if_match {
                    Some(e) => Precondition::IfMatch(Etag(e)),
                    None => Precondition::None,
                };
                match data {
                    Some(d) => {
                        let meta = m.store.put(&k, d, PutOptions { precondition, content_type: None }).await.map_err(serr)?;
                        Ok(meta.etag.map(|e| e.0))
                    }
                    None => {
                        let cond = if let Precondition::IfMatch(e) = precondition { Some(e) } else { None };
                        m.store.delete(&k, cond).await.map_err(serr)?;
                        Ok(None)
                    }
                }
            }
            MountMode::Staged => {
                if let Some(expected) = if_match {
                    let current = self.read(key, Some(0..0)).await?.map(|(_, _, e, _)| e);
                    if current.as_deref() != Some(expected.as_str()) {
                        return Err(err(format!("{key}: etag mismatch (current {current:?})"), "PRECONDITION_FAILED"));
                    }
                }
                let known = self.inner.state.lock().unwrap().seen.contains_key(key);
                if !known {
                    let meta = m.store.head(&k).await.map_err(serr)?;
                    self.remember(
                        key,
                        Seen { existed: meta.is_some(), etag: meta.and_then(|m| m.etag), hash: None },
                    );
                }
                let mut st = self.inner.state.lock().unwrap();
                let result = match data {
                    Some(bytes) => {
                        let max = self.cfg().max_staged_bytes;
                        let others = staged_bytes(&st, Some(key));
                        if others + bytes.len() > max {
                            return Err(serr(StoreError::QuotaExceeded(format!(
                                "staging {key} would hold {} bytes of uncommitted changes, over the {max} byte limit; \
                                 fs.commit() or fs.discard() first",
                                others + bytes.len()
                            ))));
                        }
                        let h = hash(&bytes);
                        st.blobs.entry(h).or_insert(bytes);
                        st.overlay.insert(key.to_owned(), Layer::Put { hash: h, modified_ms: now_ms() });
                        Some(staged_etag(&h))
                    }
                    None => {
                        st.overlay.insert(key.to_owned(), Layer::Deleted);
                        None
                    }
                };
                drop_unused_blobs(&mut st);
                Ok(result)
            }
        }
    }

    /// The merged view under `prefix`: store objects plus staged changes.
    async fn walk(&self, prefix: &str, recursive: bool) -> Result<(Listing, BTreeSet<String>), ToolError> {
        let mut objects = Listing::new();
        let mut prefixes = BTreeSet::new();
        let cap = self.cfg().max_list * 4;
        for (i, m) in self.cfg().mounts.iter().enumerate() {
            if !(m.prefix.starts_with(prefix) || prefix.starts_with(&m.prefix)) {
                continue;
            }
            if !recursive && m.prefix.len() > prefix.len() {
                // A mount below this level shows up as a folder.
                let first = m.prefix[prefix.len()..].split('/').next().unwrap_or("");
                prefixes.insert(format!("{prefix}{first}/"));
                continue;
            }
            let rel = if prefix.len() >= m.prefix.len() { &prefix[m.prefix.len()..] } else { "" };
            let mut cursor = None;
            loop {
                let page = m
                    .store
                    .list(ListOptions { prefix: rel.to_owned(), delimiter: !recursive, cursor, limit: Some(1000) })
                    .await
                    .map_err(serr)?;
                for meta in page.objects {
                    let key = format!("{}{}", m.prefix, meta.key);
                    if !key.starts_with(prefix) || self.inner.deny.is_match(&key) {
                        continue;
                    }
                    // A more specific mount (e.g. `tmp/`) shadows this one.
                    if self.route(&key).map(|(j, _)| j).ok() != Some(i) {
                        continue;
                    }
                    objects.insert(key, (meta.size, meta.modified_ms, meta.etag.map(|e| e.0), None));
                }
                for p in page.prefixes {
                    let full = format!("{}{p}", m.prefix);
                    if !self.inner.deny.is_match(full.trim_end_matches('/')) {
                        prefixes.insert(full);
                    }
                }
                match page.cursor {
                    Some(c) if objects.len() < cap => cursor = Some(c),
                    _ => break,
                }
            }
        }
        let st = self.inner.state.lock().unwrap();
        for (key, layer) in st.overlay.range(prefix.to_owned()..) {
            if !key.starts_with(prefix) {
                break;
            }
            let rest = &key[prefix.len()..];
            if !recursive && let Some(p) = rest.find('/') {
                if matches!(layer, Layer::Put { .. }) {
                    prefixes.insert(format!("{prefix}{}", &rest[..=p]));
                }
                continue;
            }
            match layer {
                Layer::Put { hash, modified_ms } => {
                    let existed = st.seen.get(key).is_some_and(|s| s.existed);
                    let status = if existed { "modified" } else { "added" };
                    objects.insert(
                        key.clone(),
                        (st.blobs[hash].len() as u64, Some(*modified_ms), Some(staged_etag(hash)), Some(status)),
                    );
                }
                Layer::Deleted => {
                    objects.remove(key);
                }
            }
        }
        Ok((objects, prefixes))
    }

    /// Pending changes with unified diffs (no-op writes are dropped).
    pub async fn changes(&self) -> Vec<Change> {
        let overlay: Vec<(String, Layer)> =
            self.inner.state.lock().unwrap().overlay.iter().map(|(k, l)| (k.clone(), l.clone())).collect();
        let mut out = Vec::new();
        for (key, layer) in overlay {
            let Ok((i, k)) = self.route(&key) else { continue };
            let current = self.mount(i).store.get(&k, GetOptions::default()).await.ok().flatten().map(|o| o.body);
            let new = match &layer {
                Layer::Put { hash, .. } => Some(self.inner.state.lock().unwrap().blobs[hash].clone()),
                Layer::Deleted => None,
            };
            let status = match (&current, &new) {
                (None, Some(_)) => "added",
                (Some(old), Some(new)) if old == new => continue,
                (Some(_), Some(_)) => "modified",
                (Some(_), None) => "deleted",
                (None, None) => continue,
            };
            let old_text = current.as_deref().map(String::from_utf8_lossy).unwrap_or_default();
            let new_text = new.as_deref().map(String::from_utf8_lossy).unwrap_or_default();
            let diff = similar::TextDiff::from_lines(old_text.as_ref(), new_text.as_ref());
            let (mut added, mut removed) = (0, 0);
            for c in diff.iter_all_changes() {
                match c.tag() {
                    similar::ChangeTag::Insert => added += 1,
                    similar::ChangeTag::Delete => removed += 1,
                    similar::ChangeTag::Equal => {}
                }
            }
            let mut text =
                diff.unified_diff().context_radius(2).header(&format!("a/{key}"), &format!("b/{key}")).to_string();
            if text.len() > 6000 {
                let mut cut = 6000;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
                text.push_str("\n… (diff truncated)\n");
            }
            out.push(Change { key, status, diff: text, added_lines: added, removed_lines: removed });
        }
        out
    }

    /// Staged keys with A/M/D markers, from local state only (used in approval prompts).
    pub fn summary(&self) -> String {
        let st = self.inner.state.lock().unwrap();
        st.overlay
            .iter()
            .filter_map(|(k, l)| {
                let existed = st.seen.get(k).is_some_and(|s| s.existed);
                match (l, existed) {
                    (Layer::Put { .. }, true) => Some(format!("M {k}")),
                    (Layer::Put { .. }, false) => Some(format!("A {k}")),
                    (Layer::Deleted, true) => Some(format!("D {k}")),
                    (Layer::Deleted, false) => None,
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Apply staged changes with conditional writes. Objects that changed in
    /// the store since the code first saw them are skipped as conflicts.
    pub async fn commit(&self) -> Value {
        let mut applied = Vec::new();
        let mut conflicts = Vec::new();
        for c in self.changes().await {
            let key = c.key.clone();
            let outcome: Result<(), String> = async {
                let (i, k) = self.route(&key).map_err(|e| e.message)?;
                let store = &self.mount(i).store;
                let (seen, layer) = {
                    let st = self.inner.state.lock().unwrap();
                    (st.seen.get(&key).cloned(), st.overlay.get(&key).cloned())
                };
                let precondition = match &seen {
                    Some(Seen { existed: false, .. }) => Precondition::IfNotExists,
                    Some(Seen { etag: Some(e), .. }) => Precondition::IfMatch(e.clone()),
                    Some(Seen { existed: true, etag: None, hash }) => {
                        // No etags from this store: compare content (not atomic).
                        let cur = store.get(&k, GetOptions::default()).await.map_err(|e| e.to_string())?;
                        if cur.map(|o| self::hash(&o.body)) != *hash {
                            return Err("changed since it was read".into());
                        }
                        Precondition::None
                    }
                    None => Precondition::None,
                };
                let r = match layer {
                    Some(Layer::Put { hash, .. }) => {
                        let body = self.inner.state.lock().unwrap().blobs[&hash].clone();
                        store.put(&k, body, PutOptions { precondition, content_type: None }).await.map(|_| ())
                    }
                    Some(Layer::Deleted) => {
                        let if_match = if let Precondition::IfMatch(e) = precondition { Some(e) } else { None };
                        store.delete(&k, if_match).await.map(|_| ())
                    }
                    None => Ok(()),
                };
                r.map_err(|e| match e {
                    StoreError::PreconditionFailed(_) | StoreError::AlreadyExists(_) => "changed since it was read".to_owned(),
                    other => other.to_string(),
                })
            }
            .await;
            match outcome {
                Ok(()) => {
                    let mut st = self.inner.state.lock().unwrap();
                    st.overlay.remove(&key);
                    st.seen.remove(&key);
                    applied.push(Value::object([("key", Value::from(key)), ("status", Value::from(c.status))]));
                }
                Err(reason) => {
                    conflicts.push(Value::object([("key", Value::from(key)), ("reason", Value::from(reason))]))
                }
            }
        }
        // Drop no-op staged entries that `changes` filtered out.
        {
            let mut st = self.inner.state.lock().unwrap();
            let pending: BTreeSet<String> =
                conflicts.iter().filter_map(|c| c.get("key").and_then(Value::as_str).map(str::to_owned)).collect();
            let stale: Vec<String> = st.overlay.keys().filter(|k| !pending.contains(*k)).cloned().collect();
            for k in stale {
                st.overlay.remove(&k);
                st.seen.remove(&k);
            }
            drop_unused_blobs(&mut st);
        }
        Value::object([("applied", Value::Array(applied)), ("conflicts", Value::Array(conflicts))])
    }

    /// Drop staged changes (all, or under `prefix`). Returns how many.
    pub fn discard(&self, prefix: &str) -> usize {
        let mut st = self.inner.state.lock().unwrap();
        let keys: Vec<String> = st.overlay.keys().filter(|k| k.starts_with(prefix)).cloned().collect();
        for k in &keys {
            st.overlay.remove(k);
            st.seen.remove(k);
        }
        drop_unused_blobs(&mut st);
        keys.len()
    }

    /// Bytes of uncommitted changes staged right now.
    pub fn staged_bytes(&self) -> usize {
        staged_bytes(&self.inner.state.lock().unwrap(), None)
    }

    /// Bytes of staged content held in memory (each distinct version once).
    pub fn held_bytes(&self) -> usize {
        self.inner.state.lock().unwrap().blobs.values().map(|b| b.len()).sum()
    }

    fn entry(key: &str, size: u64, modified: Option<f64>, etag: Option<String>, status: Option<&str>) -> Value {
        let mut e = vec![("key", Value::from(key)), ("size", Value::from(size))];
        if let Some(m) = modified {
            e.push(("modified", Value::Date(Some(m))));
        }
        if let Some(t) = etag.filter(|t| !t.is_empty()) {
            e.push(("etag", Value::from(t)));
        }
        if let Some(s) = status {
            e.push(("status", Value::from(s)));
        }
        Value::object(e)
    }

    /// Add the `fs.*` tools to a toolbox and register the staged state so it
    /// travels with suspended runs and saved sessions.
    pub fn install(&self, toolbox: &mut crate::tools::Toolbox, ns: &str) {
        for t in self.tools(ns) {
            toolbox.add(t);
        }
        toolbox.extend(Arc::new(self.clone()));
    }

    /// The `fs.*` host functions under namespace `ns`.
    pub fn tools(&self, ns: &str) -> Vec<Tool> {
        let mut tools = Vec::new();
        let mut add = |name: &str, desc: &str, input: serde_json::Value, output: serde_json::Value, f: Op| {
            let v = self.clone();
            tools.push(
                Tool::new(format!("{ns}.{name}"), move |call: Call| f(v.clone(), call))
                    .description(desc)
                    .input_schema(input)
                    .output_schema(output),
            );
        };
        let entry = serde_json::json!({ "type": "object", "properties": {
            "key": { "type": "string" }, "size": { "type": "number" }, "modified": { "tsType": "Date" },
            "etag": { "type": "string" }, "status": { "enum": ["added", "modified"] } }, "required": ["key", "size"] });
        let key_or = |extra: serde_json::Value| {
            serde_json::json!({ "anyOf": [ { "type": "string", "description": "object key (path)" }, extra ] })
        };
        let from_to = serde_json::json!({ "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" } }, "required": ["from", "to"] });

        add(
            "list",
            "List objects under a prefix, like S3 ListObjects (local folders also respect .gitignore). With recursive: false, sub-folders come back in `prefixes`.",
            serde_json::json!({ "type": "object", "properties": {
                "prefix": { "type": "string", "description": "e.g. \"src/\"" },
                "glob": { "type": "string", "description": "e.g. \"**/*.ts\"" },
                "recursive": { "type": "boolean", "description": "default true" },
                "limit": { "type": "number" } } }),
            serde_json::json!({ "type": "object", "properties": {
                "objects": { "type": "array", "items": entry.clone() },
                "prefixes": { "type": "array", "items": { "type": "string" } },
                "truncated": { "type": "boolean" } }, "required": ["objects", "prefixes", "truncated"] }),
            |v, c| Box::pin(async move { v.op_list(&c).await }),
        );
        add(
            "get",
            "Read an object: text for UTF-8, Uint8Array for binary, `as: \"json\"` to parse. Null if absent.",
            key_or(serde_json::json!({ "type": "object", "properties": {
                "key": { "type": "string" }, "as": { "enum": ["text", "bytes", "json"] },
                "range": { "type": "object", "properties": { "start": { "type": "number" }, "end": { "type": "number" } } } },
                "required": ["key"] })),
            serde_json::json!({ "tsType": "string | Uint8Array | any | null" }),
            |v, c| Box::pin(async move { v.op_get(&c).await }),
        );
        add(
            "head",
            "Metadata (size, modified, etag), or null if absent.",
            key_or(serde_json::json!({ "type": "object", "properties": { "key": { "type": "string" } }, "required": ["key"] })),
            serde_json::json!({ "anyOf": [entry.clone(), { "type": "null" }] }),
            |v, c| Box::pin(async move { v.op_head(&c).await }),
        );
        add(
            "put",
            "Write an object. On staged mounts nothing changes until fs.commit. Body: string, Uint8Array, or any value (stored as JSON). ifMatch: only if the current etag matches.",
            serde_json::json!({ "type": "object", "properties": {
                "key": { "type": "string" }, "body": {}, "ifMatch": { "type": "string" } }, "required": ["key", "body"] }),
            serde_json::json!({ "type": "object", "properties": { "key": { "type": "string" }, "size": { "type": "number" }, "etag": { "type": "string" } }, "required": ["key", "size"] }),
            |v, c| Box::pin(async move { v.op_put(&c).await }),
        );
        add(
            "delete",
            "Delete an object. Returns whether it existed.",
            key_or(serde_json::json!({ "type": "object", "properties": { "key": { "type": "string" } }, "required": ["key"] })),
            serde_json::json!({ "type": "boolean" }),
            |v, c| Box::pin(async move { v.op_delete(&c).await }),
        );
        add(
            "copy",
            "Copy an object to a new key (works across mounts).",
            from_to.clone(),
            serde_json::json!({ "type": "boolean" }),
            |v, c| Box::pin(async move { v.op_copy(&c, false).await }),
        );
        add(
            "move",
            "Move/rename an object (works across mounts).",
            from_to,
            serde_json::json!({ "type": "boolean" }),
            |v, c| Box::pin(async move { v.op_copy(&c, true).await }),
        );
        add(
            "grep",
            "Search object contents with a regex (or literal). Sees staged versions.",
            serde_json::json!({ "type": "object", "properties": {
                "pattern": { "type": "string" }, "prefix": { "type": "string" }, "glob": { "type": "string" },
                "literal": { "type": "boolean" }, "ignoreCase": { "type": "boolean" }, "maxResults": { "type": "number" } },
                "required": ["pattern"] }),
            serde_json::json!({ "type": "array", "items": { "type": "object", "properties": {
                "key": { "type": "string" }, "line": { "type": "number" }, "text": { "type": "string" } }, "required": ["key", "line", "text"] } }),
            |v, c| Box::pin(async move { v.op_grep(&c).await }),
        );
        add(
            "changes",
            "Staged changes not yet written, with unified diffs.",
            serde_json::json!({ "type": "object", "properties": {} }),
            serde_json::json!({ "type": "array", "items": { "type": "object", "properties": {
                "key": { "type": "string" }, "status": { "enum": ["added", "modified", "deleted"] }, "diff": { "type": "string" } },
                "required": ["key", "status", "diff"] } }),
            |v, _c| {
                Box::pin(async move {
                    Ok(Value::Array(
                        v.changes()
                            .await
                            .into_iter()
                            .map(|c| {
                                Value::object([
                                    ("key", Value::from(c.key)),
                                    ("status", Value::from(c.status)),
                                    ("diff", Value::from(c.diff)),
                                ])
                            })
                            .collect(),
                    ))
                })
            },
        );
        add(
            "discard",
            "Throw away staged changes (all, or under a prefix). Returns how many.",
            serde_json::json!({ "type": "object", "properties": { "prefix": { "type": "string" } } }),
            serde_json::json!({ "type": "number" }),
            |v, c| {
                Box::pin(async move {
                    let p = normalize(c.input().get("prefix").and_then(Value::as_str).unwrap_or(""))?;
                    Ok(Value::from(v.discard(&p)))
                })
            },
        );
        add(
            "commit",
            "Write staged changes to their stores. Asks the user for approval with a summary. Objects changed since you read them are skipped as conflicts.",
            serde_json::json!({ "type": "object", "properties": { "message": { "type": "string" } } }),
            serde_json::json!({ "type": "object", "properties": {
                "applied": { "type": "array", "items": { "type": "object", "properties": { "key": { "type": "string" }, "status": { "type": "string" } } } },
                "conflicts": { "type": "array", "items": { "type": "object", "properties": { "key": { "type": "string" }, "reason": { "type": "string" } } } } },
                "required": ["applied", "conflicts"] }),
            |v, _c| Box::pin(async move { Ok(v.commit().await) }),
        );
        add(
            "mounts",
            "Where data lives: each mount's key prefix, backing store, mode (read-only / staged / direct) and capabilities.",
            serde_json::json!({ "type": "object", "properties": {} }),
            serde_json::json!({ "type": "array", "items": { "type": "object", "properties": {
                "prefix": { "type": "string" }, "store": { "type": "string" }, "mode": { "enum": ["read-only", "staged", "direct"] } },
                "required": ["prefix", "store", "mode"] } }),
            |v, _c| Box::pin(async move { Ok(v.op_mounts()) }),
        );
        let _ = add;

        if self.cfg().commit_approval {
            let v = self.clone();
            let commit = tools.iter_mut().find(|t| t.name.ends_with(".commit")).unwrap();
            *commit = commit.clone().approval(Approval::When(Arc::new(move |call: &Call| {
                let summary = v.summary();
                if summary.is_empty() {
                    return None;
                }
                let msg = call.input().get("message").and_then(Value::as_str).map(|m| format!(" — {m}")).unwrap_or_default();
                Some(format!("apply changes{msg}: {summary}"))
            })));
        }
        tools
    }

    // ---- operations -------------------------------------------------------------

    fn key_arg(c: &Call) -> Result<String, ToolError> {
        let k = match c.input() {
            Value::String(s) => s.clone(),
            v => v.get("key").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| err("expected a key", "EINVAL"))?,
        };
        normalize(&k)
    }

    fn op_mounts(&self) -> Value {
        Value::Array(
            self.cfg()
                .mounts
                .iter()
                .map(|m| {
                    let caps = m.store.capabilities();
                    Value::object([
                        ("prefix", Value::from(m.prefix.clone())),
                        ("store", Value::from(m.store.describe())),
                        ("mode", Value::from(m.mode.as_str())),
                        (
                            "capabilities",
                            Value::object([
                                ("atomicPreconditions", Value::from(caps.atomic_preconditions)),
                                ("rangeReads", Value::from(caps.range_reads)),
                                ("serverSideCopy", Value::from(caps.server_side_copy)),
                            ]),
                        ),
                    ])
                })
                .collect(),
        )
    }

    async fn op_list(&self, c: &Call) -> Result<Value, ToolError> {
        let input = c.input();
        let prefix = normalize(input.get("prefix").and_then(Value::as_str).unwrap_or(""))?;
        let recursive = input.get("recursive").and_then(Value::as_bool).unwrap_or(true);
        let limit = input
            .get("limit")
            .and_then(Value::as_f64)
            .map(|n| n as usize)
            .unwrap_or(self.cfg().max_list)
            .min(self.cfg().max_list);
        let glob = match input.get("glob").and_then(Value::as_str) {
            Some(g) => Some(Glob::new(g).map_err(|e| err(format!("bad glob: {e}"), "EINVAL"))?.compile_matcher()),
            None => None,
        };
        let (objects, prefixes) = self.walk(&prefix, recursive).await?;
        let mut out = Vec::new();
        let mut truncated = false;
        for (key, (size, modified, etag, status)) in objects {
            if glob.as_ref().is_some_and(|g| !g.is_match(&key)) {
                continue;
            }
            if out.len() >= limit {
                truncated = true;
                break;
            }
            out.push(Self::entry(&key, size, modified, etag, status));
        }
        Ok(Value::object([
            ("objects", Value::Array(out)),
            ("prefixes", Value::Array(prefixes.into_iter().map(Value::from).collect())),
            ("truncated", Value::from(truncated)),
        ]))
    }

    async fn op_get(&self, c: &Call) -> Result<Value, ToolError> {
        let key = Self::key_arg(c)?;
        let input = c.input();
        let range = input.get("range").map(|r| {
            let s = r.get("start").and_then(Value::as_f64).unwrap_or(0.0) as u64;
            let e = r.get("end").and_then(Value::as_f64).map(|e| e as u64).unwrap_or(u64::MAX);
            s..e
        });
        if let Some(r) = &range
            && r.end.saturating_sub(r.start) > self.cfg().max_read_bytes as u64
            && r.end != u64::MAX
        {
            return Err(err(format!("ranges are limited to {} bytes", self.cfg().max_read_bytes), "EFBIG"));
        }
        let Some((data, _, _, _)) = self.read(&key, range).await? else { return Ok(Value::Null) };
        match input.get("as").and_then(Value::as_str) {
            Some("bytes") => Ok(Value::Bytes { kind: "Uint8Array".into(), data: data.to_vec() }),
            Some("json") => serde_json::from_slice::<serde_json::Value>(&data)
                .map(Value::from)
                .map_err(|e| err(format!("{key} is not valid JSON: {e}"), "EINVAL").with_name("SyntaxError")),
            _ => match std::str::from_utf8(&data) {
                Ok(s) => Ok(Value::from(s)),
                Err(_) => Ok(Value::Bytes { kind: "Uint8Array".into(), data: data.to_vec() }),
            },
        }
    }

    async fn op_head(&self, c: &Call) -> Result<Value, ToolError> {
        let key = Self::key_arg(c)?;
        self.check(&key)?;
        let staged = {
            let st = self.inner.state.lock().unwrap();
            st.overlay.get(&key).cloned().map(|l| (l, st.seen.get(&key).is_some_and(|s| s.existed)))
        };
        match staged {
            Some((Layer::Deleted, _)) => Ok(Value::Null),
            Some((Layer::Put { hash, modified_ms }, existed)) => {
                let size = self.inner.state.lock().unwrap().blobs[&hash].len() as u64;
                let status = if existed { "modified" } else { "added" };
                Ok(Self::entry(&key, size, Some(modified_ms), Some(staged_etag(&hash)), Some(status)))
            }
            None => {
                let (i, k) = self.route(&key)?;
                Ok(match self.mount(i).store.head(&k).await.map_err(serr)? {
                    Some(m) => Self::entry(&key, m.size, m.modified_ms, m.etag.map(|e| e.0), None),
                    None => Value::Null,
                })
            }
        }
    }

    async fn op_put(&self, c: &Call) -> Result<Value, ToolError> {
        let input = c.input();
        let key = normalize(input.get("key").and_then(Value::as_str).ok_or_else(|| err("fs.put needs { key, body }", "EINVAL"))?)?;
        let data = match input.get("body").cloned().unwrap_or(Value::Undefined) {
            Value::String(s) => s.into_bytes(),
            Value::Bytes { data, .. } => data,
            Value::Undefined => return Err(err("fs.put: body is undefined", "EINVAL")),
            other => {
                let mut s = serde_json::to_string_pretty(&other.to_json()).unwrap_or_default();
                s.push('\n');
                s.into_bytes()
            }
        };
        let size = data.len();
        let if_match = input.get("ifMatch").and_then(Value::as_str).map(str::to_owned);
        let etag = self.write(&key, Some(Bytes::from(data)), if_match).await?;
        let mut out = vec![("key", Value::from(key)), ("size", Value::from(size))];
        if let Some(e) = etag {
            out.push(("etag", Value::from(e)));
        }
        Ok(Value::object(out))
    }

    async fn op_delete(&self, c: &Call) -> Result<Value, ToolError> {
        let key = Self::key_arg(c)?;
        let existed = self.read(&key, Some(0..0)).await?.is_some();
        if existed {
            self.write(&key, None, None).await?;
        }
        Ok(Value::from(existed))
    }

    async fn op_copy(&self, c: &Call, remove: bool) -> Result<Value, ToolError> {
        let input = c.input();
        let from = normalize(input.get("from").and_then(Value::as_str).ok_or_else(|| err("needs { from, to }", "EINVAL"))?)?;
        let to = normalize(input.get("to").and_then(Value::as_str).ok_or_else(|| err("needs { from, to }", "EINVAL"))?)?;
        let Some((data, _, _, _)) = self.read(&from, None).await? else {
            return Err(err(format!("{from} does not exist"), "ENOENT"));
        };
        self.write(&to, Some(data), None).await?;
        if remove {
            self.write(&from, None, None).await?;
        }
        Ok(Value::Bool(true))
    }

    async fn op_grep(&self, c: &Call) -> Result<Value, ToolError> {
        let input = c.input();
        let pattern = input.get("pattern").and_then(Value::as_str).ok_or_else(|| err("fs.grep needs { pattern }", "EINVAL"))?;
        let pattern = if input.get("literal").and_then(Value::as_bool).unwrap_or(false) {
            regex::escape(pattern)
        } else {
            pattern.to_owned()
        };
        let re = regex::RegexBuilder::new(&pattern)
            .case_insensitive(input.get("ignoreCase").and_then(Value::as_bool).unwrap_or(false))
            .size_limit(1 << 20)
            .build()
            .map_err(|e| err(format!("bad pattern: {e}"), "EINVAL").with_name("SyntaxError"))?;
        let prefix = normalize(input.get("prefix").and_then(Value::as_str).unwrap_or(""))?;
        let glob = match input.get("glob").and_then(Value::as_str) {
            Some(g) => Some(Glob::new(g).map_err(|e| err(format!("bad glob: {e}"), "EINVAL"))?.compile_matcher()),
            None => None,
        };
        let max = input.get("maxResults").and_then(Value::as_f64).map(|n| n as usize).unwrap_or(200).min(5000);
        let (objects, _) = self.walk(&prefix, true).await?;
        let mut hits = Vec::new();
        'files: for (key, (size, _, _, _)) in objects {
            if glob.as_ref().is_some_and(|g| !g.is_match(&key)) || size > self.cfg().max_read_bytes as u64 {
                continue;
            }
            let Ok(Some((data, _, _, _))) = self.read(&key, None).await else { continue };
            let Ok(text) = std::str::from_utf8(&data) else { continue };
            for (n, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    let mut t = line.trim().to_owned();
                    if t.len() > 240 {
                        let mut cut = 240;
                        while !t.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        t.truncate(cut);
                        t.push('…');
                    }
                    hits.push(Value::object([
                        ("key", Value::from(key.clone())),
                        ("line", Value::from(n + 1)),
                        ("text", Value::from(t)),
                    ]));
                    if hits.len() >= max {
                        break 'files;
                    }
                }
            }
        }
        Ok(Value::Array(hits))
    }
}

/// Staged writes are VM-timeline state: a suspended run resumed in another
/// process must see the same pending changes, so they're sealed into tokens.
impl crate::tools::Extension for Vfs {
    fn name(&self) -> &str {
        "vfs"
    }

    fn save(&self) -> Vec<u8> {
        use base64::Engine as _;
        let b64 = |d: &[u8]| base64::engine::general_purpose::STANDARD.encode(d);
        let st = self.inner.state.lock().unwrap();
        let mut overlay = serde_json::Map::new();
        let mut blobs = serde_json::Map::new();
        for (k, l) in &st.overlay {
            match l {
                Layer::Put { hash, modified_ms } => {
                    overlay.insert(k.clone(), serde_json::json!({ "h": hex(hash), "m": modified_ms }));
                    blobs.entry(hex(hash)).or_insert_with(|| b64(&st.blobs[hash]).into());
                }
                Layer::Deleted => {
                    overlay.insert(k.clone(), serde_json::Value::Null);
                }
            }
        }
        let seen: serde_json::Map<String, serde_json::Value> = st
            .seen
            .iter()
            .map(|(k, s)| {
                (k.clone(), serde_json::json!({ "e": s.existed, "t": s.etag.as_ref().map(|e| &e.0), "h": s.hash.as_ref().map(hex) }))
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({ "overlay": overlay, "blobs": blobs, "seen": seen })).unwrap()
    }

    fn load(&self, data: &[u8]) -> Result<(), String> {
        use base64::Engine as _;
        let j: serde_json::Value = serde_json::from_slice(data).map_err(|e| e.to_string())?;
        let unhex = |s: &str| -> Result<Hash, String> {
            let mut h = [0u8; 32];
            for (i, c) in s.as_bytes().chunks(2).enumerate().take(32) {
                h[i] = u8::from_str_radix(std::str::from_utf8(c).map_err(|e| e.to_string())?, 16).map_err(|e| e.to_string())?;
            }
            Ok(h)
        };
        let mut st = State::default();
        for (hx, b) in j["blobs"].as_object().into_iter().flatten() {
            let bytes = base64::engine::general_purpose::STANDARD.decode(b.as_str().unwrap_or("")).map_err(|e| e.to_string())?;
            let h = unhex(hx)?;
            if hash(&bytes) != h {
                return Err("blob does not match its hash".into());
            }
            st.blobs.insert(h, Bytes::from(bytes));
        }
        for (k, v) in j["overlay"].as_object().into_iter().flatten() {
            let layer = match v {
                serde_json::Value::Null => Layer::Deleted,
                v => {
                    let h = unhex(v["h"].as_str().unwrap_or(""))?;
                    if !st.blobs.contains_key(&h) {
                        return Err(format!("missing blob for {k}"));
                    }
                    Layer::Put { hash: h, modified_ms: v["m"].as_f64().unwrap_or(0.0) }
                }
            };
            st.overlay.insert(k.clone(), layer);
        }
        for (k, v) in j["seen"].as_object().into_iter().flatten() {
            st.seen.insert(
                k.clone(),
                Seen {
                    existed: v["e"].as_bool().unwrap_or(false),
                    etag: v["t"].as_str().map(|s| Etag(s.to_owned())),
                    hash: v["h"].as_str().map(unhex).transpose()?,
                },
            );
        }
        *self.inner.state.lock().unwrap() = st;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn keys() {
        assert_eq!(normalize("./src//a.ts").unwrap(), "src/a.ts");
        assert_eq!(normalize("/src/").unwrap(), "src/");
        assert!(normalize("../etc/passwd").is_err());
        assert!(normalize("a/../../b").is_err());
    }
}
