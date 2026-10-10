//! Any `object_store` backend as a [`Store`]: Amazon S3 and S3-compatible
//! (Cloudflare R2, MinIO, Tigris…), Google Cloud Storage, Azure Blob, plain
//! HTTP/WebDAV, plus `object_store`'s own memory and local implementations.
//!
//! ```text
//! s3://bucket/prefix        AWS_* env vars or options (region, endpoint, …)
//! gs://bucket/prefix        GOOGLE_* env / service account
//! az://container/prefix     AZURE_* env
//! https://host/path         read-mostly HTTP / WebDAV
//! memory://                 in-memory
//! ```
//!
//! Conditional writes map to `PutMode::Create` / `PutMode::Update`. Backends
//! that don't support them natively fall back to check-then-write, and
//! `capabilities()` reports that honestly.

use std::sync::Arc;

use futures::TryStreamExt;
use object_store::path::Path as OsPath;
use object_store::{
    GetOptions as OsGetOptions, GetRange, ObjectStore, ObjectStoreExt, PutMode, PutOptions as OsPutOptions,
    PutPayload, UpdateVersion,
};

use super::*;

pub struct ObjectStoreAdapter {
    inner: Arc<dyn ObjectStore>,
    location: String,
    caps: Capabilities,
}

impl ObjectStoreAdapter {
    pub fn new(inner: Arc<dyn ObjectStore>, location: impl Into<String>, caps: Capabilities) -> Self {
        ObjectStoreAdapter { inner, location: location.into(), caps }
    }

    /// Build from a URL such as `s3://bucket/prefix`, with driver options
    /// (`region`, `endpoint`, `access_key_id`, …). Credentials also come from
    /// the usual environment variables. They stay on the host: sandboxed code
    /// never sees them.
    pub fn from_url<I, K, V>(url: &str, options: I, read_only: bool) -> StoreResult<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: Into<String>,
    {
        let parsed = url::Url::parse(url).map_err(|e| StoreError::Other(format!("{url}: {e}")))?;
        let (store, prefix) = object_store::parse_url_opts(&parsed, options).map_err(map_err)?;
        let store: Arc<dyn ObjectStore> = if prefix.as_ref().is_empty() {
            Arc::from(store)
        } else {
            Arc::new(object_store::prefix::PrefixStore::new(store, prefix))
        };
        let scheme = parsed.scheme();
        let caps = Capabilities {
            writable: !read_only,
            // S3 needs conditional-put support enabled on the bucket/endpoint;
            // GCS, Azure and memory support it natively.
            atomic_preconditions: matches!(scheme, "gs" | "az" | "azure" | "abfs" | "abfss" | "memory"),
            range_reads: true,
            server_side_copy: scheme != "http" && scheme != "https",
            consistent_list: true,
            content_etags: false,
            max_object_size: Some(5 << 30),
        };
        Ok(ObjectStoreAdapter { inner: store, location: url.to_owned(), caps })
    }
}

fn path(key: &Key) -> StoreResult<OsPath> {
    OsPath::parse(key.as_str()).map_err(|e| StoreError::InvalidKey(e.to_string()))
}

fn map_err(e: object_store::Error) -> StoreError {
    use object_store::Error as E;
    match e {
        E::NotFound { path, .. } => StoreError::NotFound(path),
        E::AlreadyExists { path, .. } => StoreError::AlreadyExists(path),
        E::Precondition { path, .. } => StoreError::PreconditionFailed(path),
        E::NotModified { path, .. } => StoreError::PreconditionFailed(path),
        E::NotSupported { source } => StoreError::Unsupported(source.to_string()),
        E::NotImplemented { operation, .. } => StoreError::Unsupported(operation),
        E::PermissionDenied { path, source } => StoreError::PermissionDenied(format!("{path}: {source}")),
        E::Unauthenticated { path, source } => StoreError::PermissionDenied(format!("{path}: {source}")),
        E::InvalidPath { source } => StoreError::InvalidKey(source.to_string()),
        other => StoreError::Transient(other.to_string()),
    }
}

fn meta(m: &object_store::ObjectMeta) -> StoreResult<Meta> {
    let key = Key::new(m.location.to_string())?;
    Ok(Meta {
        content_type: content_type_for(key.as_str()).map(str::to_owned),
        key,
        size: m.size,
        modified_ms: Some(m.last_modified.timestamp_millis() as f64),
        etag: m.e_tag.clone().map(Etag),
    })
}

impl Store for ObjectStoreAdapter {
    fn describe(&self) -> String {
        self.location.clone()
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    fn head<'a>(&'a self, key: &'a Key) -> StoreFuture<'a, Option<Meta>> {
        Box::pin(async move {
            match self.inner.head(&path(key)?).await {
                Ok(m) => Ok(Some(meta(&m)?)),
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Err(e) => Err(map_err(e)),
            }
        })
    }

    fn get<'a>(&'a self, key: &'a Key, opts: GetOptions) -> StoreFuture<'a, Option<Object>> {
        Box::pin(async move {
            let mut o = OsGetOptions { if_match: opts.if_match.as_ref().map(|e| e.0.clone()), ..Default::default() };
            if let Some(r) = &opts.range {
                // Clamp like the contract says instead of erroring on odd ranges.
                let Some(m) = self.head(key).await? else { return Ok(None) };
                let (s, e) = (r.start.min(m.size), r.end.min(m.size).max(r.start.min(m.size)));
                if s == e {
                    return Ok(Some(Object { meta: m, body: Bytes::new() }));
                }
                o.range = Some(GetRange::Bounded(s..e));
            }
            match self.inner.get_opts(&path(key)?, o).await {
                Ok(r) => {
                    let m = meta(&r.meta)?;
                    let body = r.bytes().await.map_err(map_err)?;
                    Ok(Some(Object { meta: m, body }))
                }
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Err(e) => Err(map_err(e)),
            }
        })
    }

    fn put<'a>(&'a self, key: &'a Key, body: Bytes, opts: PutOptions) -> StoreFuture<'a, Meta> {
        Box::pin(async move {
            if !self.caps.writable {
                return Err(StoreError::PermissionDenied(format!("{key}: store is read-only")));
            }
            let p = path(key)?;
            let mode = match &opts.precondition {
                Precondition::None => PutMode::Overwrite,
                Precondition::IfNotExists => PutMode::Create,
                Precondition::IfMatch(e) => PutMode::Update(UpdateVersion { e_tag: Some(e.0.clone()), version: None }),
            };
            let po = OsPutOptions { mode, ..Default::default() };
            match self.inner.put_opts(&p, PutPayload::from(body.clone()), po).await {
                Ok(_) => {}
                Err(object_store::Error::NotImplemented { .. } | object_store::Error::NotSupported { .. })
                    if opts.precondition != Precondition::None =>
                {
                    // Emulate: check, then write. Not atomic (see capabilities).
                    let current = self.head(key).await?;
                    match (&opts.precondition, &current) {
                        (Precondition::IfNotExists, Some(_)) => return Err(StoreError::AlreadyExists(key.to_string())),
                        (Precondition::IfMatch(want), cur) if cur.as_ref().and_then(|m| m.etag.as_ref()) != Some(want) => {
                            return Err(StoreError::PreconditionFailed(key.to_string()));
                        }
                        _ => {}
                    }
                    self.inner.put(&p, PutPayload::from(body)).await.map_err(map_err)?;
                }
                Err(e) => return Err(map_err(e)),
            }
            self.head(key).await?.ok_or_else(|| StoreError::Other(format!("{key}: vanished after write")))
        })
    }

    fn delete<'a>(&'a self, key: &'a Key, if_match: Option<Etag>) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            if !self.caps.writable {
                return Err(StoreError::PermissionDenied(format!("{key}: store is read-only")));
            }
            let Some(m) = self.head(key).await? else { return Ok(false) };
            if let Some(want) = if_match
                && m.etag != Some(want)
            {
                return Err(StoreError::PreconditionFailed(key.to_string()));
            }
            match self.inner.delete(&path(key)?).await {
                Ok(()) => Ok(true),
                Err(object_store::Error::NotFound { .. }) => Ok(false),
                Err(e) => Err(map_err(e)),
            }
        })
    }

    fn list<'a>(&'a self, opts: ListOptions) -> StoreFuture<'a, ListPage> {
        Box::pin(async move {
            // object_store lists by directory; list the directory containing
            // the prefix and filter by the raw string prefix.
            let dir = match opts.prefix.rfind('/') {
                Some(i) => Some(OsPath::parse(&opts.prefix[..i]).map_err(|e| StoreError::InvalidKey(e.to_string()))?),
                None => None,
            };
            let mut all = Vec::new();
            if opts.delimiter {
                let r = self.inner.list_with_delimiter(dir.as_ref()).await.map_err(map_err)?;
                for m in &r.objects {
                    all.push(meta(m)?);
                }
                for p in r.common_prefixes {
                    // Placeholder child so paginate rolls it into a prefix.
                    all.push(Meta {
                        key: Key::new(format!("{p}/\u{10FFFF}"))?,
                        size: 0,
                        modified_ms: None,
                        etag: None,
                        content_type: None,
                    });
                }
            } else {
                let mut stream = match &opts.cursor {
                    Some(c) => self.inner.list_with_offset(dir.as_ref(), &OsPath::from(c.as_str())),
                    None => self.inner.list(dir.as_ref()),
                };
                while let Some(m) = stream.try_next().await.map_err(map_err)? {
                    all.push(meta(&m)?);
                }
            }
            let mut page = paginate(all, &opts);
            page.objects.retain(|m| !m.key.as_str().ends_with('\u{10FFFF}'));
            Ok(page)
        })
    }

    fn copy<'a>(&'a self, from: &'a Key, to: &'a Key, opts: PutOptions) -> StoreFuture<'a, Meta> {
        Box::pin(async move {
            if !self.caps.writable {
                return Err(StoreError::PermissionDenied(format!("{to}: store is read-only")));
            }
            let r = match opts.precondition {
                Precondition::IfNotExists => self.inner.copy_if_not_exists(&path(from)?, &path(to)?).await,
                Precondition::None => self.inner.copy(&path(from)?, &path(to)?).await,
                Precondition::IfMatch(_) => {
                    let obj =
                        self.get(from, GetOptions::default()).await?.ok_or_else(|| StoreError::NotFound(from.to_string()))?;
                    return self.put(to, obj.body, opts).await;
                }
            };
            r.map_err(map_err)?;
            self.head(to).await?.ok_or_else(|| StoreError::NotFound(to.to_string()))
        })
    }
}
