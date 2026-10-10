//! Same workloads as bench/run-sdk.mjs (Vercel's `run`), for modebox.
//! `cargo run --release -p modebox-core --example bench`

use std::sync::Arc;
use std::time::{Duration, Instant};

use modebox_core::*;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let t = Instant::now();
    let engine = Engine::new()?;
    let engine_startup = t.elapsed();

    let mut tools = Toolbox::new();
    tools.func("tools.echo", |c: Call| async move { Ok(c.input().clone()) });
    let rows: Value = Value::Array(
        (0..5000)
            .map(|i| {
                Value::object([
                    ("id", Value::from(i)),
                    ("name", Value::from(format!("user{i}"))),
                    ("email", Value::from(format!("u{i}@example.com"))),
                    ("active", Value::from(i % 2 == 0)),
                ])
            })
            .collect(),
    );
    tools.func("db.rows", move |_c: Call| {
        let rows = rows.clone();
        async move { Ok(rows) }
    });
    tools.func("db.get", |_c: Call| async move { Ok(Value::from("z".repeat(4096))) });
    tools.add(Tool::new("approve.ask", |_c: Call| async move { Ok(Value::Bool(true)) }).approval(Approval::Always));
    let tools = Arc::new(tools);

    let opts = || RunOptions { limits: Limits { max_host_calls: 10_000, ..Limits::default() }, ..RunOptions::default() };
    let check = |r: &RunResult| {
        if let Outcome::Failed(e) = &r.outcome {
            panic!("{e}");
        }
    };

    // warm
    check(&engine.run("return 1", &tools, opts()).await);

    let mut out = serde_json::Map::new();
    let mut put = |k: &str, v: f64| {
        out.insert(k.into(), serde_json::json!((v * 1000.0).round() / 1000.0));
    };
    put("engine_startup_ms", ms(engine_startup));

    let t = Instant::now();
    for _ in 0..1000 {
        check(&engine.run("return 1", &tools, opts()).await);
    }
    put("trivial_run_ms", ms(t.elapsed()) / 1000.0);

    let t = Instant::now();
    for _ in 0..200 {
        check(&engine.run("return await tools.echo(1);", &tools, opts()).await);
    }
    put("one_host_call_ms", ms(t.elapsed()) / 200.0);

    let t = Instant::now();
    check(&engine.run("let s = 0; for (let i = 0; i < 200; i++) s += await tools.echo(i); return s;", &tools, opts()).await);
    put("host_calls_200_in_one_run_ms", ms(t.elapsed()));

    let t = Instant::now();
    for _ in 0..125 {
        let futs = (0..8).map(|_| {
            let (engine, tools) = (engine.clone(), tools.clone());
            tokio::spawn(async move { engine.run("return 1", &tools, RunOptions::default()).await })
        });
        for r in futures::future::join_all(futs).await {
            check(&r?);
        }
    }
    put("parallel8_per_run_ms", ms(t.elapsed()) / 1000.0);

    let cpu = "let s = 0; for (let i = 1; i <= 10_000_000; i++) s += i; return s;";
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        check(&engine.run(cpu, &tools, opts()).await);
        best = best.min(ms(t.elapsed()));
    }
    put("cpu_sum_10m_ms", best);
    let unmetered = Engine::with_config(EngineConfig { metering: false, ..EngineConfig::default() })?;
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        check(&unmetered.run(cpu, &tools, opts()).await);
        best = best.min(ms(t.elapsed()));
    }
    put("cpu_sum_10m_unmetered_ms", best);

    let t = Instant::now();
    check(&engine.run("return 'x'.repeat(1 << 20);", &tools, opts()).await);
    put("return_1mb_string_ms", ms(t.elapsed()));

    let t = Instant::now();
    check(&engine.run("const r = await db.rows(); return r.length;", &tools, opts()).await);
    put("host_returns_5000_rows_ms", ms(t.elapsed()));

    let t = Instant::now();
    for _ in 0..50 {
        let a = engine.run("return await approve.ask();", &tools, opts()).await;
        let s = a.suspension().expect("suspended").clone();
        check(&engine.resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &tools, opts()).await);
    }
    put("interrupt_resume_pair_ms", ms(t.elapsed()) / 50.0);

    let src = "for (let i = 0; i < 200; i++) await db.get(i); return await approve.ask();";
    let first = engine.run(src, &tools, opts()).await;
    let s = first.suspension().expect("suspended").clone();
    put("token_bytes_after_200_calls", s.continuation.len() as f64);
    let t = Instant::now();
    check(&engine.resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &tools, opts()).await);
    put("resume_after_200_calls_ms", ms(t.elapsed()));

    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
