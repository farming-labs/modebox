use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use modebox_core::*;

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine::new().expect("engine"))
}

fn opts() -> RunOptions {
    RunOptions::default()
}

fn math() -> Toolbox {
    let mut t = Toolbox::new();
    t.func("math.double", |c: Call| async move { Ok(Value::from(c.input().as_f64().unwrap_or(0.0) * 2.0)) });
    t.func("math.add", |c: Call| async move {
        Ok(Value::from(c.args.iter().filter_map(Value::as_f64).sum::<f64>()))
    });
    t
}

async fn run(code: &str) -> RunResult {
    engine().run(code, &math(), opts()).await
}

fn ok(r: &RunResult) -> &Value {
    match &r.outcome {
        Outcome::Completed(v) => v,
        other => panic!("expected completion, got {other:?}\nlogs: {:?}", r.logs),
    }
}

fn err(r: &RunResult) -> &RunError {
    match &r.outcome {
        Outcome::Failed(e) => e,
        other => panic!("expected failure, got {other:?}"),
    }
}

// ---- evaluation ----------------------------------------------------------------

#[tokio::test]
async fn last_expression_is_the_result() {
    assert_eq!(ok(&run("const a = 20; a + 22").await), &Value::from(42));
}

#[tokio::test]
async fn top_level_return_works() {
    assert_eq!(ok(&run("const a = 40;\nreturn a + 2;").await), &Value::from(42));
}

#[tokio::test]
async fn host_call() {
    assert_eq!(ok(&run("await math.double(21)").await), &Value::from(42));
}

#[tokio::test]
async fn unawaited_trailing_call_is_awaited() {
    assert_eq!(ok(&run("math.double(4)").await), &Value::from(8));
}

#[tokio::test]
async fn positional_args() {
    assert_eq!(ok(&run("await math.add(1, 2, 3)").await), &Value::from(6));
}

#[tokio::test]
async fn typescript_is_stripped_with_positions_kept() {
    let r = run("interface P { x: number }\nenum Color { Red, Green }\nconst p: P = { x: 1 };\nreturn p.x + Color.Green;").await;
    assert_eq!(ok(&r), &Value::from(2));

    // A runtime error on line 3 reports line 3.
    let r = run("const a: number = 1;\nconst b: string = 'x';\nthrow new Error('boom');").await;
    let e = err(&r);
    assert_eq!(e.message, "boom");
    assert!(e.stack.as_deref().unwrap_or("").contains("agent.ts:3"), "stack: {:?}", e.stack);
}

#[tokio::test]
async fn syntax_errors_are_reported_before_running() {
    let r = run("const = ;").await;
    assert_eq!(err(&r).kind, ErrorKind::Syntax);
}

#[tokio::test]
async fn uncaught_errors_keep_name_message_stack() {
    let r = run("function f() { throw new TypeError('bad input') }\nf()").await;
    let e = err(&r);
    assert_eq!(e.kind, ErrorKind::Uncaught);
    assert_eq!(e.name, "TypeError");
    assert_eq!(e.message, "bad input");
    assert!(e.stack.as_deref().unwrap().contains("at f"));
}

#[tokio::test]
async fn tool_errors_are_catchable_and_not_redacted() {
    let mut t = Toolbox::new();
    t.func("db.get", |_c: Call| async move {
        Err(ToolError::new("row 7 not found").with_code("NOT_FOUND"))
    });
    let r = engine()
        .run("try { await db.get(7) } catch (e) { `${e.name}|${e.message}|${e.code}` }", &t, opts())
        .await;
    assert_eq!(ok(&r), &Value::from("Error|row 7 not found|NOT_FOUND"));
}

#[tokio::test]
async fn rich_values_round_trip() {
    let mut t = Toolbox::new();
    t.func("echo", |c: Call| async move { Ok(c.input().clone()) });
    let code = r#"
        const v = {
            date: new Date(0), map: new Map([['k', 1]]), set: new Set([1, 2]), big: 2n ** 70n,
            bytes: new Uint8Array([1, 2, 3]), floats: new Float64Array([1.5]), undef: undefined,
            nan: NaN, inf: -Infinity, negzero: -0, re: /a+/gi, nested: { $t: 'not a tag' },
            err: new RangeError('r'),
        };
        const back = await echo(v);
        [
            back.date instanceof Date && back.date.getTime() === 0,
            back.map instanceof Map && back.map.get('k') === 1,
            back.set instanceof Set && back.set.has(2),
            back.big === 2n ** 70n,
            back.bytes instanceof Uint8Array && back.bytes[2] === 3,
            back.floats instanceof Float64Array && back.floats[0] === 1.5,
            'undef' in back && back.undef === undefined,
            Number.isNaN(back.nan), back.inf === -Infinity, Object.is(back.negzero, -0),
            back.re instanceof RegExp && back.re.flags === 'gi',
            back.nested.$t === 'not a tag',
            back.err instanceof RangeError && back.err.message === 'r',
        ]
    "#;
    let r = engine().run(code, &t, opts()).await;
    let Value::Array(checks) = ok(&r) else { panic!() };
    for (i, c) in checks.iter().enumerate() {
        assert_eq!(c, &Value::Bool(true), "check {i} failed");
    }
}

#[tokio::test]
async fn host_sees_rich_values() {
    let r = run("({ when: new Date(86400000), tags: new Set(['a']), n: 10n })").await;
    let v = ok(&r);
    assert_eq!(v.get("when"), Some(&Value::Date(Some(86_400_000.0))));
    assert_eq!(v.get("tags"), Some(&Value::Set(vec![Value::from("a")])));
    assert_eq!(v.get("n"), Some(&Value::BigInt("10".into())));
}

#[tokio::test]
async fn unserializable_results_explain_where() {
    let r = run("({ items: [1, { handler() {} }] })").await;
    let e = err(&r);
    assert_eq!(e.kind, ErrorKind::Unserializable);
    assert!(e.message.contains(".items[1].handler"), "{}", e.message);
}

#[tokio::test]
async fn console_is_captured_node_style() {
    let r = run("console.log('hi', 1, {a: [1, 'x']}); console.warn(new Map([[1, 2]])); console.error(null)").await;
    ok(&r);
    let lines: Vec<_> = r.logs.iter().map(|l| (l.level.as_str(), l.message.as_str())).collect();
    assert_eq!(
        lines,
        vec![("log", "hi 1 { a: [ 1, 'x' ] }"), ("warn", "Map(1) { 1 => 2 }"), ("error", "null")]
    );
}

#[tokio::test]
async fn web_platform_basics_exist() {
    let r = run(r#"
        const enc = new TextEncoder().encode('héllo');
        [
            enc.length,
            new TextDecoder().decode(enc),
            btoa('hi'), atob('aGk='),
            /^[0-9a-f-]{36}$/.test(crypto.randomUUID()),
            structuredClone({ d: new Date(5) }).d.getTime(),
            typeof setTimeout, typeof queueMicrotask,
        ]
    "#)
    .await;
    assert_eq!(
        ok(&r).to_json(),
        serde_json::json!([6, "héllo", "aGk=", "hi", true, 5, "function", "function"])
    );
}

#[tokio::test]
async fn no_ambient_authority() {
    let r = run(r#"[typeof fetch, typeof require, typeof process, typeof Deno, typeof XMLHttpRequest, typeof WebSocket]"#).await;
    assert_eq!(ok(&r).to_json(), serde_json::json!(["undefined", "undefined", "undefined", "undefined", "undefined", "undefined"]));
    let r = run("await import('fs')").await;
    assert!(matches!(r.outcome, Outcome::Failed(_)));
}

#[tokio::test]
async fn unhandled_rejections_are_logged() {
    let mut t = Toolbox::new();
    t.func("fail", |_c: Call| async move { Err(ToolError::new("nope")) });
    let r = engine().run("fail(); await new Promise(r => setTimeout(r, 1)); 'done'", &t, opts()).await;
    assert_eq!(ok(&r), &Value::from("done"));
    assert!(r.logs.iter().any(|l| l.level == "warn" && l.message.contains("Unhandled promise rejection")), "{:?}", r.logs);
}

// ---- concurrency & timers ---------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn parallel_calls_run_concurrently() {
    let mut t = Toolbox::new();
    t.func("slow", |c: Call| async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(c.input().clone())
    });
    let engine = engine(); // created before timing: the first use compiles the guest
    let start = Instant::now();
    let r = engine.run("(await Promise.all([1,2,3,4,5,6,7,8,9,10].map(i => slow(i)))).length", &t, opts()).await;
    assert_eq!(ok(&r), &Value::from(10));
    assert!(start.elapsed() < Duration::from_millis(400), "took {:?}", start.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrency_over_the_limit_queues_instead_of_failing() {
    let mut t = Toolbox::new();
    let live = Arc::new(AtomicU32::new(0));
    let peak = Arc::new(AtomicU32::new(0));
    let (l, p) = (live.clone(), peak.clone());
    t.func("work", move |_c: Call| {
        let (l, p) = (l.clone(), p.clone());
        async move {
            let n = l.fetch_add(1, Ordering::SeqCst) + 1;
            p.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            l.fetch_sub(1, Ordering::SeqCst);
            Ok(Value::Null)
        }
    });
    let mut o = opts();
    o.limits.max_concurrency = 4;
    let r = engine().run("await Promise.all(Array.from({length: 40}, () => work())); 'ok'", &t, o).await;
    assert_eq!(ok(&r), &Value::from("ok"));
    assert!(peak.load(Ordering::SeqCst) <= 4);
}

#[tokio::test]
async fn timers_fire_in_order_and_clear_works() {
    let r = run(r#"
        const out = [];
        setTimeout(() => out.push('b'), 20);
        setTimeout(() => out.push('a'), 5);
        const t = setTimeout(() => out.push('never'), 10);
        clearTimeout(t);
        await new Promise(r => setTimeout(r, 40));
        out.join('')
    "#)
    .await;
    assert_eq!(ok(&r), &Value::from("ab"));
}

#[tokio::test]
async fn virtual_time_is_instant_and_deterministic() {
    let mut o = opts();
    o.clock = ClockMode::Virtual { start_ms: 1_700_000_000_000.0 };
    let engine = engine();
    let start = Instant::now();
    let r = engine
        .run("const t0 = Date.now(); await new Promise(r => setTimeout(r, 3_600_000)); [t0, Date.now() - t0]", &math(), o)
        .await;
    assert_eq!(ok(&r).to_json(), serde_json::json!([1_700_000_000_000i64, 3_600_000]));
    assert!(start.elapsed() < Duration::from_millis(200));
}

#[tokio::test]
async fn seeded_randomness_is_reproducible() {
    let mut o = opts();
    o.seed = Some([7; 32]);
    let code = "[Math.random(), Math.random(), crypto.randomUUID()]";
    let a = engine().run(code, &math(), o.clone()).await;
    let b = engine().run(code, &math(), o).await;
    assert_eq!(ok(&a), ok(&b));
    let c = run(code).await;
    assert_ne!(ok(&a), ok(&c));
}

#[tokio::test]
async fn deadlocks_are_detected() {
    let r = run("await new Promise(() => {})").await;
    assert_eq!(err(&r).kind, ErrorKind::Deadlock);
}

// ---- limits ----------------------------------------------------------------------

#[tokio::test]
async fn infinite_loop_times_out() {
    let mut o = opts();
    o.limits.timeout = Duration::from_millis(200);
    let engine = engine();
    let start = Instant::now();
    let r = engine.run("while (true) {}", &math(), o).await;
    assert_eq!(err(&r).kind, ErrorKind::Timeout);
    assert!(start.elapsed() < Duration::from_millis(600), "took {:?}", start.elapsed());
}

#[tokio::test]
async fn timeout_also_covers_slow_tools() {
    let mut t = Toolbox::new();
    t.func("hang", |_c: Call| async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(Value::Null)
    });
    let mut o = opts();
    o.limits.timeout = Duration::from_millis(100);
    let r = engine().run("await hang()", &t, o).await;
    assert_eq!(err(&r).kind, ErrorKind::Timeout);
}

#[tokio::test]
async fn fuel_is_deterministic_and_enforced() {
    let code = "let s = 0; for (let i = 0; i < 100000; i++) s += i; s";
    let a = run(code).await;
    let b = run(code).await;
    assert_eq!(ok(&a), &Value::from(4_999_950_000u64));
    let (fa, fb) = (a.stats.fuel_used.unwrap(), b.stats.fuel_used.unwrap());
    assert_eq!(fa, fb, "same code, same fuel");

    let mut o = opts();
    o.limits.fuel = Some(fa / 2);
    let r = engine().run(code, &math(), o).await;
    assert_eq!(err(&r).kind, ErrorKind::CpuLimit);
}

#[tokio::test]
async fn memory_limit_is_a_catchable_error_first() {
    let mut o = opts();
    o.limits.memory_bytes = 8 << 20;
    let r = engine()
        .run("const a = []; try { while (true) a.push('x'.repeat(1 << 16) + a.length) } catch (e) { 'caught: ' + e.message }", &math(), o)
        .await;
    match &r.outcome {
        Outcome::Completed(v) => assert!(v.as_str().unwrap().contains("out of memory"), "{v:?}"),
        Outcome::Failed(e) => assert_eq!(e.kind, ErrorKind::MemoryLimit, "{e}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn deep_recursion_is_a_range_error() {
    let r = run("function f(n) { return f(n + 1) + 1 }\ntry { f(0) } catch (e) { e.constructor.name + ': ' + e.message }").await;
    match &r.outcome {
        Outcome::Completed(v) => assert!(v.as_str().unwrap().starts_with("RangeError"), "{v:?}"),
        Outcome::Failed(e) => assert_eq!(e.kind, ErrorKind::StackOverflow, "{e}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn host_call_limit() {
    let mut o = opts();
    o.limits.max_host_calls = 10;
    let r = engine().run("for (let i = 0; i < 20; i++) await math.double(i)", &math(), o).await;
    assert_eq!(err(&r).kind, ErrorKind::HostCallLimit);
}

#[tokio::test]
async fn big_values_cross_quickly() {
    let mut t = Toolbox::new();
    t.func("blob", |_c: Call| async move { Ok(Value::from("x".repeat(4 << 20))) });
    let engine = engine();
    let start = Instant::now();
    let r = engine.run("const s = await blob(); ({ len: s.length, back: s })", &t, opts()).await;
    assert_eq!(ok(&r).get("len"), Some(&Value::from(4 << 20)));
    // 4 MiB in and 4 MiB out; Run's devalue bridge moves ~1 MB/s.
    assert!(start.elapsed() < Duration::from_secs(1), "took {:?}", start.elapsed());
}

// ---- suspend / resume -----------------------------------------------------------------

fn bank(counter: Arc<AtomicU32>) -> Toolbox {
    let mut t = Toolbox::new();
    t.add(
        Tool::new("bank.transfer", move |c: Call| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(Value::object([("sent", c.input().get("amount").cloned().unwrap_or_default())]))
            }
        })
        .approval(Approval::Always),
    );
    t.func("bank.balance", |_c: Call| async move { Ok(Value::from(100)) });
    t
}

#[tokio::test]
async fn approval_suspends_and_resumes_without_replay() {
    let counter = Arc::new(AtomicU32::new(0));
    let tools = bank(counter.clone());
    let code = r#"
        const before = await bank.balance();
        const nonce = Math.random();
        console.log('before approval');
        const r = await bank.transfer({ amount: 25 });
        console.log('after approval');
        ({ before, sent: r.sent, nonce })
    "#;
    let first = engine().run(code, &tools, opts()).await;
    let s = first.suspension().expect("suspended").clone();
    assert_eq!(s.interrupts.len(), 1);
    assert_eq!(s.interrupts[0].tool, "bank.transfer");
    assert_eq!(s.interrupts[0].kind, "approval");
    assert_eq!(counter.load(Ordering::SeqCst), 0, "not executed before approval");
    assert_eq!(first.logs.len(), 1);

    let second = engine()
        .resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &tools, opts())
        .await;
    let v = ok(&second);
    assert_eq!(v.get("sent"), Some(&Value::from(25)));
    assert_eq!(v.get("before"), Some(&Value::from(100)));
    assert_eq!(counter.load(Ordering::SeqCst), 1, "executed exactly once");
    // No replay: the first log line is not printed again.
    assert_eq!(second.logs.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(), vec!["after approval"]);
    assert_eq!(second.stats.host_calls, 2, "call count carries over");
}

#[tokio::test]
async fn continuations_are_single_use_and_tamper_proof() {
    let tools = bank(Arc::new(AtomicU32::new(0)));
    let first = engine().run("await bank.transfer({ amount: 1 })", &tools, opts()).await;
    let s = first.suspension().unwrap().clone();
    let id = s.interrupts[0].id;

    let mut forged = s.continuation.clone().into_bytes();
    let i = forged.len() / 2;
    forged[i] = if forged[i] == b'a' { b'b' } else { b'a' };
    let r = engine().resume(std::str::from_utf8(&forged).unwrap(), [(id, Resolution::Approve)], &tools, opts()).await;
    assert_eq!(err(&r).kind, ErrorKind::InvalidContinuation);

    ok(&engine().resume(&s.continuation, [(id, Resolution::Approve)], &tools, opts()).await);
    let again = engine().resume(&s.continuation, [(id, Resolution::Approve)], &tools, opts()).await;
    assert_eq!(err(&again).kind, ErrorKind::InvalidContinuation);
}

#[tokio::test]
async fn a_shared_store_makes_tokens_single_use_across_engines() {
    // Two engines with the same key stand in for two processes.
    let used = Arc::new(StoreContinuations::new(Arc::new(store::memory::MemoryStore::new())).unwrap());
    let make = |continuations: Arc<dyn ContinuationStore>| {
        let cfg = EngineConfig { secret: Some(b"shared-secret-for-two-engines-0123".to_vec()), continuations, ..Default::default() };
        Engine::with_config(cfg).unwrap()
    };
    let (a, b) = (make(used.clone()), make(used));
    let tools = bank(Arc::new(AtomicU32::new(0)));
    let first = a.run("await bank.transfer({ amount: 1 })", &tools, opts()).await;
    let s = first.suspension().unwrap().clone();
    let id = s.interrupts[0].id;

    ok(&b.resume(&s.continuation, [(id, Resolution::Approve)], &tools, opts()).await);
    let again = a.resume(&s.continuation, [(id, Resolution::Approve)], &tools, opts()).await;
    assert_eq!(err(&again).kind, ErrorKind::InvalidContinuation);
    assert!(err(&again).message.contains("already used"), "{}", err(&again).message);

    // Without a shared store each engine only knows its own resumes.
    let (c, d) = (make(Arc::new(MemoryContinuations::default())), make(Arc::new(MemoryContinuations::default())));
    let s = c.run("await bank.transfer({ amount: 1 })", &tools, opts()).await.suspension().unwrap().clone();
    ok(&c.resume(&s.continuation, [(id, Resolution::Approve)], &tools, opts()).await);
    ok(&d.resume(&s.continuation, [(id, Resolution::Approve)], &tools, opts()).await);
}

#[tokio::test]
async fn resume_requires_the_same_tools() {
    let tools = bank(Arc::new(AtomicU32::new(0)));
    let first = engine().run("await bank.transfer({ amount: 1 })", &tools, opts()).await;
    let s = first.suspension().unwrap().clone();
    let mut other = bank(Arc::new(AtomicU32::new(0)));
    other.func("extra", |_c: Call| async move { Ok(Value::Null) });
    let r = engine().resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &other, opts()).await;
    assert_eq!(err(&r).kind, ErrorKind::InvalidContinuation);
}

#[tokio::test]
async fn deny_becomes_a_catchable_error() {
    let tools = bank(Arc::new(AtomicU32::new(0)));
    let first = engine()
        .run("try { await bank.transfer({ amount: 9 }) } catch (e) { `${e.name}: ${e.message}` }", &tools, opts())
        .await;
    let s = first.suspension().unwrap().clone();
    let r = engine()
        .resume(&s.continuation, [(s.interrupts[0].id, Resolution::Deny("user said no".into()))], &tools, opts())
        .await;
    assert_eq!(ok(&r), &Value::from("PermissionDenied: user said no"));
}

#[tokio::test]
async fn parallel_approvals_suspend_together() {
    let counter = Arc::new(AtomicU32::new(0));
    let tools = bank(counter.clone());
    let first = engine()
        .run("const r = await Promise.all([bank.transfer({amount: 1}), bank.transfer({amount: 2})]); r.map(x => x.sent)", &tools, opts())
        .await;
    let s = first.suspension().unwrap().clone();
    assert_eq!(s.interrupts.len(), 2);
    let res = vec![
        (s.interrupts[0].id, Resolution::Approve),
        (s.interrupts[1].id, Resolution::Return(Value::object([("sent", Value::from(-1))]))),
    ];
    let r = engine().resume(&s.continuation, res, &tools, opts()).await;
    assert_eq!(ok(&r).to_json(), serde_json::json!([1, -1]));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tool_raised_interrupts_resume_with_a_value() {
    let mut tools = Toolbox::new();
    tools.func("auth.token", |c: Call| async move {
        match &c.resume {
            None => Err(c.interrupt("sign_in", Value::object([("url", Value::from("https://example.test/login"))]))),
            Some(code) => Ok(Value::from(format!("token-for-{}", code.as_str().unwrap_or("?")))),
        }
    });
    let first = engine().run("const t = await auth.token(); t.toUpperCase()", &tools, opts()).await;
    let s = first.suspension().unwrap().clone();
    assert_eq!(s.interrupts[0].kind, "sign_in");
    assert_eq!(s.interrupts[0].payload.get("url"), Some(&Value::from("https://example.test/login")));
    let r = engine()
        .resume(&s.continuation, [(s.interrupts[0].id, Resolution::ResumeWith(Value::from("abc")))], &tools, opts())
        .await;
    assert_eq!(ok(&r), &Value::from("TOKEN-FOR-ABC"));
}

#[tokio::test]
async fn work_after_an_interrupt_is_deferred_until_resume() {
    let counter = Arc::new(AtomicU32::new(0));
    let mut tools = bank(counter.clone());
    let side = Arc::new(AtomicU32::new(0));
    let s2 = side.clone();
    tools.func("audit.log", move |_c: Call| {
        let s2 = s2.clone();
        async move {
            s2.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Null)
        }
    });
    let code = "const p = bank.transfer({amount: 5}); await audit.log('started'); await p; await audit.log('done'); 'ok'";
    let first = engine().run(code, &tools, opts()).await;
    let s = first.suspension().unwrap().clone();
    // audit.log('started') was issued after the transfer interrupt was raised.
    assert_eq!(side.load(Ordering::SeqCst), 0);
    let r = engine().resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &tools, opts()).await;
    assert_eq!(ok(&r), &Value::from("ok"));
    assert_eq!(side.load(Ordering::SeqCst), 2);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn resume_cost_does_not_grow_with_history() {
    let mut tools = bank(Arc::new(AtomicU32::new(0)));
    tools.func("noop", |_c: Call| async move { Ok(Value::Null) });
    let mut sizes = Vec::new();
    let mut times = Vec::new();
    for n in [0, 200] {
        let code = format!("for (let i = 0; i < {n}; i++) await noop(i); await bank.transfer({{amount: 1}}); 'done'");
        let mut o = opts();
        o.limits.max_host_calls = 10_000;
        let first = engine().run(&code, &tools, o.clone()).await;
        let s = first.suspension().unwrap().clone();
        sizes.push(s.continuation.len());
        let t = Instant::now();
        let r = engine().resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &tools, o).await;
        times.push(t.elapsed());
        assert_eq!(ok(&r), &Value::from("done"));
    }
    eprintln!("continuation sizes {sizes:?}, resume times {times:?}");
    assert!(sizes[1] < sizes[0] * 2, "token grew with history: {sizes:?}");
}
