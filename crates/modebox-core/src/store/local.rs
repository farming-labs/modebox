//! A local directory as a [`Store`].
//!
//! Keys map to paths under the root. Every access is jailed: a key's path is
//! resolved (symlinks included) and must stay inside the root, and files are
//! then opened refusing any symlink, so a path swapped for one in between
//! can't redirect the access (see `nofollow`). Writes go to a temporary file
//! that is renamed into place, so they never change a file's contents in
//! place, and a hard link can't carry them outside the root. Etags are weak
//! version tokens from size, mtime and inode, so they're cheap and change on
//! every write. `IfNotExists` is atomic (hard-link create); `IfMatch` is
//! checked just before an atomic rename.

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

    /// The real location of `key`: symlinks resolved, refused if that leaves
    /// the root. Parts that don't exist yet are appended as they are.
    fn path(&self, key: &Key) -> StoreResult<PathBuf> {
        let path = self.root.join(key.as_str());
        let mut probe = path.as_path();
        loop {
            if let Ok(real) = probe.canonicalize() {
                if !real.starts_with(&self.root) {
                    return Err(StoreError::PermissionDenied(format!("{key} resolves outside the store")));
                }
                let rest = path.strip_prefix(probe).unwrap_or(Path::new(""));
                return Ok(if rest.as_os_str().is_empty() { real } else { real.join(rest) });
            }
            match probe.parent() {
                Some(p) => probe = p,
                None => return Ok(path),
            }
        }
    }

    /// The directory holding `key`'s entry (resolved like [`Self::path`]) and
    /// the entry's name. Writes and deletes act on the entry itself, so a
    /// symlink is replaced or removed, not its target.
    fn parent(&self, key: &Key) -> StoreResult<(PathBuf, String)> {
        match key.as_str().rsplit_once('/') {
            Some((dir, name)) => Ok((self.path(&Key::new(dir)?)?, name.to_owned())),
            None => Ok((self.root.clone(), key.as_str().to_owned())),
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

    /// `key` opened for reading, or `None` if it isn't a regular file.
    fn open_file(&self, key: &Key) -> StoreResult<Option<(std::fs::File, std::fs::Metadata)>> {
        let f = match nofollow::open_read(&self.path(key)?) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(race_or_io(key, e)),
        };
        let m = f.metadata().map_err(|e| io_err(key, e))?;
        Ok(m.is_file().then_some((f, m)))
    }

    fn head_sync(&self, key: &Key) -> StoreResult<Option<Meta>> {
        Ok(self.open_file(key)?.map(|(_, m)| self.meta_for(key.clone(), &m)))
    }

    fn get_sync(&self, key: &Key, opts: GetOptions) -> StoreResult<Option<Object>> {
        let Some((mut f, m)) = self.open_file(key)? else { return Ok(None) };
        let meta = self.meta_for(key.clone(), &m);
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
        let (dir, _) = self.parent(key)?;
        std::fs::create_dir_all(&dir).map_err(|e| io_err(key, e))?;
        // Resolve again now that the directories exist, then work inside the
        // directory itself: if its path changes from here on, the open fails
        // or the write still lands where it was checked.
        let (dir, name) = self.parent(key)?;
        let dir = nofollow::Dir::open(&dir).map_err(|e| race_or_io(key, e))?;
        if dir.is_dir(&name) {
            return Err(StoreError::AlreadyExists(format!("{key} (a directory)")));
        }
        let tmp = format!(".{name}.modebox-{}-{}", std::process::id(), rand::random::<u32>());
        dir.write_new(&tmp, &body).map_err(|e| io_err(key, e))?;
        let cleanup = |e: StoreError| {
            let _ = dir.remove(&tmp);
            e
        };
        match &opts.precondition {
            Precondition::IfNotExists => {
                // link(2) fails if the target exists: an atomic create.
                match dir.link(&tmp, &name) {
                    Ok(()) => {
                        let _ = dir.remove(&tmp);
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
                dir.rename(&tmp, &name).map_err(|e| cleanup(io_err(key, e)))?;
            }
            Precondition::None => dir.rename(&tmp, &name).map_err(|e| cleanup(io_err(key, e)))?,
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
        let (dir, name) = self.parent(key)?;
        let dir = nofollow::Dir::open(&dir).map_err(|e| race_or_io(key, e))?;
        match dir.remove(&name) {
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

/// An open that failed because a symlink appeared on an already-checked path.
fn race_or_io(key: &Key, e: std::io::Error) -> StoreError {
    #[cfg(unix)]
    if matches!(e.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR)) {
        return StoreError::PermissionDenied(format!("{key}: its path changed while it was being opened"));
    }
    io_err(key, e)
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

/// Opening checked paths without following symlinks that appear later.
///
/// `LocalStore::path` resolves a key to its real location and checks that it
/// is inside the root, but another process could replace a directory on that
/// path with a symlink before the file is opened. A real path contains no
/// symlinks, so files are opened refusing one in *any* component:
/// `O_NOFOLLOW_ANY` on macOS, `openat2(RESOLVE_NO_SYMLINKS)` on Linux 5.6+
/// (older kernels can only pin the last component). Writes and deletes use
/// the `*at` calls inside a directory opened that way, so they land in the
/// directory that was checked whatever happens to its path afterwards.
#[cfg(unix)]
mod nofollow {
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io::{self, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn cstr(s: &OsStr) -> io::Result<CString> {
        CString::new(s.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
    }

    fn fd(raw: libc::c_int) -> io::Result<OwnedFd> {
        if raw < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: a new descriptor that nothing else owns.
            Ok(unsafe { OwnedFd::from_raw_fd(raw) })
        }
    }

    fn ok(r: libc::c_int) -> io::Result<()> {
        if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// open(2) on a path that must not contain a symlink.
    fn open(path: &Path, flags: libc::c_int) -> io::Result<OwnedFd> {
        let c = cstr(path.as_os_str())?;
        let flags = flags | libc::O_CLOEXEC;
        #[cfg(target_os = "linux")]
        {
            // SAFETY: all-zero is a valid `open_how` (it's non-exhaustive, so no literal).
            let mut how: libc::open_how = unsafe { std::mem::zeroed() };
            how.flags = flags as u64;
            how.resolve = libc::RESOLVE_NO_SYMLINKS;
            // SAFETY: valid C string and `open_how`, with its size.
            let r = unsafe {
                libc::syscall(libc::SYS_openat2, libc::AT_FDCWD, c.as_ptr(), &how as *const libc::open_how, size_of::<libc::open_how>())
            };
            if r >= 0 {
                return fd(r as libc::c_int);
            }
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ENOSYS) {
                return Err(e);
            }
            // SAFETY: valid C string.
            fd(unsafe { libc::open(c.as_ptr(), flags | libc::O_NOFOLLOW) })
        }
        #[cfg(target_vendor = "apple")]
        {
            // SAFETY: valid C string.
            fd(unsafe { libc::open(c.as_ptr(), flags | libc::O_NOFOLLOW_ANY) })
        }
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        {
            // SAFETY: valid C string.
            fd(unsafe { libc::open(c.as_ptr(), flags | libc::O_NOFOLLOW) })
        }
    }

    /// A file to read. Non-blocking, so opening a FIFO can't hang.
    pub fn open_read(path: &Path) -> io::Result<File> {
        open(path, libc::O_RDONLY | libc::O_NONBLOCK).map(File::from)
    }

    /// A directory, for operating on its entries by name.
    pub struct Dir(OwnedFd);

    impl Dir {
        pub fn open(path: &Path) -> io::Result<Dir> {
            open(path, libc::O_RDONLY | libc::O_DIRECTORY).map(Dir)
        }

        fn raw(&self) -> libc::c_int {
            self.0.as_raw_fd()
        }

        /// Create `name`, which must not exist, holding `body`.
        pub fn write_new(&self, name: &str, body: &[u8]) -> io::Result<()> {
            let c = cstr(OsStr::new(name))?;
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            // SAFETY: valid directory descriptor and C string.
            let f = fd(unsafe { libc::openat(self.raw(), c.as_ptr(), flags, 0o644 as libc::c_uint) })?;
            File::from(f).write_all(body)
        }

        pub fn link(&self, from: &str, to: &str) -> io::Result<()> {
            let (a, b) = (cstr(OsStr::new(from))?, cstr(OsStr::new(to))?);
            // SAFETY: valid descriptors and C strings.
            ok(unsafe { libc::linkat(self.raw(), a.as_ptr(), self.raw(), b.as_ptr(), 0) })
        }

        pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
            let (a, b) = (cstr(OsStr::new(from))?, cstr(OsStr::new(to))?);
            // SAFETY: valid descriptors and C strings.
            ok(unsafe { libc::renameat(self.raw(), a.as_ptr(), self.raw(), b.as_ptr()) })
        }

        pub fn remove(&self, name: &str) -> io::Result<()> {
            let c = cstr(OsStr::new(name))?;
            // SAFETY: valid descriptor and C string.
            ok(unsafe { libc::unlinkat(self.raw(), c.as_ptr(), 0) })
        }

        /// Whether the entry itself (not a symlink's target) is a directory.
        pub fn is_dir(&self, name: &str) -> bool {
            let Ok(c) = cstr(OsStr::new(name)) else { return false };
            // SAFETY: zeroed `stat` is valid; fstatat fills it.
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            let r = unsafe { libc::fstatat(self.raw(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
            r == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFDIR
        }
    }
}

/// Elsewhere, the same operations by path (without the race protection).
#[cfg(not(unix))]
mod nofollow {
    use std::fs::File;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    pub fn open_read(path: &Path) -> io::Result<File> {
        File::open(path)
    }

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn open(path: &Path) -> io::Result<Dir> {
            if path.is_dir() { Ok(Dir(path.to_owned())) } else { Err(io::Error::new(io::ErrorKind::NotFound, "not a directory")) }
        }

        pub fn write_new(&self, name: &str, body: &[u8]) -> io::Result<()> {
            std::fs::OpenOptions::new().write(true).create_new(true).open(self.0.join(name))?.write_all(body)
        }

        pub fn link(&self, from: &str, to: &str) -> io::Result<()> {
            std::fs::hard_link(self.0.join(from), self.0.join(to))
        }

        pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
            std::fs::rename(self.0.join(from), self.0.join(to))
        }

        pub fn remove(&self, name: &str) -> io::Result<()> {
            std::fs::remove_file(self.0.join(name))
        }

        pub fn is_dir(&self, name: &str) -> bool {
            std::fs::symlink_metadata(self.0.join(name)).is_ok_and(|m| m.is_dir())
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::nofollow;

    #[test]
    fn opens_refuse_a_symlinked_component() {
        let dir = std::env::temp_dir().join(format!("modebox-nofollow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).unwrap();
        let dir = dir.canonicalize().unwrap();
        std::fs::write(dir.join("real/f.txt"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("alias")).unwrap();
        // What a race would leave: a checked path whose directory is now a symlink.
        assert!(nofollow::open_read(&dir.join("real/f.txt")).is_ok());
        assert!(nofollow::open_read(&dir.join("alias/f.txt")).is_err());
        assert!(nofollow::Dir::open(&dir.join("alias")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
