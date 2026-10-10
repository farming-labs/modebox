//! The storage interface: how modebox talks to any place files live.
//!
//! A [`Store`] is a flat namespace of [`Key`]s mapping to objects, with
//! object-store semantics: every write produces a new version identified by an
//! [`Etag`], writes can be conditional on the current version, and listing is
//! by key prefix. S3, R2, GCS and Azure Blob work this way natively, and a
//! local directory or an in-memory map can satisfy it too.
//!
//! Everything above this trait is backend-agnostic: mounts, staged writes,
//! approvals, deny rules, and the `fs.*` API sandboxed code sees (see
//! [`crate::vfs`]). A new backend implements six methods and passes
//! [`conformance::run`].
//!
//! The full contract, including the out-of-process driver protocol, is in
//! `docs/storage.md`.

pub mod conformance;
pub mod local;
pub mod memory;
#[cfg(feature = "object-store")]
pub mod objstore;

use std::fmt;
use std::ops::Range;

pub use bytes::Bytes;
use futures::future::BoxFuture;

pub type StoreResult<T> = Result<T, StoreError>;
pub type StoreFuture<'a, T> = BoxFuture<'a, StoreResult<T>>;

/// A validated object key: `/`-separated UTF-8, like `src/app/main.ts`.
///
/// Rules (the intersection of what S3, GCS, Azure and POSIX filesystems
/// accept): 1–1024 bytes; no leading `/`; no empty, `.` or `..` segments; no
/// backslashes or control characters. Keys are case-sensitive.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(String);

impl Key {
    pub fn new(s: impl Into<String>) -> StoreResult<Key> {
        let s = s.into();
        let bad = |why: &str| Err(StoreError::InvalidKey(format!("{s:?}: {why}")));
        if s.is_empty() || s.len() > 1024 {
            return bad("must be 1-1024 bytes");
        }
        if s.starts_with('/') {
            return bad("must not start with /");
        }
        if s.chars().any(|c| c == '\\' || c.is_control()) {
            return bad("must not contain backslashes or control characters");
        }
        if s.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..") {
            return bad("segments must not be empty, `.` or `..`");
        }
        Ok(Key(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment (the "file name").
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({:?})", self.0)
    }
}

/// An opaque version token. Two etags of the same key are equal exactly when
/// they name the same version. Whether a rewrite with identical bytes gets a
/// new etag is backend-specific (see [`Capabilities::content_etags`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Etag(pub String);

impl fmt::Display for Etag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct Meta {
    pub key: Key,
    pub size: u64,
    /// Last modified, epoch milliseconds, when the backend knows it.
    pub modified_ms: Option<f64>,
    pub etag: Option<Etag>,
    pub content_type: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Object {
    pub meta: Meta,
    /// The whole object, or the requested range.
    pub body: Bytes,
}

#[derive(Clone, Debug, Default)]
pub struct GetOptions {
    /// Byte range; `end` is clamped to the size, a start past the end yields
    /// an empty body.
    pub range: Option<Range<u64>>,
    /// Fail with [`StoreError::PreconditionFailed`] unless this is the current version.
    pub if_match: Option<Etag>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Precondition {
    /// Write unconditionally.
    #[default]
    None,
    /// Write only if the current version has this etag (compare-and-swap).
    IfMatch(Etag),
    /// Write only if nothing exists at the key (create).
    IfNotExists,
}

#[derive(Clone, Debug, Default)]
pub struct PutOptions {
    pub precondition: Precondition,
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ListOptions {
    /// Raw string prefix (`"src/ma"` matches `"src/main.ts"`), as in S3.
    pub prefix: String,
    /// Roll keys with a further `/` after the prefix into [`ListPage::prefixes`]
    /// (one "directory level"), as with S3's `delimiter=/`.
    pub delimiter: bool,
    /// Continue after a previous page.
    pub cursor: Option<String>,
    /// Page size; backends may return fewer. Default 1000.
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct ListPage {
    /// Sorted by key (byte order).
    pub objects: Vec<Meta>,
    /// Sorted, each ending in `/`. Only with `delimiter`.
    pub prefixes: Vec<String>,
    /// `Some` when more results follow.
    pub cursor: Option<String>,
}

/// What a backend can do. The layers above adapt instead of assuming.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    pub writable: bool,
    /// Preconditions are enforced atomically (compare-and-swap). When false
    /// the backend checks and then writes, which a concurrent writer can race.
    pub atomic_preconditions: bool,
    pub range_reads: bool,
    /// `copy` happens inside the backend (no download/upload).
    pub server_side_copy: bool,
    /// A completed write is visible to the next `list` immediately.
    pub consistent_list: bool,
    /// Etags are derived from content: rewriting identical bytes keeps the etag.
    pub content_etags: bool,
    pub max_object_size: Option<u64>,
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities {
            writable: true,
            atomic_preconditions: false,
            range_reads: true,
            server_side_copy: false,
            consistent_list: true,
            content_etags: false,
            max_object_size: None,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    #[error("{0}: not found")]
    NotFound(String),
    #[error("{0}: already exists")]
    AlreadyExists(String),
    #[error("{0}: precondition failed (it changed since it was read)")]
    PreconditionFailed(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("invalid key: {0}")]
    InvalidKey(String),
    #[error("{key}: too large ({size} bytes)")]
    TooLarge { key: String, size: u64 },
    #[error("not supported: {0}")]
    Unsupported(String),
    /// A size limit on the store or on staged changes was reached.
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    /// Worth retrying (throttling, network).
    #[error("temporary failure: {0}")]
    Transient(String),
    #[error("{0}")]
    Other(String),
}

impl StoreError {
    /// Stable error code, surfaced to sandboxed code as `error.code`.
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::NotFound(_) => "ENOENT",
            StoreError::AlreadyExists(_) => "EEXIST",
            StoreError::PreconditionFailed(_) => "PRECONDITION_FAILED",
            StoreError::PermissionDenied(_) => "EACCES",
            StoreError::InvalidKey(_) => "EINVAL",
            StoreError::TooLarge { .. } => "EFBIG",
            StoreError::Unsupported(_) => "ENOTSUP",
            StoreError::QuotaExceeded(_) => "ENOSPC",
            StoreError::Transient(_) => "EAGAIN",
            StoreError::Other(_) => "EIO",
        }
    }
}

/// A storage backend. See the module docs and `docs/storage.md` for the
/// contract; [`conformance::run`] checks an implementation against it.
pub trait Store: Send + Sync + 'static {
    /// Where the data lives, for humans: `file:///repo`, `s3://bucket/prefix`.
    fn describe(&self) -> String;

    fn capabilities(&self) -> Capabilities;

    /// Metadata, or `None` if absent.
    fn head<'a>(&'a self, key: &'a Key) -> StoreFuture<'a, Option<Meta>>;

    /// Contents (or a range), or `None` if absent.
    fn get<'a>(&'a self, key: &'a Key, opts: GetOptions) -> StoreFuture<'a, Option<Object>>;

    /// Write a whole object atomically: readers see the old or the new version, never a mix.
    fn put<'a>(&'a self, key: &'a Key, body: Bytes, opts: PutOptions) -> StoreFuture<'a, Meta>;

    /// Remove; returns whether something was there. With `if_match`, only
    /// that version is removed.
    fn delete<'a>(&'a self, key: &'a Key, if_match: Option<Etag>) -> StoreFuture<'a, bool>;

    fn list<'a>(&'a self, opts: ListOptions) -> StoreFuture<'a, ListPage>;

    /// Copy within the store. Default: read then write.
    fn copy<'a>(&'a self, from: &'a Key, to: &'a Key, opts: PutOptions) -> StoreFuture<'a, Meta> {
        Box::pin(async move {
            let obj = self
                .get(from, GetOptions::default())
                .await?
                .ok_or_else(|| StoreError::NotFound(from.to_string()))?;
            let content_type = opts.content_type.clone().or(obj.meta.content_type);
            self.put(to, obj.body, PutOptions { content_type, ..opts }).await
        })
    }
}

/// Every object under `prefix`, across pages.
pub async fn list_all(store: &dyn Store, prefix: &str, max: usize) -> StoreResult<Vec<Meta>> {
    let mut out = Vec::new();
    let mut cursor = None;
    loop {
        let page = store
            .list(ListOptions { prefix: prefix.to_owned(), delimiter: false, cursor, limit: Some(1000) })
            .await?;
        out.extend(page.objects);
        if out.len() >= max {
            out.truncate(max);
            return Ok(out);
        }
        match page.cursor {
            Some(c) => cursor = Some(c),
            None => return Ok(out),
        }
    }
}

/// Shared paging for backends that can enumerate all matching keys: sort,
/// roll up delimiters, resume after the cursor, cut at the limit.
pub(crate) fn paginate(mut all: Vec<Meta>, opts: &ListOptions) -> ListPage {
    all.retain(|m| m.key.as_str().starts_with(&opts.prefix));
    all.sort_by(|a, b| a.key.cmp(&b.key));
    let limit = opts.limit.unwrap_or(1000).max(1);
    // With a delimiter, entries are objects or rolled-up prefixes, interleaved in key order.
    enum Entry {
        Obj(Meta),
        Pre(String),
    }
    let mut entries: Vec<(String, Entry)> = Vec::new();
    let mut last_prefix: Option<String> = None;
    for m in all {
        let rest = &m.key.as_str()[opts.prefix.len()..];
        if opts.delimiter
            && let Some(i) = rest.find('/')
        {
            let p = format!("{}{}", opts.prefix, &rest[..=i]);
            if last_prefix.as_deref() != Some(p.as_str()) {
                last_prefix = Some(p.clone());
                entries.push((p.clone(), Entry::Pre(p)));
            }
            continue;
        }
        entries.push((m.key.as_str().to_owned(), Entry::Obj(m)));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup_by(|a, b| a.0 == b.0);
    let start = match &opts.cursor {
        Some(c) => entries.partition_point(|(k, _)| k.as_str() <= c.as_str()),
        None => 0,
    };
    let mut page = ListPage::default();
    let mut last: Option<String> = None;
    for (k, e) in entries.into_iter().skip(start) {
        if page.objects.len() + page.prefixes.len() == limit {
            page.cursor = last;
            break;
        }
        match e {
            Entry::Obj(m) => page.objects.push(m),
            Entry::Pre(p) => page.prefixes.push(p),
        }
        last = Some(k);
    }
    page
}

/// Guess a content type from the key's extension.
pub fn content_type_for(key: &str) -> Option<&'static str> {
    let ext = key.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" | "mts" => "text/typescript",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "xml" => "application/xml",
        "rs" | "py" | "go" | "java" | "c" | "h" | "cpp" | "rb" | "sh" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "wasm" => "application/wasm",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert!(Key::new("src/main.ts").is_ok());
        assert!(Key::new("dir/naïve ünïcode file.txt").is_ok());
        for bad in ["", "/abs", "a//b", "a/./b", "a/../b", "..", "a\\b", "tab\there", "trailing/"] {
            assert!(Key::new(bad).is_err(), "{bad:?} should be rejected");
        }
        assert_eq!(Key::new("a/b/c.txt").unwrap().name(), "c.txt");
    }

    fn meta(k: &str) -> Meta {
        Meta { key: Key::new(k).unwrap(), size: 0, modified_ms: None, etag: None, content_type: None }
    }

    #[test]
    fn paging_and_delimiters() {
        let all = || ["a.txt", "dir/x", "dir/y", "dir/sub/z", "e.txt", "f/g"].map(meta).to_vec();
        let p = paginate(all(), &ListOptions { delimiter: true, ..Default::default() });
        assert_eq!(p.objects.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), ["a.txt", "e.txt"]);
        assert_eq!(p.prefixes, ["dir/", "f/"]);

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let p = paginate(all(), &ListOptions { limit: Some(2), cursor: cursor.clone(), ..Default::default() });
            seen.extend(p.objects.iter().map(|m| m.key.as_str().to_owned()));
            match p.cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(seen, ["a.txt", "dir/sub/z", "dir/x", "dir/y", "e.txt", "f/g"]);
    }
}
