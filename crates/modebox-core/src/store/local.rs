//! A local directory as a [`Store`].
//!
//! Keys map to paths under the root. Every access is jailed: the deepest
//! existing ancestor of a path is canonicalized (resolving symlinks) and must
//! stay inside the root. Etags are weak version tokens from size, mtime and
//! inode, so they're cheap and change on every write. `IfNotExists` is atomic
//! (hard-link create); `IfMatch` is checked just before an atomic rename.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::*;

#[derive(Clone, Debug)]
pub struct LocalOptions {
    pub writable: bool,
    /// Skip what `.gitignore` excludes when listing (reads still work).
    pub respect_gitignore: bool,
    /// Skip dotfiles and dot-directories when listing (reads still work).
    pub skip_hidden: bool,
    pub max_object_size: u64,
}

impl Default for LocalOptions {
    fn default() -> Self {
        LocalOptions { writable: true, respect_gitignore: true, skip_hidden: true, max_object_size: 256 << 20 }
    }
}

pub struct LocalStore {
    root: PathBuf,
    opts: LocalOptions,
}

impl LocalStore {
    pub fn new(root: impl AsRef<Path>, opts: LocalOptions) -> StoreResult<LocalStore> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|e| StoreError::Other(format!("{}: {e}", root.as_ref().display())))?;
        if !root.is_dir() {
            return Err(StoreError::Other(format!("{} is not a directory", root.display())));
        }
        Ok(LocalStore { root, opts })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path for `key`, refusing anything that resolves outside the root.
    fn path(&self, key: &Key) -> StoreResult<PathBuf> {
        let path = self.root.join(key.as_str());
        let mut probe = path.as_path();
        loop {
            if let Ok(real) = probe.canonicalize() {
                if !real.starts_with(&self.root) {
                    return Err(StoreError::PermissionDenied(format!("{key} resolves outside the store")));
                }
                return Ok(path);
            }
            match probe.parent() {
                Some(p) => probe = p,
                None => return Ok(path),
            }
        }
    }

    fn meta_for(&self, key: Key, m: &std::fs::Metadata) -> Meta {
        Meta {
            content_type: content_type_for(key.as_str()).map(str::to_owned),
            key,
            size: m.len(),
            modified_ms: mtime_ms(m),
            etag: Some(weak_etag(m)),
        }
    }

    fn head_sync(&self, key: &Key) -> StoreResult<Option<Meta>> {
        let path = self.path(key)?;
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => Ok(Some(self.meta_for(key.clone(), &m))),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(key, e)),
        }
    }

    fn get_sync(&self, key: &Key, opts: GetOptions) -> StoreResult<Option<Object>> {
        let Some(meta) = self.head_sync(key)? else { return Ok(None) };
        if let Some(want) = &opts.if_match
            && meta.etag.as_ref() != Some(want)
        {
            return Err(StoreError::PreconditionFailed(key.to_string()));
        }
        let (start, end) = match &opts.range {
            Some(r) => (r.start.min(meta.size), r.end.min(meta.size).max(r.start.min(meta.size))),
            None => (0, meta.size),
        };
        if end - start > self.opts.max_object_size {
            return Err(StoreError::TooLarge { key: key.to_string(), size: meta.size });
        }
        let mut f = std::fs::File::open(self.path(key)?).map_err(|e| io_err(key, e))?;
        f.seek(SeekFrom::Start(start)).map_err(|e| io_err(key, e))?;
        let mut buf = vec![0u8; (end - start) as usize];
        f.read_exact(&mut buf).map_err(|e| io_err(key, e))?;
        Ok(Some(Object { meta, body: Bytes::from(buf) }))
    }

    fn put_sync(&self, key: &Key, body: Bytes, opts: PutOptions) -> StoreResult<Meta> {
        if !self.opts.writable {
            return Err(StoreError::PermissionDenied(format!("{key}: store is read-only")));
        }
        if body.len() as u64 > self.opts.max_object_size {
            return Err(StoreError::TooLarge { key: key.to_string(), size: body.len() as u64 });
        }
        let path = self.path(key)?;
        let parent = path.parent().ok_or_else(|| StoreError::InvalidKey(key.to_string()))?;
        std::fs::create_dir_all(parent).map_err(|e| io_err(key, e))?;
        // Re-check now that the parents exist (a symlinked dir could point out).
        self.path(key)?;
        if path.is_dir() {
            return Err(StoreError::AlreadyExists(format!("{key} (a directory)")));
        }
        let tmp = parent.join(format!(".{}.modebox-{}-{}", key.name(), std::process::id(), rand::random::<u32>()));
        std::fs::write(&tmp, &body).map_err(|e| io_err(key, e))?;
        let cleanup = |e: StoreError| {
            let _ = std::fs::remove_file(&tmp);
            e
        };
        match &opts.precondition {
            Precondition::IfNotExists => {
                // link(2) fails if the target exists: an atomic create.
                match std::fs::hard_link(&tmp, &path) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&tmp);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Err(cleanup(StoreError::AlreadyExists(key.to_string())));
                    }
                    Err(e) => return Err(cleanup(io_err(key, e))),
                }
            }
            Precondition::IfMatch(want) => {
                let current = self.head_sync(key).map_err(cleanup)?;
                if current.and_then(|m| m.etag).as_ref() != Some(want) {
                    return Err(cleanup(StoreError::PreconditionFailed(key.to_string())));
                }
                std::fs::rename(&tmp, &path).map_err(|e| cleanup(io_err(key, e)))?;
            }
            Precondition::None => std::fs::rename(&tmp, &path).map_err(|e| cleanup(io_err(key, e)))?,
        }
        self.head_sync(key)?.ok_or_else(|| StoreError::Other(format!("{key}: vanished after write")))
    }

    fn delete_sync(&self, key: &Key, if_match: Option<Etag>) -> StoreResult<bool> {
        if !self.opts.writable {
            return Err(StoreError::PermissionDenied(format!("{key}: store is read-only")));
        }
        let Some(meta) = self.head_sync(key)? else { return Ok(false) };
        if let Some(want) = if_match
            && meta.etag != Some(want)
        {
            return Err(StoreError::PreconditionFailed(key.to_string()));
        }
        match std::fs::remove_file(self.path(key)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_err(key, e)),
        }
    }

    fn list_sync(&self, opts: ListOptions) -> StoreResult<ListPage> {
        // Walk only the directory that can contain the prefix.
        let dir = match opts.prefix.rfind('/') {
            Some(i) => &opts.prefix[..i],
            None => "",
        };
        let start = if dir.is_empty() { self.root.clone() } else { self.path(&Key::new(dir)?)? };
        if !start.is_dir() {
            return Ok(ListPage::default());
        }
        let mut wb = ignore::WalkBuilder::new(&start);
        wb.hidden(self.opts.skip_hidden)
            .git_ignore(self.opts.respect_gitignore)
            .git_exclude(self.opts.respect_gitignore)
            .git_global(false)
            .ignore(false)
            .parents(self.opts.respect_gitignore)
            .require_git(false)
            .follow_links(false);
        if opts.delimiter {
            // Files directly in `dir`, plus one level of sub-directories to roll up.
            wb.max_depth(Some(1));
        }
        let mut all = Vec::new();
        for entry in wb.build().flatten() {
            let Ok(rel) = entry.path().strip_prefix(&self.root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel.is_empty() || !rel.starts_with(opts.prefix.as_str()) && !opts.prefix.starts_with(&rel) {
                continue;
            }
            let Ok(key) = Key::new(rel.clone()) else { continue };
            let Ok(m) = entry.metadata() else { continue };
            if m.is_file() {
                all.push(self.meta_for(key, &m));
            } else if m.is_dir() && opts.delimiter && rel.starts_with(opts.prefix.as_str()) {
                // Represent the directory by a placeholder child so paginate rolls it up.
                if let Ok(k) = Key::new(format!("{rel}/\u{10FFFF}")) {
                    all.push(Meta { key: k, size: 0, modified_ms: None, etag: None, content_type: None });
                }
            }
        }
        Ok(paginate(all, &opts))
    }
}

fn mtime_ms(m: &std::fs::Metadata) -> Option<f64> {
    m.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs_f64() * 1000.0)
}

fn weak_etag(m: &std::fs::Metadata) -> Etag {
    let nanos = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(m);
    #[cfg(not(unix))]
    let ino = 0u64;
    let h = blake3::hash(format!("{}:{nanos}:{ino}", m.len()).as_bytes());
    Etag(format!("W/{}", &h.to_hex()[..20]))
}

fn io_err(key: &Key, e: std::io::Error) -> StoreError {
    match e.kind() {
        std::io::ErrorKind::NotFound => StoreError::NotFound(key.to_string()),
        std::io::ErrorKind::PermissionDenied => StoreError::PermissionDenied(format!("{key}: {e}")),
        std::io::ErrorKind::AlreadyExists => StoreError::AlreadyExists(key.to_string()),
        _ => StoreError::Other(format!("{key}: {e}")),
    }
}

/// Run blocking filesystem work off the async executor.
fn blocking<T: Send + 'static>(f: impl FnOnce() -> StoreResult<T> + Send + 'static) -> StoreFuture<'static, T> {
    Box::pin(async move {
        tokio::task::spawn_blocking(f).await.map_err(|e| StoreError::Other(format!("task failed: {e}")))?
    })
}

impl Store for LocalStore {
    fn describe(&self) -> String {
        format!("file://{}", self.root.display())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            writable: self.opts.writable,
            atomic_preconditions: false,
            // Creates go through hard_link, which fails if the name exists.
            atomic_create: true,
            range_reads: true,
            server_side_copy: false,
            consistent_list: true,
            content_etags: false,
            max_object_size: Some(self.opts.max_object_size),
        }
    }

    fn head<'a>(&'a self, key: &'a Key) -> StoreFuture<'a, Option<Meta>> {
        let (this, key) = (self.clone_ref(), key.clone());
        blocking(move || this.head_sync(&key))
    }

    fn get<'a>(&'a self, key: &'a Key, opts: GetOptions) -> StoreFuture<'a, Option<Object>> {
        let (this, key) = (self.clone_ref(), key.clone());
        blocking(move || this.get_sync(&key, opts))
    }

    fn put<'a>(&'a self, key: &'a Key, body: Bytes, opts: PutOptions) -> StoreFuture<'a, Meta> {
        let (this, key) = (self.clone_ref(), key.clone());
        blocking(move || this.put_sync(&key, body, opts))
    }

    fn delete<'a>(&'a self, key: &'a Key, if_match: Option<Etag>) -> StoreFuture<'a, bool> {
        let (this, key) = (self.clone_ref(), key.clone());
        blocking(move || this.delete_sync(&key, if_match))
    }

    fn list<'a>(&'a self, opts: ListOptions) -> StoreFuture<'a, ListPage> {
        let this = self.clone_ref();
        Box::pin(async move {
            let mut page = blocking(move || this.list_sync(opts)).await?;
            // Placeholders only exist to produce prefixes.
            page.objects.retain(|m| !m.key.as_str().ends_with('\u{10FFFF}'));
            Ok(page)
        })
    }
}

impl LocalStore {
    fn clone_ref(&self) -> LocalStore {
        LocalStore { root: self.root.clone(), opts: self.opts.clone() }
    }
}
