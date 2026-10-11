#![cfg(feature = "fetch")]
//! `fetch()` from sandboxed code, against a local test server.

use std::sync::OnceLock;

use modebox_core::fetch::{FetchPolicy, fetch_tool};
use modebox_core::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine::new().expect("engine"))
}

/// A tiny HTTP/1.1 server on loopback. Paths: `/hello`, `/echo` (method and
/// body back), `/redirect-in` (to `/hello`), `/redirect-out` (to the same
/// server as `localhost`), `/big` (2 MiB).
async fn server() -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    let n = c.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    let n = c.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let mut parts = head.split(' ');
                let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                let (status, extra, out) = match path {
                    "/hello" => ("200 OK", "X-Test: yes\r\n".to_owned(), "hello".to_owned()),
                    "/echo" => ("200 OK", String::new(), format!("{method} {body}")),
                    "/redirect-in" => ("302 Found", "Location: /hello\r\n".to_owned(), String::new()),
                    "/redirect-out" => ("302 Found", format!("Location: http://localhost:{port}/hello\r\n"), String::new()),
                    "/big" => ("200 OK", String::new(), "x".repeat(2 << 20)),
                    _ => ("404 Not Found", String::new(), String::new()),
                };
                let resp = format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
                let _ = c.write_all(resp.as_bytes()).await;
            });
        }
    });
    port
}

fn tools() -> Toolbox {
    let policy = FetchPolicy { allow: vec!["127.0.0.1".into()], max_response_bytes: 1 << 20, ..Default::default() };
    Toolbox::new().with(fetch_tool(policy))
}

async fn eval(code: &str) -> serde_json::Value {
    let r = engine().run(code, &tools(), RunOptions::default()).await;
    match &r.outcome {
        Outcome::Completed(v) => v.to_json(),
        other => panic!("{code}: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fetches_allowed_hosts() {
    let port = server().await;
    let v = eval(&format!(
        r#"const r = await fetch("http://127.0.0.1:{port}/hello");
           [r.status, r.ok, r.statusText, r.headers.get("x-test"), await r.text(), r.redirected, r.bodyUsed]"#
    ))
    .await;
    assert_eq!(v, serde_json::json!([200, true, "OK", "yes", "hello", false, true]));
    let v = eval(&format!(
        r#"const r = await fetch(new URL("/echo", "http://127.0.0.1:{port}"), {{ method: "post", body: JSON.stringify({{ a: 1 }}) }});
           await r.text()"#
    ))
    .await;
    assert_eq!(v, serde_json::json!(r#"POST {"a":1}"#));
}

#[tokio::test(flavor = "multi_thread")]
async fn redirects_stay_in_reach() {
    let port = server().await;
    let v = eval(&format!(r#"const r = await fetch("http://127.0.0.1:{port}/redirect-in"); [r.status, r.redirected, await r.text()]"#)).await;
    assert_eq!(v, serde_json::json!([200, true, "hello"]));
    let v = eval(&format!(r#"try {{ await fetch("http://127.0.0.1:{port}/redirect-out") }} catch (e) {{ [e.name, e.message] }}"#)).await;
    assert_eq!(v[0], "TypeError");
    assert!(v[1].as_str().unwrap().contains("isn't on the allow list"), "{v}");
    let v = eval(&format!(r#"(await fetch("http://127.0.0.1:{port}/redirect-in", {{ redirect: "manual" }})).status"#)).await;
    assert_eq!(v, serde_json::json!(302));
}

#[tokio::test(flavor = "multi_thread")]
async fn responses_are_capped() {
    let port = server().await;
    let v = eval(&format!(r#"try {{ await fetch("http://127.0.0.1:{port}/big") }} catch (e) {{ e.message }}"#)).await;
    assert!(v.as_str().unwrap().contains("larger than"), "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn other_hosts_need_approval_and_stay_public() {
    let port = server().await;
    let code = format!(r#"try {{ await fetch("http://localhost:{port}/hello") }} catch (e) {{ e.message }}"#);
    let r = engine().run(&code, &tools(), RunOptions::default()).await;
    let s = r.suspension().expect("asks first").clone();
    assert_eq!(s.interrupts[0].tool, "$fetch");
    // Approved, but `localhost` is loopback, which only an exact allow-list entry may reach.
    let ids = s.interrupts.iter().map(|i| (i.id, Resolution::Approve));
    let r = engine().resume(&s.continuation, ids, &tools(), RunOptions::default()).await;
    let msg = r.value().and_then(Value::as_str).unwrap_or_default().to_owned();
    assert!(msg.contains("no public address"), "{msg}");
}

#[tokio::test]
async fn without_the_tool_there_is_no_fetch() {
    let r = engine().run("[typeof fetch, typeof $fetch, typeof Response]", &Toolbox::new(), RunOptions::default()).await;
    assert_eq!(r.value().map(Value::to_json), Some(serde_json::json!(["undefined", "undefined", "function"])));
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_is_a_sink_for_data_flow() {
    let port = server().await;
    let mut t = tools();
    t.add(Tool::new("secrets.read", |_c: Call| async { Ok(Value::from("s3cret")) }).source());
    let code = format!(r#"await fetch("http://127.0.0.1:{port}/hello"); const s = await secrets.read(); await fetch("http://127.0.0.1:{port}/echo?" + s)"#);
    let r = engine().run(&code, &t, RunOptions::default()).await;
    let s = r.suspension().expect("a fetch after reading a secret asks first");
    let reason = s.interrupts[0].payload.get("reason").and_then(Value::as_str).unwrap_or_default().to_owned();
    assert!(reason.contains("secrets.read"), "{reason}");
}
