//! Hostile values from either side of the boundary. Whatever the script
//! returns, logs, throws or receives, the guest must not crash: the result is
//! a value or an ordinary error, never `ErrorKind::Internal` (a wasm trap,
//! which loses the run).

use std::sync::OnceLock;

use modebox_core::*;

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine::new().expect("engine"))
}

/// Tools that hand the script awkward values.
fn tools() -> Toolbox {
    let mut t = Toolbox::new();
    // ~6 MB of JSON that becomes two million JS objects: more than the
    // default 64 MiB heap holds.
    t.func("many_objects", |_c: Call| async {
        Ok(Value::Array((0..2_000_000).map(|_| Value::object(Vec::<(String, Value)>::new())).collect()))
    });
    t.func("proto_key", |_c: Call| async {
        Ok(Value::from(serde_json::json!({ "__proto__": { "polluted": true }, "a": 1 })))
    });
    t.func("list", |_c: Call| async { Ok(Value::from(serde_json::json!([1, 2, 3]))) });
    t.func("fails_with_big_data", |_c: Call| async {
        Err(ToolError::new("nope").with_data(Value::from("x".repeat(4 << 20))))
    });
    t
}

async fn run(code: &str) -> RunResult {
    let r = engine().run(code, &tools(), RunOptions::default()).await;
    if let Outcome::Failed(e) = &r.outcome {
        assert_ne!(e.kind, ErrorKind::Internal, "the guest crashed on {code:?}: {e:?}");
    }
    r
}

fn ok(r: &RunResult) -> serde_json::Value {
    match &r.outcome {
        Outcome::Completed(v) => v.to_json(),
        other => panic!("expected completion, got {other:?}"),
    }
}

fn err(r: &RunResult) -> &RunError {
    match &r.outcome {
        Outcome::Failed(e) => e,
        other => panic!("expected failure, got {other:?}"),
    }
}

#[tokio::test]
async fn maps_and_sets_encode_their_real_contents() {
    // A replaced iterator, on the value or on the prototype, doesn't change what crosses.
    let r = run("const m = new Map([[1, 2]]); m[Symbol.iterator] = function* () { yield 5; }; m").await;
    assert_eq!(ok(&r), serde_json::json!([[1, 2]]));
    let r = run("Map.prototype[Symbol.iterator] = function* () { throw 1; }; Set.prototype[Symbol.iterator] = function* () {}; [new Map([['k', 'v']]), new Set([7])]").await;
    assert_eq!(ok(&r), serde_json::json!([{ "k": "v" }, [7]])); // to_json shows string-keyed Maps as objects
    let r = run("const m = new Map([[1, 2]]); m[Symbol.iterator] = function* () { yield 5; }; console.log(m); 1").await;
    assert_eq!(r.logs[0].message, "Map(1) { 1 => 2 }");
}

#[tokio::test]
async fn huge_array_lengths_are_errors() {
    for n in ["2 ** 31", "2 ** 32 - 1"] {
        let r = run(&format!("const a = []; a.length = {n}; a")).await;
        assert_eq!(err(&r).kind, ErrorKind::Unserializable, "{n}");
        let r = run(&format!("const a = []; a.length = {n}; console.log(a); a.length")).await;
        assert!(r.logs[0].message.contains("more items"), "{n}: {:?}", r.logs);
    }
}

#[tokio::test]
async fn oversized_results_are_errors() {
    let r = run("'x'.repeat(50 << 20)").await;
    assert_eq!(err(&r).kind, ErrorKind::Unserializable);
    assert!(err(&r).message.contains("byte limit"), "{}", err(&r).message);
    // Too big for memory before the byte limit is reached.
    let r = run("new Array(3e6).fill(undefined)").await;
    assert_eq!(err(&r).kind, ErrorKind::Unserializable);
    let r = run("try { structuredClone('x'.repeat(50 << 20)); 'cloned' } catch (e) { e.message.slice(0, 14) }").await;
    assert_eq!(ok(&r), serde_json::json!("DataCloneError"));
}

#[tokio::test]
async fn huge_logs_are_cut() {
    let r = run("console.log('x'.repeat(50 << 20)); 1").await;
    assert!(r.logs[0].message.len() < 20_000 && r.logs[0].message.ends_with("(truncated)"));
    let r = run("console.log(...Array(5000).fill('y'.repeat(9000))); 1").await;
    assert!(r.logs[0].message.len() < 100_000, "{}", r.logs[0].message.len());
    let r = run("console.log(Array(100).fill(Array(100).fill('z'.repeat(9000)))); 1").await;
    assert!(r.logs[0].message.len() < 20_000, "{}", r.logs[0].message.len());
}

#[tokio::test]
async fn huge_errors_are_cut() {
    let r = run("throw new Error('m'.repeat(50 << 20))").await;
    let e = err(&r);
    assert_eq!(e.kind, ErrorKind::Uncaught);
    assert!(e.message.len() < (2 << 20) && e.message.ends_with("(truncated)"));
}

#[tokio::test]
async fn invalid_strings_are_errors() {
    let r = run("({ ['\\uD800']: 1 })").await;
    assert_eq!(err(&r).kind, ErrorKind::Unserializable);
    let r = run("'a\\uD800b'").await;
    assert_eq!(err(&r).kind, ErrorKind::Unserializable);
    let r = run("Array.from(new TextEncoder().encode('a\\uD800b'))").await;
    // One U+FFFD per unpaired surrogate, as TextEncoder specifies.
    assert_eq!(ok(&r), serde_json::json!([97, 0xef, 0xbf, 0xbd, 98]));
    let r = run("console.log('a\\uD800b'); 1").await;
    assert_eq!(r.logs[0].message, "a\u{FFFD}b");
}

#[tokio::test]
async fn string_natives_survive_huge_input() {
    for code in [
        "new TextEncoder().encode('x'.repeat(40 << 20)).length",
        "new TextDecoder().decode(new Uint8Array(30 << 20).fill(0xff)).length",
        "atob('QUFB'.repeat(10 << 20)).length",
        "btoa('x'.repeat(30 << 20)).length",
    ] {
        let r = run(&format!("try {{ {code} }} catch (e) {{ e.name }}")).await;
        match ok(&r) {
            serde_json::Value::Number(_) => {}
            serde_json::Value::String(name) => assert!(name == "RangeError" || name == "InternalError", "{code}: {name}"),
            other => panic!("{code}: {other}"),
        }
    }
}

#[tokio::test]
async fn results_too_big_for_memory_reject_the_call() {
    let r = run("try { (await many_objects()).length } catch (e) { e.name }").await;
    match ok(&r) {
        serde_json::Value::Number(n) => assert_eq!(n.as_u64(), Some(2_000_000)),
        other => assert_eq!(other, serde_json::json!("RangeError")),
    }
    let r = run("try { await fails_with_big_data() } catch (e) { [e.message, e.data.length] }").await;
    assert_eq!(ok(&r), serde_json::json!(["nope", 4 << 20]));
}

#[tokio::test]
async fn host_results_are_defined_not_assigned() {
    // `__proto__` from the host is an own key, not a prototype change.
    let r = run("const r = await proto_key(); [Object.getPrototypeOf(r) === Object.prototype, Object.keys(r), r.polluted]").await;
    assert_eq!(ok(&r), serde_json::json!([true, ["__proto__", "a"], null]));
    // Setters on Array.prototype don't intercept decoded elements.
    let r = run(
        "let hit = false; Object.defineProperty(Array.prototype, '0', { set() { hit = true }, configurable: true });\n\
         const r = await list(); delete Array.prototype[0]; [r, hit]",
    )
    .await;
    assert_eq!(ok(&r), serde_json::json!([[1, 2, 3], false]));
}

#[tokio::test]
async fn tiny_memory_limits_fail_cleanly() {
    let mut opts = RunOptions::default();
    opts.limits.memory_bytes = 64 << 10;
    let r = engine().run("const xs = []; for (let i = 0; i < 1e5; i++) xs.push({ i }); xs.length", &tools(), opts).await;
    let e = err(&r);
    assert_eq!(e.kind, ErrorKind::MemoryLimit, "{e:?}");
}
