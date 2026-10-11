#![cfg(unix)]
//! Local-directory specifics beyond the conformance suite: symlinks, special
//! files and where writes land.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use modebox_core::store::local::{LocalOptions, LocalStore};
use modebox_core::store::{Bytes, GetOptions, Key, PutOptions, Store, StoreError};

fn root(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("local-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn store(dir: &PathBuf) -> LocalStore {
    LocalStore::new(dir, LocalOptions { respect_gitignore: false, skip_hidden: false, ..LocalOptions::default() }).unwrap()
}

fn key(s: &str) -> Key {
    Key::new(s).unwrap()
}

#[tokio::test]
async fn symlinks_inside_the_root_work_and_writes_replace_the_link() {
    let dir = root("links");
    std::fs::write(dir.join("target.txt"), "T").unwrap();
    std::os::unix::fs::symlink(dir.join("target.txt"), dir.join("alias.txt")).unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::os::unix::fs::symlink(dir.join("sub"), dir.join("subalias")).unwrap();
    let s = store(&dir);

    let got = s.get(&key("alias.txt"), GetOptions::default()).await.unwrap().unwrap();
    assert_eq!(&got.body[..], b"T");
    s.put(&key("subalias/new.txt"), Bytes::from_static(b"N"), PutOptions::default()).await.unwrap();
    assert_eq!(std::fs::read(dir.join("sub/new.txt")).unwrap(), b"N");

    // Writing to the link replaces the link, and leaves its target alone.
    s.put(&key("alias.txt"), Bytes::from_static(b"A"), PutOptions::default()).await.unwrap();
    assert_eq!(std::fs::read(dir.join("target.txt")).unwrap(), b"T");
    assert!(!std::fs::symlink_metadata(dir.join("alias.txt")).unwrap().file_type().is_symlink());
    assert!(s.delete(&key("alias.txt"), None).await.unwrap());
    assert!(dir.join("target.txt").exists());
}

#[tokio::test]
async fn symlinks_out_of_the_root_are_refused() {
    let dir = root("escape");
    let outside = root("escape-outside");
    std::fs::write(outside.join("secret"), "S").unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("out")).unwrap();
    let s = store(&dir);
    let r = s.get(&key("out/secret"), GetOptions::default()).await;
    assert!(matches!(r, Err(StoreError::PermissionDenied(_))), "{r:?}");
    let w = s.put(&key("out/new"), Bytes::from_static(b"x"), PutOptions::default()).await;
    assert!(matches!(w, Err(StoreError::PermissionDenied(_))), "{w:?}");
    assert!(!outside.join("new").exists());
}

#[tokio::test]
async fn a_fifo_does_not_hang_reads() {
    let dir = root("fifo");
    let path = std::ffi::CString::new(dir.join("pipe").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let s = store(&dir);
    let t = Instant::now();
    assert!(s.head(&key("pipe")).await.unwrap().is_none());
    assert!(s.get(&key("pipe"), GetOptions::default()).await.unwrap().is_none());
    assert!(t.elapsed() < Duration::from_secs(2));
}
