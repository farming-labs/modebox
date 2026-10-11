//! modebox guest: QuickJS driven by a host-controlled event loop.
//!
//! The host never calls into the guest while JS is on the stack. Every export
//! runs QuickJS until its job queue is drained, then returns a status. Between
//! export calls the wasm stack is empty and all VM state lives in linear
//! memory, which is what makes whole-instance snapshots (true continuations,
//! no replay) possible.
//!
//! Host calls are fire-and-forget from the guest's point of view: `al.call`
//! emits a request and returns a pending Promise. The host settles it later via
//! `mb_settle`. There are no synchronous host functions by design.

mod codec;
mod inspect;

use std::cell::RefCell;
use std::collections::BTreeMap;

use base64::Engine as _;
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use rquickjs::context::EvalOptions;
use rquickjs::function::{Opt, Rest};
use rquickjs::{qjs, Array, Context, Ctx, Function, Object, Persistent, Runtime, TypedArray, Value};
use serde_json::value::RawValue;
use serde_json::Value as Json;

use crate::codec::{clip, replace_surrogates, utf8, Helpers, StrError};

const PRELUDE: &str = include_str!("prelude.js");

/// QuickJS stack budget. Must stay below the 4 MiB shadow stack set at link
/// time so overflow surfaces as a catchable RangeError instead of a trap.
const MAX_STACK: usize = 3 * 1024 * 1024;

/// Most text one `console.*` call can produce; the rest is cut.
const MAX_LOG_MESSAGE: usize = 64 << 10;

/// Longest error name, message or stack reported to the host.
const MAX_ERROR_TEXT: usize = 1 << 20;

// ---- host ABI ---------------------------------------------------------------

#[link(wasm_import_module = "modebox")]
extern "C" {
    /// Deliver one event to the host. `kind` is one of the `EV_*` constants and
    /// the payload is UTF-8 JSON.
    fn emit(kind: u32, ptr: *const u8, len: u32);
}

const EV_CALL: u32 = 1;
const EV_LOG: u32 = 2;
const EV_DONE: u32 = 3;
const EV_ERROR: u32 = 4;
const EV_CANCEL: u32 = 5;

const ST_DONE: u32 = 0;
const ST_WAITING: u32 = 1;
const ST_ERROR: u32 = 2;

fn send(kind: u32, bytes: &[u8]) {
    unsafe { emit(kind, bytes.as_ptr(), bytes.len() as u32) }
}

/// Allocate `len` bytes for the host to write an input message into. Ownership
/// passes back to the guest when the pointer is handed to an export.
#[no_mangle]
pub extern "C" fn mb_alloc(len: u32) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

unsafe fn take_input(ptr: *mut u8, len: u32) -> Vec<u8> {
    Vec::from_raw_parts(ptr, len as usize, len as usize)
}

// ---- state ------------------------------------------------------------------

struct Vm {
    rt: Runtime,
    ctx: Context,
    hooks: Persistent<Object<'static>>,
}

type Settlers = (Persistent<Function<'static>>, Persistent<Function<'static>>);

struct Bridge {
    next_id: u32,
    last_id: u32,
    pending: BTreeMap<u32, Settlers>,
    main: Option<Persistent<Value<'static>>>,
    script_mode: bool,
    max_value_bytes: usize,
    max_depth: usize,
    rng: ChaCha8Rng,
    /// Rejections with no handler yet, keyed by promise address. Reported once
    /// the job queue drains, so a handler attached later in the same turn
    /// doesn't produce a false warning.
    unhandled: Vec<(usize, String)>,
}

thread_local! {
    static VM: RefCell<Option<Vm>> = const { RefCell::new(None) };
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
}

fn bridge<R>(f: impl FnOnce(&mut Bridge) -> R) -> R {
    BRIDGE.with(|b| f(b.borrow_mut().as_mut().expect("modebox: not initialized")))
}

fn with_ctx<R>(f: impl for<'js> FnOnce(&Ctx<'js>, &Vm) -> R) -> R {
    VM.with(|vm| {
        let vm = vm.borrow();
        let vm = vm.as_ref().expect("modebox: not initialized");
        vm.ctx.with(|ctx| f(&ctx, vm))
    })
}

pub(crate) fn helpers<'js>(ctx: &Ctx<'js>) -> Helpers<'js> {
    let hooks = VM.with(|vm| vm.borrow().as_ref().unwrap().hooks.clone());
    Helpers::new(hooks.restore(ctx).expect("hooks"))
}

// ---- init -------------------------------------------------------------------

/// Build the runtime, context and prelude. Run once at pre-initialization;
/// the resulting memory image is what every run starts from.
#[no_mangle]
pub extern "C" fn mb_init() {
    if VM.with(|vm| vm.borrow().is_some()) {
        return;
    }
    BRIDGE.with(|b| {
        *b.borrow_mut() = Some(Bridge {
            next_id: 0,
            last_id: 0,
            pending: BTreeMap::new(),
            main: None,
            script_mode: false,
            max_value_bytes: 16 << 20,
            max_depth: 256,
            rng: ChaCha8Rng::from_seed([0; 32]),
            unhandled: Vec::new(),
        })
    });

    let rt = Runtime::new().expect("modebox: runtime");
    rt.set_max_stack_size(MAX_STACK);
    rt.set_host_promise_rejection_tracker(Some(Box::new(|ctx, promise, reason, is_handled| {
        let key = unsafe { qjs::JS_VALUE_GET_PTR(promise.as_raw()) } as usize;
        if is_handled {
            bridge(|b| b.unhandled.retain(|(k, _)| *k != key));
        } else {
            let text = inspect::inspect(&ctx, &reason, &helpers(&ctx), false);
            bridge(|b| b.unhandled.push((key, text)));
        }
    })));
    let ctx = Context::full(&rt).expect("modebox: context");
    let hooks = ctx
        .with(|ctx| -> rquickjs::Result<Persistent<Object<'static>>> {
            let natives = natives(&ctx)?;
            let mut opts = EvalOptions::default();
            opts.global = true;
            opts.strict = true;
            opts.backtrace_barrier = true;
            opts.filename = Some("modebox:prelude".into());
            let factory: Function = ctx.eval_with_options(PRELUDE, opts)?;
            let hooks: Object = factory.call((natives,))?;
            Ok(Persistent::save(&ctx, hooks))
        })
        .expect("modebox: prelude");
    rt.run_gc();
    VM.with(|vm| *vm.borrow_mut() = Some(Vm { rt, ctx, hooks }));
}

fn natives<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Object<'js>> {
    let al = Object::new(ctx.clone())?;

    al.set(
        "call",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, name: String, args: Value<'js>| {
            host_call(&ctx, name, args)
        })?,
    )?;
    al.set("lastCallId", Function::new(ctx.clone(), || bridge(|b| b.last_id))?)?;
    al.set(
        "cancel",
        Function::new(ctx.clone(), |id: u32| {
            send(EV_CANCEL, format!("{{\"id\":{id}}}").as_bytes());
        })?,
    )?;
    al.set(
        "log",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, level: String, args: Rest<Value<'js>>| {
            let h = helpers(&ctx);
            let mut msg = String::new();
            for (i, arg) in args.0.iter().enumerate() {
                if msg.len() >= MAX_LOG_MESSAGE {
                    msg.push_str("… (truncated)");
                    break;
                }
                if i > 0 {
                    msg.push(' ');
                }
                msg.push_str(&inspect::inspect(&ctx, arg, &h, true));
            }
            let event = serde_json::json!({ "level": level, "msg": msg });
            send(EV_LOG, event.to_string().as_bytes());
        })?,
    )?;
    al.set(
        "random",
        Function::new(ctx.clone(), || {
            let bits = bridge(|b| b.rng.next_u64()) >> 11;
            bits as f64 * (1.0 / (1u64 << 53) as f64)
        })?,
    )?;
    al.set(
        "fillRandom",
        Function::new(ctx.clone(), |bytes: TypedArray<'js, u8>| {
            if let Some(raw) = bytes.as_raw() {
                let slice = unsafe { &mut *raw.as_ptr() };
                bridge(|b| b.rng.fill_bytes(slice));
            }
        })?,
    )?;
    al.set(
        "utf8Encode",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: Value<'js>| -> rquickjs::Result<TypedArray<'js, u8>> {
            // Copied once, from QuickJS's UTF-8 straight into a new buffer.
            match utf8(&ctx, &s) {
                Some(Ok(u)) => TypedArray::new_copy(ctx, u.as_str().as_bytes()),
                Some(Err(StrError::NotUtf8)) => {
                    // Unpaired surrogates encode as U+FFFD, as TextEncoder specifies.
                    let raw = s.as_string().cloned().map(|s| s.to_cstring());
                    let text = match raw {
                        Some(Ok(c)) => replace_surrogates(unsafe {
                            std::slice::from_raw_parts(c.as_ptr() as *const u8, c.len())
                        }),
                        _ => None,
                    };
                    match text {
                        Some(t) => TypedArray::new_copy(ctx, t.as_bytes()),
                        None => {
                            let _ = ctx.catch();
                            Err(throw_range(&ctx, "TextEncoder: string is too large"))
                        }
                    }
                }
                Some(Err(StrError::OutOfMemory)) => Err(throw_range(&ctx, "TextEncoder: string is too large")),
                None => TypedArray::new_copy(ctx, b""),
            }
        })?,
    )?;
    al.set(
        "utf8Decode",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, bytes: TypedArray<'js, u8>, fatal: Opt<bool>| -> rquickjs::Result<rquickjs::String<'js>> {
                let slice = codec::bytes_of(&bytes);
                match std::str::from_utf8(slice) {
                    Ok(s) => rquickjs::String::from_str(ctx, s),
                    Err(_) if fatal.0.unwrap_or(false) => Err(rquickjs::Exception::throw_type(
                        &ctx,
                        "TextDecoder: the encoded data was not valid UTF-8",
                    )),
                    Err(_) => {
                        let mut text = Vec::new();
                        if text.try_reserve(slice.len() + 16).is_err() {
                            return Err(throw_range(&ctx, "TextDecoder: input is too large"));
                        }
                        for chunk in slice.utf8_chunks() {
                            text.extend_from_slice(chunk.valid().as_bytes());
                            if !chunk.invalid().is_empty() {
                                if text.try_reserve(3).is_err() {
                                    return Err(throw_range(&ctx, "TextDecoder: input is too large"));
                                }
                                text.extend_from_slice("\u{FFFD}".as_bytes());
                            }
                        }
                        rquickjs::String::from_str(ctx, std::str::from_utf8(&text).unwrap_or_default())
                    }
                }
            },
        )?,
    )?;
    al.set(
        "atob",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: Value<'js>| -> rquickjs::Result<rquickjs::String<'js>> {
            let invalid = |ctx: &Ctx<'js>| {
                rquickjs::Exception::throw_message(ctx, "InvalidCharacterError: atob() input is not valid base64")
            };
            let input = match utf8(&ctx, &s) {
                Some(Ok(u)) => u,
                Some(Err(StrError::OutOfMemory)) => return Err(throw_range(&ctx, "atob() input is too large")),
                _ => return Err(invalid(&ctx)),
            };
            let raw = input.as_str().as_bytes();
            let mut cleaned = Vec::new();
            let b64 = if raw.iter().any(u8::is_ascii_whitespace) {
                if cleaned.try_reserve(raw.len()).is_err() {
                    return Err(throw_range(&ctx, "atob() input is too large"));
                }
                cleaned.extend(raw.iter().filter(|b| !b.is_ascii_whitespace()));
                &cleaned[..]
            } else {
                raw
            };
            let engine = base64::engine::GeneralPurpose::new(
                &base64::alphabet::STANDARD,
                base64::engine::GeneralPurposeConfig::new()
                    .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
            );
            let mut bytes = Vec::new();
            if bytes.try_reserve(base64::decoded_len_estimate(b64.len())).is_err() {
                return Err(throw_range(&ctx, "atob() input is too large"));
            }
            if engine.decode_vec(b64, &mut bytes).is_err() {
                return Err(invalid(&ctx));
            }
            // Each byte is one Latin-1 character: up to 2 bytes of UTF-8.
            let mut text = String::new();
            if text.try_reserve(bytes.len() * 2).is_err() {
                return Err(throw_range(&ctx, "atob() input is too large"));
            }
            text.extend(bytes.iter().map(|&b| char::from(b)));
            rquickjs::String::from_str(ctx, &text)
        })?,
    )?;
    al.set(
        "btoa",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: Value<'js>| -> rquickjs::Result<rquickjs::String<'js>> {
            let outside = |ctx: &Ctx<'js>| {
                rquickjs::Exception::throw_message(
                    ctx,
                    "InvalidCharacterError: btoa() input contains characters outside Latin-1",
                )
            };
            let input = match utf8(&ctx, &s) {
                Some(Ok(u)) => u,
                Some(Err(StrError::OutOfMemory)) => return Err(throw_range(&ctx, "btoa() input is too large")),
                _ => return Err(outside(&ctx)),
            };
            let text = input.as_str();
            let mut bytes = Vec::new();
            let mut encoded = String::new();
            if bytes.try_reserve(text.len()).is_err() || encoded.try_reserve(text.len().div_ceil(3) * 4).is_err() {
                return Err(throw_range(&ctx, "btoa() input is too large"));
            }
            for c in text.chars() {
                if (c as u32) > 0xff {
                    return Err(outside(&ctx));
                }
                bytes.push(c as u8);
            }
            base64::engine::general_purpose::STANDARD.encode_string(&bytes, &mut encoded);
            rquickjs::String::from_str(ctx, &encoded)
        })?,
    )?;
    al.set(
        "clone",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, v: Value<'js>| -> rquickjs::Result<Value<'js>> {
            let h = helpers(&ctx);
            let (limit, depth) = bridge(|b| (b.max_value_bytes, b.max_depth));
            let mut out = Vec::new();
            if let Err(e) = codec::encode(&ctx, &v, &h, &mut out, limit, depth) {
                return Err(rquickjs::Exception::throw_message(
                    &ctx,
                    &format!("DataCloneError: {}", e.describe("value")),
                ));
            }
            let decoded = std::str::from_utf8(&out).map_err(|e| e.to_string()).and_then(|json| codec::decode(&ctx, json, &h));
            decoded.map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("DataCloneError: {e}")))
        })?,
    )?;
    Ok(al)
}

/// Throw a RangeError (for inputs too large to handle).
fn throw_range(ctx: &Ctx<'_>, msg: &str) -> rquickjs::Error {
    rquickjs::Exception::throw_range(ctx, msg)
}

fn host_call<'js>(ctx: &Ctx<'js>, name: String, args: Value<'js>) -> rquickjs::Result<Value<'js>> {
    let h = helpers(ctx);
    let (limit, depth) = bridge(|b| (b.max_value_bytes, b.max_depth));
    let mut encoded = Vec::with_capacity(64);
    if let Err(e) = codec::encode(ctx, &args, &h, &mut encoded, limit, depth) {
        let msg = format!("{name}(): {}", e.describe("arguments"));
        return Err(rquickjs::Exception::throw_type(ctx, &msg));
    }
    let (promise, resolve, reject) = ctx.promise()?;
    let id = bridge(|b| {
        b.next_id += 1;
        b.last_id = b.next_id;
        b.pending
            .insert(b.next_id, (Persistent::save(ctx, resolve), Persistent::save(ctx, reject)));
        b.next_id
    });
    let mut msg = Vec::with_capacity(encoded.len() + name.len() + 32);
    msg.extend_from_slice(b"{\"id\":");
    msg.extend_from_slice(id.to_string().as_bytes());
    msg.extend_from_slice(b",\"fn\":");
    serde_json::to_writer(&mut msg, &name).unwrap();
    msg.extend_from_slice(b",\"args\":");
    msg.extend_from_slice(&encoded);
    msg.push(b'}');
    send(EV_CALL, &msg);
    Ok(promise.into_value())
}

// ---- run ----------------------------------------------------------------------

/// Start a run. Input: JSON
/// `{code, mode: "auto"|"script"|"function", fns: [..], seed: hex32, memoryLimit, maxValueBytes, filename}`.
///
/// "script" evaluates global code with top-level await; the completion value
/// of the last statement is the result and top-level declarations persist in
/// the context (REPL semantics, used by sessions). "function" wraps the code in
/// an async function body so `return` works. "auto" picks between them.
#[no_mangle]
pub unsafe extern "C" fn mb_start(ptr: *mut u8, len: u32) -> u32 {
    let input = take_input(ptr, len);
    let req: Json = match serde_json::from_slice(&input) {
        Ok(v) => v,
        Err(e) => return fail_plain("ModeboxError", &format!("bad start request: {e}")),
    };
    if bridge(|b| b.main.is_some()) {
        return fail_plain("ModeboxError", "a run is already in progress in this instance");
    }
    let code = req["code"].as_str().unwrap_or("");
    let mode = req["mode"].as_str().unwrap_or("auto");
    let filename = req["filename"].as_str().unwrap_or("agent.js").to_owned();
    let mut seed = [0u8; 32];
    if let Some(hex) = req["seed"].as_str() {
        for (i, chunk) in hex.as_bytes().chunks(2).take(32).enumerate() {
            seed[i] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap_or("0"), 16).unwrap_or(0);
        }
    }
    bridge(|b| {
        b.rng = ChaCha8Rng::from_seed(seed);
        if let Some(n) = req["maxValueBytes"].as_u64() {
            b.max_value_bytes = n as usize;
        }
        b.unhandled.clear();
    });
    VM.with(|vm| {
        let vm = vm.borrow();
        let vm = vm.as_ref().expect("modebox: not initialized");
        if let Some(limit) = req["memoryLimit"].as_u64() {
            vm.rt.set_memory_limit(limit as usize);
        }
    });

    with_ctx(|ctx, vm| {
        // Can fail if the memory limit is tiny: report it like any other error.
        let installed = (|| -> rquickjs::Result<()> {
            let hooks = vm.hooks.clone().restore(ctx)?;
            let install: Function = hooks.get("install")?;
            let fns = Array::new(ctx.clone())?;
            if let Some(list) = req["fns"].as_array() {
                for (i, f) in list.iter().enumerate() {
                    if let Some(s) = f.as_str() {
                        fns.set(i, s)?;
                    }
                }
            }
            install.call::<_, ()>((fns,))
        })();
        if installed.is_err() {
            let exc = ctx.catch();
            return fail_value(ctx, &exc);
        }

        // "auto": try REPL-style script mode first; QuickJS rejects a top-level
        // `return` at parse time (nothing has run yet), so fall back to an
        // async function body in that case.
        let attempts: &[bool] = match mode {
            "script" => &[true],
            "function" => &[false],
            _ => &[true, false],
        };
        for (n, &script_mode) in attempts.iter().enumerate() {
            let source = if script_mode {
                code.to_owned()
            } else {
                // Keep user code on line 1 so stack positions match the source.
                format!("(async () => {{{code}\n}})()")
            };
            let mut opts = EvalOptions::default();
            opts.global = true;
            opts.strict = false;
            opts.backtrace_barrier = true;
            opts.promise = script_mode;
            opts.filename = Some(filename.clone());
            match ctx.eval_with_options::<Value, _>(source, opts) {
                Ok(main) => {
                    bridge(|b| b.script_mode = script_mode);
                    set_main(ctx, main);
                    return drive(ctx);
                }
                Err(_) => {
                    let exc = ctx.catch();
                    let is_return = exc
                        .as_object()
                        .and_then(|o| o.get::<_, String>("message").ok())
                        .is_some_and(|m| m.contains("return not in a function"));
                    if is_return && n + 1 < attempts.len() {
                        continue;
                    }
                    return fail_value(ctx, &exc);
                }
            }
        }
        unreachable!()
    })
}

fn set_main<'js>(ctx: &Ctx<'js>, main: Value<'js>) {
    // Mark the main promise handled; its outcome is read directly.
    if let Some(p) = main.as_object() {
        let noop = Function::new(ctx.clone(), || {});
        if let (Ok(catch), Ok(noop)) = (p.get::<_, Function>("catch"), noop) {
            let _ = catch.call::<_, Value>((rquickjs::function::This(main.clone()), noop));
        }
        let _ = ctx.catch();
    }
    bridge(|b| b.main = Some(Persistent::save(ctx, main)));
}

/// One settled host call. Values stay raw JSON until they're decoded
/// straight into JS, so no parse tree of a large result is ever built.
#[derive(serde::Deserialize)]
struct Settled<'a> {
    id: u32,
    #[serde(default)]
    ok: bool,
    #[serde(borrow, default)]
    v: Option<&'a RawValue>,
    #[serde(borrow, default)]
    e: Option<HostError<'a>>,
}

#[derive(serde::Deserialize, Default)]
struct HostError<'a> {
    #[serde(borrow, default)]
    name: Option<std::borrow::Cow<'a, str>>,
    #[serde(borrow, default)]
    message: Option<std::borrow::Cow<'a, str>>,
    #[serde(borrow, default)]
    code: Option<&'a RawValue>,
    #[serde(borrow, default)]
    data: Option<&'a RawValue>,
}

/// The JS error a failed host call rejects with.
fn host_error<'js>(ctx: &Ctx<'js>, h: &Helpers<'js>, e: &HostError<'_>) -> rquickjs::Result<Value<'js>> {
    let extra = Object::new(ctx.clone())?;
    for (k, raw) in [("code", e.code), ("data", e.data)] {
        if let Some(raw) = raw {
            // A field that can't be loaded is dropped rather than failing the call.
            if let Ok(v) = codec::decode(ctx, raw.get(), h) {
                extra.set(k, v)?;
            }
        }
    }
    h.mk_error.call((
        e.name.as_deref().unwrap_or("Error"),
        e.message.as_deref().unwrap_or("host function failed"),
        Value::new_undefined(ctx.clone()),
        extra,
    ))
}

/// Settle host calls. Input: JSON array of `{id, ok: true, v}` or
/// `{id, ok: false, e: {name, message, code?, data?}}`.
#[no_mangle]
pub unsafe extern "C" fn mb_settle(ptr: *mut u8, len: u32) -> u32 {
    let input = take_input(ptr, len);
    let items: Vec<Settled> = match serde_json::from_slice(&input) {
        Ok(v) => v,
        Err(e) => return fail_plain("ModeboxError", &format!("bad settle request: {e}")),
    };
    with_ctx(|ctx, _vm| {
        let h = helpers(ctx);
        for item in &items {
            let Some((resolve, reject)) = bridge(|b| b.pending.remove(&item.id)) else { continue };
            let resolve = resolve.restore(ctx).expect("resolve");
            let reject = reject.restore(ctx).expect("reject");
            let result = if item.ok {
                match codec::decode(ctx, item.v.map_or("null", |v| v.get()), &h) {
                    Ok(value) => resolve.call::<_, ()>((value,)),
                    // Too big to load (or malformed): the call fails, the run goes on.
                    Err(why) => {
                        let msg = format!("the host function's result could not be loaded into the sandbox: {why}");
                        let err = h.mk_error.call::<_, Value>(("RangeError", msg, Value::new_undefined(ctx.clone())));
                        let err = err.unwrap_or_else(|_| ctx.catch());
                        reject.call::<_, ()>((err,))
                    }
                }
            } else {
                let err = host_error(ctx, &h, item.e.as_ref().unwrap_or(&HostError::default()));
                let err = err.unwrap_or_else(|_| ctx.catch());
                reject.call::<_, ()>((err,))
            };
            if result.is_err() {
                // resolve/reject never throw for well-formed promises; drain anyway.
                let _ = ctx.catch();
            }
        }
        drive(ctx)
    })
}

/// Prepare for a snapshot: run a full GC cycle, then zero the dead part of
/// the shadow stack so it matches the base image and drops out of the diff.
/// Returns heap usage.
#[no_mangle]
pub extern "C" fn mb_gc() -> u32 {
    let used = VM.with(|vm| {
        let vm = vm.borrow();
        let vm = vm.as_ref().expect("modebox: not initialized");
        vm.rt.run_gc();
        vm.rt.memory_usage().malloc_size.max(0) as u32
    });
    scrub_stack();
    used
}

/// The stack is placed first in linear memory and grows down from 4 MiB, so
/// everything below the current frame is garbage left by earlier calls.
#[inline(never)]
fn scrub_stack() {
    let marker = 0u8;
    let sp = std::hint::black_box(&marker) as *const u8 as usize;
    let low = 1024usize;
    let high = sp.saturating_sub(8192);
    if high > low {
        // SAFETY: [low, high) is unused stack below every live frame.
        unsafe { std::ptr::write_bytes(low as *mut u8, 0, high - low) }
    }
}

/// Abandon an unfinished run (the host gave up on it, e.g. a timeout while a
/// tool was still working) so the instance can take a new one. Pending
/// promises become unreachable and are garbage collected.
#[no_mangle]
pub extern "C" fn mb_reset_run() {
    let dropped = bridge(|b| {
        b.main = None;
        b.unhandled.clear();
        std::mem::take(&mut b.pending)
    });
    drop(dropped);
}

/// Number of host calls the guest is still waiting on.
#[no_mangle]
pub extern "C" fn mb_pending() -> u32 {
    bridge(|b| b.pending.len() as u32)
}

fn drive(ctx: &Ctx<'_>) -> u32 {
    while ctx.execute_pending_job() {}
    flush_unhandled();

    let Some(main) = bridge(|b| b.main.clone()) else {
        return fail_plain("ModeboxError", "no run in progress");
    };
    let main = main.restore(ctx).expect("main");
    let state = unsafe { qjs::JS_PromiseState(ctx.as_raw().as_ptr(), main.as_raw()) };
    match state {
        qjs::JSPromiseStateEnum_JS_PROMISE_FULFILLED => {
            bridge(|b| b.main = None);
            let raw = unsafe { qjs::JS_PromiseResult(ctx.as_raw().as_ptr(), main.as_raw()) };
            let mut value = unsafe { Value::from_raw(ctx.clone(), raw) };
            if bridge(|b| b.script_mode) {
                // Async global eval resolves to `{ value: <completion> }`.
                if let Some(obj) = value.as_object() {
                    value = obj.get("value").unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
                }
                // A script ending in an un-awaited promise (`tools.x()`) means
                // "the result of that call": await it rather than failing.
                if value.is_promise() {
                    bridge(|b| b.script_mode = false);
                    set_main(ctx, value);
                    return drive(ctx);
                }
            }
            let h = helpers(ctx);
            let (limit, depth) = bridge(|b| (b.max_value_bytes, b.max_depth));
            let mut out = Vec::with_capacity(64);
            out.extend_from_slice(b"{\"value\":");
            if let Err(e) = codec::encode(ctx, &value, &h, &mut out, limit, depth) {
                let msg = format!("the script's result {}", e.describe_bare());
                return fail_plain("TypeError", &msg);
            }
            out.push(b'}');
            send(EV_DONE, &out);
            ST_DONE
        }
        qjs::JSPromiseStateEnum_JS_PROMISE_REJECTED => {
            bridge(|b| b.main = None);
            let raw = unsafe { qjs::JS_PromiseResult(ctx.as_raw().as_ptr(), main.as_raw()) };
            let reason = unsafe { Value::from_raw(ctx.clone(), raw) };
            fail_value(ctx, &reason)
        }
        _ => {
            if bridge(|b| b.pending.is_empty()) {
                bridge(|b| b.main = None);
                fail_plain(
                    "DeadlockError",
                    "the script is awaiting a promise that can never settle (no host calls or timers are pending)",
                )
            } else {
                ST_WAITING
            }
        }
    }
}

fn flush_unhandled() {
    let items = bridge(|b| std::mem::take(&mut b.unhandled));
    for (_, text) in items {
        let event = serde_json::json!({ "level": "warn", "msg": format!("Unhandled promise rejection: {text}") });
        send(EV_LOG, event.to_string().as_bytes());
    }
}

/// Whether QuickJS's heap is (all but) at its limit. Raw API: the rquickjs
/// runtime is already locked while a context is in use.
fn at_memory_limit(ctx: &Ctx<'_>) -> bool {
    unsafe {
        let rt = qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
        let mut usage: qjs::JSMemoryUsage = std::mem::zeroed();
        qjs::JS_ComputeMemoryUsage(rt, &mut usage);
        usage.malloc_limit > 0 && usage.malloc_size + (16 << 10) >= usage.malloc_limit
    }
}

fn fail_plain(name: &str, message: &str) -> u32 {
    let event = serde_json::json!({ "name": name, "message": message });
    send(EV_ERROR, event.to_string().as_bytes());
    ST_ERROR
}

fn fail_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> u32 {
    let h = helpers(ctx);
    let mut event = serde_json::Map::new();
    if let Some(obj) = value.as_object().filter(|_| unsafe { qjs::JS_IsError(value.as_raw()) }) {
        // Cut before copying: the message of a thrown error can be any size.
        let get = |k: &str| match obj.get::<_, Value>(k) {
            Ok(v) if v.is_string() => {
                let (mut s, cut) = clip(ctx, &v, MAX_ERROR_TEXT);
                if cut {
                    s.push_str("… (truncated)");
                }
                Some(s)
            }
            Ok(_) => None,
            Err(_) => {
                let _ = ctx.catch();
                None
            }
        };
        event.insert("name".into(), get("name").unwrap_or_else(|| "Error".into()).into());
        event.insert("message".into(), get("message").unwrap_or_default().into());
        if let Some(stack) = get("stack") {
            event.insert("stack".into(), stack.into());
        }
        if let Some(code) = get("code") {
            event.insert("code".into(), code.into());
        }
    } else if value.is_null() && at_memory_limit(ctx) {
        // Out of memory with no room left even for the error object.
        event.insert("name".into(), "InternalError".into());
        event.insert("message".into(), "out of memory".into());
    } else {
        event.insert("name".into(), "Uncaught".into());
        event.insert("message".into(), inspect::inspect(ctx, value, &h, false).into());
    }
    send(EV_ERROR, Json::Object(event).to_string().as_bytes());
    ST_ERROR
}
