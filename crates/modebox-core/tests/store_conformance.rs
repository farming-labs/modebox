//! Every backend must pass the same contract.
use std::path::PathBuf;

use modebox_core::store::conformance::{run, run_read_only};
use modebox_core::store::local::{LocalOptions, LocalStore};
use modebox_core::store::memory::MemoryStore;
use modebox_core::store::objstore::ObjectStoreAdapter;
use modebox_core::store::Store;

fn dir(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("conf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_store() {
    run(&MemoryStore::new()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn local_store() {
    // Listing must see everything the suite writes, so no gitignore/hidden filtering here.
    let opts = LocalOptions { respect_gitignore: false, skip_hidden: false, ..LocalOptions::default() };
    run(&LocalStore::new(dir("local"), opts).unwrap()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn object_store_memory_adapter() {
    let s = ObjectStoreAdapter::from_url("memory://", std::iter::empty::<(&str, &str)>(), false).unwrap();
    run(&s).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn object_store_local_fs_adapter() {
    let d = dir("osfs");
    let url = format!("file://{}", d.canonicalize().unwrap().display());
    let s = ObjectStoreAdapter::from_url(&url, std::iter::empty::<(&str, &str)>(), false).unwrap();
    run(&s).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_stores_refuse_writes() {
    let m = MemoryStore::new();
    m.insert("docs/a.md", "# A").unwrap();
    let m = {
        let ro = MemoryStore::new().read_only();
        ro.insert("docs/a.md", "# A").unwrap();
        ro
    };
    run_read_only(&m, &[("docs/a.md", b"# A")]).await.unwrap();

    let d = dir("ro");
    std::fs::create_dir_all(d.join("docs")).unwrap();
    std::fs::write(d.join("docs/a.md"), "# A").unwrap();
    let l = LocalStore::new(&d, LocalOptions { writable: false, ..LocalOptions::default() }).unwrap();
    run_read_only(&l, &[("docs/a.md", b"# A")]).await.unwrap();
    assert!(!l.capabilities().writable);
}
