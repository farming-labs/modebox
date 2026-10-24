//! Node.js bindings. Deliberately small: values cross as tagged JSON
//! strings, and a single JS `dispatch` callback executes every host call.
//! The ergonomic API lives in `packages/modebox/index.mjs`.

#![allow(clippy::result_large_err)] // ToolError carries messages; boxing it buys nothing here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use modebox_core::{
    Approval, Call, ClockMode, Engine, EngineConfig, Limits, Outcome, Resolution, RunOptions, RunResult,
    StoreContinuations, Tool, ToolError, Toolbox, Value,
};
use napi::bindgen_prelude::Promise;
use napi::threadsafe_function::ThreadsafeFunction;
use napi::Status;
use napi_derive::napi;
use serde_json::{Value as Json, json};

type Dispatch = ThreadsafeFunction<String, Promise<String>, String, Status, false>;

fn engines() -> &'static Mutex<HashMap<u32, Engine>> {
    static E: OnceLock<Mutex<HashMap<u32, Engine>>> = OnceLock::new();
    E.get_or_init(Default::default)
}

fn err(e: impl std::fmt::Display) -> napi::Error {
    napi::Error::from_reason(e.to_string())
}

/// Create an engine. `config`: `{ metering?, secret?, cacheDir?, continuationTtlMs?, continuationsDir?, maxInstances? }`.
#[napi]
pub fn create_engine(config: String) -> napi::Result<u32> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let c: Json = serde_json::from_str(&config).map_err(err)?;
    let mut cfg = EngineConfig::default();
    if let Some(m) = c["metering"].as_bool() {
        cfg.metering = m;
    }
    if let Some(s) = c["secret"].as_str() {
        cfg.secret = Some(s.as_bytes().to_vec());
    }
    if let Some(d) = c["cacheDir"].as_str() {
        cfg.cache_dir = Some(d.into());
    }
    if let Some(ms) = c["continuationTtlMs"].as_u64() {
        cfg.continuation_ttl = Duration::from_millis(ms);
    }
    if let Some(n) = c["maxInstances"].as_u64() {
        cfg.max_instances = n as u32;
    }
    if let Some(d) = c["continuationsDir"].as_str() {
        cfg.continuations = Arc::new(StoreContinuations::local(d).map_err(err)?);
    }
    let engine = Engine::with_config(cfg).map_err(|e| err(format!("{e:#}")))?;
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    engines().lock().unwrap().insert(id, engine);
    Ok(id)
}

#[napi]
pub fn drop_engine(id: u32) {
    engines().lock().unwrap().remove(&id);
}

fn engine(id: u32) -> napi::Result<Engine> {
    engines().lock().unwrap().get(&id).cloned().ok_or_else(|| err("engine was dropped"))
}

/// `tools`: `[{ name, approval?: boolean, source?: boolean, sink?: boolean, description?, inputSchema?, outputSchema? }]`.
fn toolbox(tools: &str, dispatch: Arc<Dispatch>) -> napi::Result<Toolbox> {
    let specs: Vec<Json> = serde_json::from_str(tools).map_err(err)?;
    let mut tb = Toolbox::new();
    for spec in specs {
        let name = spec["name"].as_str().ok_or_else(|| err("tool without a name"))?.to_owned();
        let d = dispatch.clone();
        let mut tool = Tool::new(name, move |call: Call| {
            let d = d.clone();
            async move { dispatch_call(&d, call).await }
        });
        if spec["source"] == true {
            tool = tool.source();
        }
        if spec["sink"] == true {
            tool = tool.sink();
        }
        if spec["approval"] == true {
            tool = tool.approval(Approval::Always);
        }
        if let Some(desc) = spec["description"].as_str() {
            tool = tool.description(desc);
        }
        if spec["inputSchema"].is_object() {
            tool = tool.input_schema(spec["inputSchema"].clone());
        }
        if spec["outputSchema"].is_object() {
            tool = tool.output_schema(spec["outputSchema"].clone());
        }
        tb.add(tool);
    }
    Ok(tb)
}

async fn dispatch_call(d: &Dispatch, call: Call) -> Result<Value, ToolError> {
    let mut msg = json!({
        "name": call.name,
        "id": call.id,
        "args": Value::Array(call.args.clone()).to_tagged(),
        "resume": call.resume.as_ref().map(Value::to_tagged),
    });
    if let Some(payload) = &call.interrupt_payload {
        msg["payload"] = payload.to_tagged();
    }
    let promise = d.call_async(msg.to_string()).await.map_err(|e| ToolError::new(format!("host dispatch failed: {e}")))?;
    let out = promise.await.map_err(|e| ToolError::new(e.reason.clone()))?;
    let out: Json = serde_json::from_str(&out).map_err(|e| ToolError::new(format!("bad host reply: {e}")))?;
    if let Some(i) = out.get("interrupt") {
        let kind = i["kind"].as_str().unwrap_or("interrupt").to_owned();
        return Err(call.interrupt(kind, Value::from_tagged(&i["payload"])));
    }
    if out["ok"] == true {
        return Ok(Value::from_tagged(&out["v"]));
    }
    let e = &out["e"];
    let mut te = ToolError::new(e["message"].as_str().unwrap_or("host function failed"))
        .with_name(e["name"].as_str().unwrap_or("Error"));
    if let Some(c) = e["code"].as_str() {
        te = te.with_code(c);
    }
    Err(te)
}

/// `options`: `{ timeoutMs?, fuel?, memoryLimitBytes?, maxHostCalls?, maxConcurrency?, maxValueBytes?, maxLogBytes?, seed?: hex, virtualTimeMs?, mode?, filename?, typescript? }`.
fn run_options(options: &str) -> napi::Result<RunOptions> {
    let o: Json = serde_json::from_str(options).map_err(err)?;
    let mut l = Limits::default();
    if let Some(ms) = o["timeoutMs"].as_u64() {
        l.timeout = Duration::from_millis(ms);
    }
    l.fuel = o["fuel"].as_u64();
    if let Some(n) = o["memoryLimitBytes"].as_u64() {
        l.memory_bytes = n as usize;
    }
    if let Some(n) = o["maxHostCalls"].as_u64() {
        l.max_host_calls = n as u32;
    }
    if let Some(n) = o["maxConcurrency"].as_u64() {
        l.max_concurrency = n as usize;
    }
    if let Some(n) = o["maxValueBytes"].as_u64() {
        l.max_value_bytes = n as usize;
    }
    if let Some(n) = o["maxLogBytes"].as_u64() {
        l.max_log_bytes = n as usize;
    }
    let mut opts = RunOptions { limits: l, ..RunOptions::default() };
    if let Some(seed) = o["seed"].as_str() {
        let mut s = [0u8; 32];
        for (i, ch) in seed.as_bytes().chunks(2).take(32).enumerate() {
            s[i] = u8::from_str_radix(std::str::from_utf8(ch).unwrap_or("0"), 16).unwrap_or(0);
        }
        opts.seed = Some(s);
    }
    if let Some(t) = o["virtualTimeMs"].as_f64() {
        opts.clock = ClockMode::Virtual { start_ms: t };
    }
    if let Some(m) = o["mode"].as_str() {
        opts.mode = serde_json::from_value(json!(m)).map_err(err)?;
    }
    if let Some(f) = o["filename"].as_str() {
        opts.filename = f.into();
    }
    if let Some(ts) = o["typescript"].as_bool() {
        opts.typescript = ts;
    }
    Ok(opts)
}

fn result_json(r: RunResult) -> String {
    let logs: Vec<Json> = r.logs.iter().map(|l| json!({ "level": l.level, "message": l.message })).collect();
    let stats = json!({
        "wallMs": r.stats.wall.as_secs_f64() * 1000.0,
        "guestMs": r.stats.guest.as_secs_f64() * 1000.0,
        "instantiateMs": r.stats.instantiate.as_secs_f64() * 1000.0,
        "fuelUsed": r.stats.fuel_used,
        "hostCalls": r.stats.host_calls,
        "memoryBytes": r.stats.memory_bytes,
        "continuationBytes": r.stats.continuation_bytes,
    });
    let out = match r.outcome {
        Outcome::Completed(v) => json!({ "status": "completed", "value": v.to_tagged(), "logs": logs, "stats": stats }),
        Outcome::Failed(e) => json!({
            "status": "failed",
            "error": { "kind": format!("{:?}", e.kind), "name": e.name, "message": e.message, "stack": e.stack },
            "logs": logs, "stats": stats,
        }),
        Outcome::Suspended(s) => json!({
            "status": "suspended",
            "interrupts": s.interrupts.iter().map(|i| json!({
                "id": i.id, "tool": i.tool, "kind": i.kind,
                "args": Value::Array(i.args.clone()).to_tagged(), "payload": i.payload.to_tagged(),
            })).collect::<Vec<_>>(),
            "continuation": s.continuation, "logs": logs, "stats": stats,
        }),
    };
    out.to_string()
}

#[napi]
pub async fn run(engine_id: u32, code: String, tools: String, options: String, dispatch: Dispatch) -> napi::Result<String> {
    let engine = engine(engine_id)?;
    let tb = toolbox(&tools, Arc::new(dispatch))?;
    let opts = run_options(&options)?;
    Ok(result_json(engine.run(&code, &tb, opts).await))
}

/// `resolutions`: `[{ id, action: "approve"|"deny"|"resume"|"return"|"throw", value?: tagged, message? }]`.
#[napi]
pub async fn resume(
    engine_id: u32,
    continuation: String,
    resolutions: String,
    tools: String,
    options: String,
    dispatch: Dispatch,
) -> napi::Result<String> {
    let engine = engine(engine_id)?;
    let tb = toolbox(&tools, Arc::new(dispatch))?;
    let opts = run_options(&options)?;
    let list: Vec<Json> = serde_json::from_str(&resolutions).map_err(err)?;
    let mut res = Vec::new();
    for r in list {
        let id = r["id"].as_u64().ok_or_else(|| err("resolution without an id"))? as u32;
        let msg = r["message"].as_str().unwrap_or("denied").to_owned();
        let res_one = match r["action"].as_str().unwrap_or("approve") {
            "approve" => Resolution::Approve,
            "deny" => Resolution::Deny(msg),
            "resume" => Resolution::ResumeWith(Value::from_tagged(&r["value"])),
            "return" => Resolution::Return(Value::from_tagged(&r["value"])),
            "throw" => Resolution::Throw(ToolError::new(msg)),
            other => return Err(err(format!("unknown resolution action {other}"))),
        };
        res.push((id, res_one));
    }
    Ok(result_json(engine.resume(&continuation, res, &tb, opts).await))
}

/// TypeScript declarations for a tool list (same shape as `run`'s `tools`).
#[napi]
pub fn declarations(tools: String) -> napi::Result<String> {
    let specs: Vec<Json> = serde_json::from_str(&tools).map_err(err)?;
    let mut tb = Toolbox::new();
    for spec in specs {
        let name = spec["name"].as_str().ok_or_else(|| err("tool without a name"))?.to_owned();
        let mut tool = Tool::new(name, |_c: Call| async { Ok(Value::Undefined) });
        if let Some(desc) = spec["description"].as_str() {
            tool = tool.description(desc);
        }
        if spec["inputSchema"].is_object() {
            tool = tool.input_schema(spec["inputSchema"].clone());
        }
        if spec["outputSchema"].is_object() {
            tool = tool.output_schema(spec["outputSchema"].clone());
        }
        tb.add(tool);
    }
    Ok(modebox_core::declarations(&tb))
}
