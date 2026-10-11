#![cfg(unix)]
//! `sh` with network limited to some hosts: through the proxy on macOS, and
//! through the proxy plus the in-sandbox bridge on Linux.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

/// A tiny HTTP server on loopback answering every request with "ok".
fn serve() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { continue };
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf);
            let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    port
}

fn sh(args: &[&str], command: &str) -> Option<String> {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("net-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_modebox"))
        .env("MODEBOX_SECRET_FILE", root.join("secret"))
        .args(["sh", "--root", root.to_str().unwrap()])
        .args(args)
        .args(["--", command])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("no OS sandbox is available") {
        assert!(std::env::var_os("MODEBOX_REQUIRE_SANDBOX").is_none(), "{stderr}");
        eprintln!("skipping: {stderr}");
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn only_listed_hosts_are_reachable() {
    let port = serve();
    let get = |host: &str| format!("curl -s -m 5 http://{host}:{port}/; echo \" exit=$?\"");
    let Some(allowed) = sh(&["--allow-host", "127.0.0.1"], &get("127.0.0.1")) else { return };
    assert_eq!(allowed.trim(), "ok exit=0");

    // Same server, under a name that isn't listed.
    let other = sh(&["--allow-host", "127.0.0.1"], &get("localhost")).unwrap();
    assert!(other.contains("isn't on the network allow list"), "{other}");

    // A wildcard doesn't reach loopback addresses.
    let wildcard = sh(&["--allow-host", "*"], &get("127.0.0.1")).unwrap();
    assert!(wildcard.contains("non-public address"), "{wildcard}");

    // Nothing listed: no network at all.
    let none = sh(&[], &get("127.0.0.1")).unwrap();
    assert!(!none.contains("ok exit=0"), "{none}");
}
