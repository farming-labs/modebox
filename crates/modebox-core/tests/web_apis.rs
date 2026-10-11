//! Web APIs the guest provides beyond the language: URL and URLSearchParams.

use std::sync::OnceLock;

use modebox_core::*;

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine::new().expect("engine"))
}

fn echo() -> Toolbox {
    let mut t = Toolbox::new();
    t.func("echo", |c: Call| async move { Ok(c.input().clone()) });
    t
}

async fn eval(code: &str) -> serde_json::Value {
    let r = engine().run(code, &echo(), RunOptions::default()).await;
    match &r.outcome {
        Outcome::Completed(v) => v.to_json(),
        other => panic!("{code}: {other:?}"),
    }
}

#[tokio::test]
async fn urls_parse_and_resolve_like_the_standard() {
    let v = eval(
        r#"const u = new URL("../b/c?x=1#f", "https://user:pw@EXAMPLE.com:8443/a/d");
           [u.href, u.origin, u.protocol, u.username, u.password, u.host, u.hostname, u.port, u.pathname, u.search, u.hash]"#,
    )
    .await;
    assert_eq!(
        v,
        serde_json::json!([
            "https://user:pw@example.com:8443/b/c?x=1#f", "https://example.com:8443", "https:", "user", "pw",
            "example.com:8443", "example.com", "8443", "/b/c", "?x=1", "#f"
        ])
    );
    let v = eval(r#"[new URL("https://bücher.de/").hostname, new URL("HTTPS://a.b:443/%7e").href, URL.canParse("nope"), URL.parse("::")]"#).await;
    assert_eq!(v, serde_json::json!(["xn--bcher-kva.de", "https://a.b/%7e", false, null]));
    let v = eval(r#"try { new URL("/relative") } catch (e) { [e.name, e.message] }"#).await;
    assert_eq!(v, serde_json::json!(["TypeError", "Invalid URL: /relative"]));
}

#[tokio::test]
async fn setters_apply_or_ignore_like_the_standard() {
    let v = eval(
        r#"const u = new URL("https://a.b/x");
           u.protocol = "http"; u.port = "99999"; u.pathname = "/y z"; u.hash = "h"; u.hostname = "c.d";
           const before = u.href; u.port = "8080"; [before, u.href]"#,
    )
    .await;
    assert_eq!(v, serde_json::json!(["http://c.d/y%20z#h", "http://c.d:8080/y%20z#h"]));
}

#[tokio::test]
async fn search_params_stay_in_sync_with_their_url() {
    let v = eval(
        r#"const u = new URL("https://a.b/?x=1&y=two");
           const p = u.searchParams; p.append("z", "ä&="); p.set("x", "9"); p.delete("y");
           const after = u.href; u.search = "?q=new";
           [after, [...p], p.size]"#,
    )
    .await;
    assert_eq!(v, serde_json::json!(["https://a.b/?x=9&z=%C3%A4%26%3D", [["q", "new"]], 1]));
    let v = eval(
        r#"const s = new URLSearchParams({ b: "x y", a: "1" }); s.append("a", "0"); s.sort();
           [s.toString(), s.getAll("a"), new URLSearchParams("?a=%zz+x&b").get("a"), new URLSearchParams([["k", "v"]]).has("k", "v")]"#,
    )
    .await;
    assert_eq!(v, serde_json::json!(["a=1&a=0&b=x+y", ["1", "0"], "%zz x", true]));
}

#[tokio::test]
async fn urls_cross_the_boundary_as_strings() {
    let v = eval(r#"await echo({ u: new URL("https://a.b/c?d=1"), q: new URLSearchParams("x=1&y=2") })"#).await;
    assert_eq!(v, serde_json::json!({ "u": "https://a.b/c?d=1", "q": "x=1&y=2" }));
    let r = engine().run(r#"console.log(new URL("https://a.b/")); 1"#, &echo(), RunOptions::default()).await;
    assert_eq!(r.logs[0].message, "URL { 'https://a.b/' }");
}

#[tokio::test]
async fn oversized_urls_are_refused() {
    let v = eval(r#"try { new URL("https://a.b/" + "x".repeat(3 << 20)) } catch (e) { e.name }"#).await;
    assert_eq!(v, serde_json::json!("RangeError"));
    // An unpaired surrogate is encoded as U+FFFD, not refused.
    let v = eval(r#"new URL("https://a.b/\uD800").pathname"#).await;
    assert_eq!(v, serde_json::json!("/%EF%BF%BD"));
}
