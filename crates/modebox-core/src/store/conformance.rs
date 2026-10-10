//! The executable contract for [`Store`] implementations.
//!
//! ```ignore
//! modebox_core::store::conformance::run(&my_store).await.unwrap();
//! ```
//!
//! Everything happens under a fresh random prefix, and the suite cleans up
//! after itself. Read-only stores get the read-side checks only (seed them via
//! [`run_read_only`]).

use super::*;

macro_rules! ensure {
    ($cond:expr, $($msg:tt)+) => {
        if !$cond {
            return Err(format!($($msg)+));
        }
    };
}

fn k(base: &str, rel: &str) -> Key {
    Key::new(format!("{base}{rel}")).expect("valid test key")
}

async fn put(s: &dyn Store, key: &Key, body: &[u8]) -> Result<Meta, String> {
    s.put(key, Bytes::copy_from_slice(body), PutOptions::default()).await.map_err(|e| format!("put {key}: {e}"))
}

async fn body(s: &dyn Store, key: &Key) -> Result<Option<Vec<u8>>, String> {
    Ok(s.get(key, GetOptions::default()).await.map_err(|e| format!("get {key}: {e}"))?.map(|o| o.body.to_vec()))
}

/// Run the full suite against a writable store.
pub async fn run(store: &dyn Store) -> Result<(), String> {
    ensure!(store.capabilities().writable, "run() needs a writable store; use run_read_only()");
    let base = format!("conformance-{:08x}/", rand::random::<u32>());
    let r = suite(store, &base).await;
    // Best-effort cleanup, even after a failure.
    if let Ok(all) = list_all(store, &base, 10_000).await {
        for m in all {
            let _ = store.delete(&m.key, None).await;
        }
    }
    r
}

async fn suite(s: &dyn Store, base: &str) -> Result<(), String> {
    // -- absent objects
    let missing = k(base, "missing.txt");
    ensure!(s.head(&missing).await.map_err(|e| e.to_string())?.is_none(), "head of a missing key must be None");
    ensure!(body(s, &missing).await?.is_none(), "get of a missing key must be None");
    ensure!(!s.delete(&missing, None).await.map_err(|e| e.to_string())?, "deleting a missing key must return false");

    // -- round trip, metadata, binary and empty bodies, unicode keys
    let a = k(base, "dir/a.txt");
    let m1 = put(s, &a, b"hello world").await?;
    ensure!(m1.size == 11, "put meta size {} != 11", m1.size);
    ensure!(m1.key == a, "put meta key {:?} != {a:?}", m1.key);
    ensure!(m1.etag.is_some(), "put must return an etag");
    ensure!(body(s, &a).await?.as_deref() == Some(&b"hello world"[..]), "get must return what was put");
    let h = s.head(&a).await.map_err(|e| e.to_string())?.ok_or("head after put")?;
    ensure!(h.size == 11 && h.etag == m1.etag, "head must agree with put (size/etag)");
    let bin = k(base, "bin/raw.bin");
    let bytes: Vec<u8> = (0..=255u8).chain([0, 0xff, 0xfe]).collect();
    put(s, &bin, &bytes).await?;
    ensure!(body(s, &bin).await?.as_deref() == Some(&bytes[..]), "binary content must round-trip exactly");
    let empty = k(base, "empty");
    put(s, &empty, b"").await?;
    ensure!(body(s, &empty).await?.as_deref() == Some(&b""[..]), "empty objects must round-trip");
    let uni = k(base, "dir/naïve ünïcode file.txt");
    put(s, &uni, "héllo".as_bytes()).await?;
    ensure!(body(s, &uni).await?.as_deref() == Some("héllo".as_bytes()), "unicode keys must work");

    // -- overwrite changes the etag (different content)
    let m2 = put(s, &a, b"hello again").await?;
    ensure!(m2.etag != m1.etag, "a write with different content must change the etag");

    // -- ranges
    if s.capabilities().range_reads {
        let o = s
            .get(&a, GetOptions { range: Some(2..7), if_match: None })
            .await
            .map_err(|e| e.to_string())?
            .ok_or("ranged get")?;
        ensure!(&o.body[..] == b"llo a", "range 2..7 must return bytes 2-6, got {:?}", String::from_utf8_lossy(&o.body));
        let o = s.get(&a, GetOptions { range: Some(6..1000), if_match: None }).await.map_err(|e| e.to_string())?.unwrap();
        ensure!(&o.body[..] == b"again", "range end must clamp to the size");
    }

    // -- preconditions
    let r = s.put(&a, Bytes::from_static(b"x"), PutOptions { precondition: Precondition::IfNotExists, ..Default::default() }).await;
    ensure!(matches!(r, Err(StoreError::AlreadyExists(_))), "IfNotExists on an existing key must fail with AlreadyExists, got {r:?}");
    let fresh = k(base, "fresh.txt");
    s.put(&fresh, Bytes::from_static(b"new"), PutOptions { precondition: Precondition::IfNotExists, ..Default::default() })
        .await
        .map_err(|e| format!("IfNotExists on an absent key must succeed: {e}"))?;
    let stale = m1.etag.clone().unwrap();
    let r = s
        .put(&a, Bytes::from_static(b"lost update"), PutOptions { precondition: Precondition::IfMatch(stale.clone()), ..Default::default() })
        .await;
    ensure!(matches!(r, Err(StoreError::PreconditionFailed(_))), "IfMatch with a stale etag must fail, got {r:?}");
    ensure!(body(s, &a).await?.as_deref() == Some(&b"hello again"[..]), "a failed conditional write must not change the object");
    let cur = s.head(&a).await.map_err(|e| e.to_string())?.unwrap().etag.unwrap();
    let m3 = s
        .put(&a, Bytes::from_static(b"third"), PutOptions { precondition: Precondition::IfMatch(cur), ..Default::default() })
        .await
        .map_err(|e| format!("IfMatch with the current etag must succeed: {e}"))?;
    let r = s.get(&a, GetOptions { range: None, if_match: Some(stale.clone()) }).await;
    ensure!(matches!(r, Err(StoreError::PreconditionFailed(_))), "get with a stale if_match must fail, got {r:?}");
    let r = s.delete(&a, Some(stale)).await;
    ensure!(matches!(r, Err(StoreError::PreconditionFailed(_))), "delete with a stale if_match must fail, got {r:?}");
    ensure!(s.head(&a).await.map_err(|e| e.to_string())?.is_some(), "a failed conditional delete must keep the object");

    // -- copy
    let c = k(base, "dir/copy.txt");
    s.copy(&a, &c, PutOptions::default()).await.map_err(|e| format!("copy: {e}"))?;
    ensure!(body(s, &c).await?.as_deref() == Some(&b"third"[..]), "copy must duplicate content");
    let r = s.copy(&a, &c, PutOptions { precondition: Precondition::IfNotExists, ..Default::default() }).await;
    ensure!(matches!(r, Err(StoreError::AlreadyExists(_))), "copy with IfNotExists onto an existing key must fail, got {r:?}");

    // -- listing: raw prefixes, sorting, delimiters, pagination
    for name in ["list/b.txt", "list/a.txt", "list/sub/c.txt", "list/sub/deep/d.txt", "list2.txt"] {
        put(s, &k(base, name), name.as_bytes()).await?;
    }
    let keys = |p: &ListPage| p.objects.iter().map(|m| m.key.as_str()[base.len()..].to_owned()).collect::<Vec<_>>();
    let all = s.list(ListOptions { prefix: format!("{base}list"), ..Default::default() }).await.map_err(|e| e.to_string())?;
    ensure!(
        keys(&all) == ["list/a.txt", "list/b.txt", "list/sub/c.txt", "list/sub/deep/d.txt", "list2.txt"],
        "list must match raw prefixes and sort by key, got {:?}",
        keys(&all)
    );
    let one = s
        .list(ListOptions { prefix: format!("{base}list/"), delimiter: true, ..Default::default() })
        .await
        .map_err(|e| e.to_string())?;
    ensure!(keys(&one) == ["list/a.txt", "list/b.txt"], "delimiter listing objects, got {:?}", keys(&one));
    ensure!(one.prefixes == [format!("{base}list/sub/")], "delimiter listing prefixes, got {:?}", one.prefixes);
    let partial = s.list(ListOptions { prefix: format!("{base}list/s"), ..Default::default() }).await.map_err(|e| e.to_string())?;
    ensure!(keys(&partial) == ["list/sub/c.txt", "list/sub/deep/d.txt"], "partial-segment prefix, got {:?}", keys(&partial));
    let mut paged = Vec::new();
    let mut cursor = None;
    for _ in 0..10 {
        let p = s
            .list(ListOptions { prefix: format!("{base}list"), limit: Some(2), cursor: cursor.clone(), ..Default::default() })
            .await
            .map_err(|e| e.to_string())?;
        ensure!(p.objects.len() <= 2, "a page must respect the limit");
        paged.extend(keys(&p));
        match p.cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    ensure!(paged == keys(&all), "pagination must visit every key once in order, got {paged:?}");

    // -- deletes are visible
    ensure!(s.delete(&c, None).await.map_err(|e| e.to_string())?, "delete of an existing key must return true");
    ensure!(s.head(&c).await.map_err(|e| e.to_string())?.is_none(), "deleted keys must be gone");
    if s.capabilities().consistent_list {
        let after = s.list(ListOptions { prefix: format!("{base}dir/"), ..Default::default() }).await.map_err(|e| e.to_string())?;
        ensure!(!keys(&after).contains(&"dir/copy.txt".to_owned()), "list must not show deleted keys");
    }
    let _ = m3;
    Ok(())
}

/// Read-side checks for a read-only store that contains `seeded` (key → body).
pub async fn run_read_only(store: &dyn Store, seeded: &[(&str, &[u8])]) -> Result<(), String> {
    for (key, expected) in seeded {
        let key = Key::new(*key).map_err(|e| e.to_string())?;
        ensure!(body(store, &key).await?.as_deref() == Some(*expected), "{key}: content mismatch");
    }
    let key = Key::new(seeded.first().map(|s| s.0).unwrap_or("x")).map_err(|e| e.to_string())?;
    let r = store.put(&key, Bytes::from_static(b"nope"), PutOptions::default()).await;
    ensure!(matches!(r, Err(StoreError::PermissionDenied(_))), "writes to a read-only store must fail with PermissionDenied, got {r:?}");
    Ok(())
}
