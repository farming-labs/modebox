//! The host event loop: start the guest, execute the host calls it emits,
//! settle results back in, and suspend into a sealed snapshot when a call
//! needs a decision from outside.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use tokio::task::AbortHandle;

use crate::engine::{
    Clock, EV_CALL, EV_CANCEL, EV_DONE, EV_ERROR, EV_LOG, Engine, EngineInner, Guest, HostCallLimit, ST_DONE,
    now_epoch_us,
};
use crate::snapshot;
use crate::tools::{Call, Tool, ToolError, ToolResult, Toolbox};
use crate::value::Value;

// ---- public types ---------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limits {
    /// Wall-clock budget for one segment of the run (start → finish/suspend,
    /// or resume → finish/suspend).
    #[serde(with = "duration_ms")]
    pub timeout: Duration,
    /// Deterministic CPU budget in wasm fuel units, cumulative across
    /// resumes. Requires `EngineConfig::metering`.
    pub fuel: Option<u64>,
    /// QuickJS heap limit in bytes.
    pub memory_bytes: usize,
    /// Maximum host calls per run (timers excluded), cumulative across resumes.
    pub max_host_calls: u32,
    /// Host calls executing at once; extra calls queue instead of failing.
    pub max_concurrency: usize,
    /// Largest value allowed across the boundary in either direction.
    pub max_value_bytes: usize,
    /// Console output kept per run; the rest is counted and dropped.
    pub max_log_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            timeout: Duration::from_secs(30),
            fuel: None,
            memory_bytes: 64 << 20,
            max_host_calls: 1000,
            max_concurrency: 32,
            max_value_bytes: 16 << 20,
            max_log_bytes: 1 << 20,
        }
    }
}

mod duration_ms {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;
    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis() as u64)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_millis(u64::deserialize(d)?))
    }
}

/// How code is evaluated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Script mode unless the code uses a top-level `return`.
    #[default]
    Auto,
    /// Global script with top-level await. The value of the last expression
    /// is the result, and declarations persist across session cells.
    Script,
    /// Async function body: the result is whatever is `return`ed.
    Function,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum ClockMode {
    /// Wall-clock time; timers really wait.
    #[default]
    Real,
    /// Deterministic virtual time starting at `start_ms`; timers fire
    /// instantly in order and advance the clock. Same seed + same tool results
    /// = same execution.
    Virtual { start_ms: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum Event {
    Log(LogEntry),
    CallStarted { id: u32, tool: String, args: Vec<Value> },
    CallFinished { id: u32, tool: String, ok: bool, duration: Duration },
    Interrupted { id: u32, tool: String, kind: String },
}

pub type EventSink = Arc<dyn Fn(&Event) + Send + Sync>;

#[derive(Clone)]
pub struct RunOptions {
    pub limits: Limits,
    pub mode: Mode,
    /// Shown in stack traces.
    pub filename: String,
    /// Strip TypeScript syntax before running.
    pub typescript: bool,
    /// Seed for `Math.random` and `crypto`; fresh random bytes if `None`.
    pub seed: Option<[u8; 32]>,
    pub clock: ClockMode,
    /// Streaming callback for logs and tool-call progress.
    pub on_event: Option<EventSink>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            limits: Limits::default(),
            mode: Mode::Auto,
            filename: "agent.ts".into(),
            typescript: true,
            seed: None,
            clock: ClockMode::Real,
            on_event: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunResult {
    pub outcome: Outcome,
    pub logs: Vec<LogEntry>,
    pub stats: Stats,
}

impl RunResult {
    pub fn value(&self) -> Option<&Value> {
        match &self.outcome {
            Outcome::Completed(v) => Some(v),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&RunError> {
        match &self.outcome {
            Outcome::Failed(e) => Some(e),
            _ => None,
        }
    }

    pub fn suspension(&self) -> Option<&Suspension> {
        match &self.outcome {
            Outcome::Suspended(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Completed(Value),
    Failed(RunError),
    /// Paused on one or more interrupts. Resume with [`Engine::resume`].
    Suspended(Suspension),
}

#[derive(Clone, Debug)]
pub struct Suspension {
    pub interrupts: Vec<Interrupt>,
    /// Sealed snapshot of the whole VM. Opaque, encrypted, single-use.
    pub continuation: String,
}

#[derive(Clone, Debug)]
pub struct Interrupt {
    /// Call id; key the matching [`Resolution`] by it.
    pub id: u32,
    pub tool: String,
    pub args: Vec<Value>,
    /// `"approval"` for policy-driven pauses, or whatever a tool passed to
    /// [`Call::interrupt`].
    pub kind: String,
    pub payload: Value,
}

/// What a continuation token is waiting on, read without using it up (see
/// [`Engine::inspect_continuation`]).
#[derive(Clone, Debug)]
pub struct ContinuationInfo {
    pub run_id: String,
    /// Unix ms after which the token is refused.
    pub expires_ms: u64,
    /// The calls to resolve, one [`Resolution`] each.
    pub interrupts: Vec<Interrupt>,
}

/// How to continue an interrupted call.
#[derive(Clone, Debug)]
pub enum Resolution {
    /// Run the tool now (for approval interrupts).
    Approve,
    /// Re-invoke the tool with [`Call::resume`] set to this value.
    ResumeWith(Value),
    /// Reject the call inside the sandbox with a `PermissionDenied` error.
    Deny(String),
    /// Settle the call with this value without invoking the tool.
    Return(Value),
    /// Settle the call with this error without invoking the tool.
    Throw(ToolError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The code didn't parse.
    Syntax,
    /// The code threw (or rejected) and nothing caught it.
    Uncaught,
    /// The script awaited something that can never settle.
    Deadlock,
    Timeout,
    CpuLimit,
    MemoryLimit,
    StackOverflow,
    HostCallLimit,
    /// The result couldn't cross the boundary (functions, symbols, too large).
    Unserializable,
    InvalidContinuation,
    Internal,
}

#[derive(Clone, Debug)]
pub struct RunError {
    pub kind: ErrorKind,
    pub name: String,
    pub message: String,
    pub stack: Option<String>,
}

impl RunError {
    fn new(kind: ErrorKind, name: &str, message: impl Into<String>) -> Self {
        RunError { kind, name: name.into(), message: message.into(), stack: None }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)?;
        if let Some(stack) = &self.stack {
            write!(f, "\n{}", stack.trim_end())?;
        }
        Ok(())
    }
}

impl std::error::Error for RunError {}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Wall time of this segment.
    pub wall: Duration,
    /// Time spent executing guest code.
    pub guest: Duration,
    /// Time to create (or restore) the sandbox instance.
    pub instantiate: Duration,
    /// Fuel consumed so far, cumulative across resumes.
    pub fuel_used: Option<u64>,
    /// Host calls dispatched so far (timers excluded), cumulative.
    pub host_calls: u32,
    /// Size of the guest's linear memory.
    pub memory_bytes: usize,
    /// Size of the continuation token, when suspended.
    pub continuation_bytes: Option<usize>,
    pub dropped_logs: u32,
    /// The instance was stopped mid-execution (timeout, CPU or memory trap);
    /// its VM state can't be trusted afterwards.
    pub aborted: bool,
}

// ---- continuation state ---------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingCall {
    id: u32,
    name: String,
    /// Tagged-JSON argument array.
    args: Json,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payload: Option<Json>,
    /// Timers: when they fire (epoch µs, or virtual µs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due_us: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<Json>,
}

impl PendingCall {
    fn args(&self) -> Vec<Value> {
        match Value::from_tagged(&self.args) {
            Value::Array(a) => a,
            other => vec![other],
        }
    }

    fn interrupt(&self) -> Interrupt {
        Interrupt {
            id: self.id,
            tool: self.name.clone(),
            args: self.args(),
            kind: self.kind.clone().unwrap_or_else(|| "interrupt".into()),
            payload: self.payload.as_ref().map(Value::from_tagged).unwrap_or(Value::Undefined),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    v: u32,
    image: String,
    scope: String,
    run_id: String,
    nonce: String,
    created_ms: u64,
    expires_ms: u64,
    limits: Limits,
    clock: Option<f64>,
    fuel_used: u64,
    calls_made: u32,
    calls_emitted: u32,
    log_bytes: usize,
    interrupts: Vec<PendingCall>,
    deferred: Vec<PendingCall>,
    /// Extension state (e.g. staged filesystem writes), base64 by name.
    #[serde(default)]
    ext: BTreeMap<String, String>,
    /// Source tools called so far (data-flow checks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sources: Vec<String>,
}

const SLEEP: &str = "$sleep";

// ---- engine entry points ------------------------------------------------------------

impl Engine {
    /// Run `code` with access to `tools` and nothing else.
    pub async fn run(&self, code: &str, tools: &Toolbox, opts: RunOptions) -> RunResult {
        let t0 = Instant::now();
        let clock = match opts.clock {
            ClockMode::Real => Clock::Real,
            ClockMode::Virtual { start_ms } => Clock::Virtual(start_ms * 1000.0),
        };
        let guest = match Guest::new(&self.inner, clock, guest_memory_cap(&self.inner, &opts.limits)).await {
            Ok(g) => g,
            Err(e) => return fail(RunError::new(ErrorKind::Internal, "ModeboxError", format!("{e:#}"))),
        };
        self.run_on(guest, code, tools, opts, t0, false).await.0
    }

    /// Evaluate `code` in an existing instance (fresh, or a session's).
    /// `repl`: make top-level `const`/`let` re-declarable (session cells).
    pub(crate) async fn run_on(
        &self,
        guest: Guest,
        code: &str,
        tools: &Toolbox,
        opts: RunOptions,
        t0: Instant,
        repl: bool,
    ) -> (RunResult, Option<Guest>) {
        let mut js = if opts.typescript {
            match crate::ts::strip(code, &opts.filename) {
                Ok(js) => js,
                Err(msg) => return (fail(RunError::new(ErrorKind::Syntax, "SyntaxError", msg)), Some(guest)),
            }
        } else {
            code.to_owned()
        };
        if repl && opts.mode != Mode::Function {
            js = crate::ts::redeclarable(&js);
        }
        let instantiate = t0.elapsed();
        let seed = opts.seed.unwrap_or_else(rand::random);
        let request = json!({
            "code": js,
            "mode": opts.mode,
            "fns": tools.names().collect::<Vec<_>>(),
            "seed": hex(&seed),
            "memoryLimit": opts.limits.memory_bytes,
            "maxValueBytes": opts.limits.max_value_bytes,
            "filename": opts.filename,
        });
        let run_id = hex(&rand::random::<[u8; 8]>());
        let mut d = Driver::new(&self.inner, tools, opts, guest, run_id, t0);
        d.stats.instantiate = instantiate;
        d.configure_store(0, 0, 0);
        let status = d.guest_call(GuestOp::Start(serde_json::to_vec(&request).unwrap())).await;
        let (r, g) = d.drive(status).await;
        (r, Some(g))
    }

    /// The calls a continuation token is waiting on, so each can be shown and
    /// decided on its own. Checks the token like a resume does (it must open
    /// with this engine's key and image), but doesn't use it up.
    pub fn inspect_continuation(&self, continuation: &str) -> Result<ContinuationInfo, RunError> {
        let invalid = |m: String| RunError::new(ErrorKind::InvalidContinuation, "ContinuationError", m);
        let blob = self.inner.sealer.open(continuation).map_err(invalid)?;
        let (header, _): (Header, snapshot::MemDiff) = snapshot::unpack(&blob).map_err(invalid)?;
        if header.v != 1 || header.image != self.inner.image_id {
            return Err(invalid("continuation was created by a different sandbox image".into()));
        }
        Ok(ContinuationInfo {
            run_id: header.run_id,
            expires_ms: header.expires_ms,
            interrupts: header.interrupts.iter().map(PendingCall::interrupt).collect(),
        })
    }

    /// Continue a suspended run. Every interrupt in the suspension needs a
    /// resolution; the rest of the VM picks up exactly where it stopped.
    pub async fn resume(
        &self,
        continuation: &str,
        resolutions: impl IntoIterator<Item = (u32, Resolution)>,
        tools: &Toolbox,
        opts: RunOptions,
    ) -> RunResult {
        self.resume_inner(continuation, resolutions, tools, opts).await.0
    }

    pub(crate) async fn resume_inner(
        &self,
        continuation: &str,
        resolutions: impl IntoIterator<Item = (u32, Resolution)>,
        tools: &Toolbox,
        opts: RunOptions,
    ) -> (RunResult, Option<Guest>) {
        let t0 = Instant::now();
        let invalid = |m: String| (fail(RunError::new(ErrorKind::InvalidContinuation, "ContinuationError", m)), None);
        let inner = &self.inner;

        let blob = match inner.sealer.open(continuation) {
            Ok(b) => b,
            Err(m) => return invalid(m),
        };
        let (header, diff): (Header, snapshot::MemDiff) = match snapshot::unpack(&blob) {
            Ok(x) => x,
            Err(m) => return invalid(m),
        };
        if header.v != 1 || header.image != inner.image_id {
            return invalid("continuation was created by a different sandbox image".into());
        }
        if header.scope != tools.scope_hash() {
            return invalid("continuation was created with a different set of tools".into());
        }
        let now_ms = (now_epoch_us() / 1000.0) as u64;
        if now_ms <= header.expires_ms
            && let Err(m) = tools.load_extensions(&header.ext)
        {
            return invalid(m);
        }
        if now_ms > header.expires_ms {
            return invalid("continuation has expired".into());
        }
        if inner.config.single_use_continuations {
            match inner.config.continuations.claim(&header.nonce, header.expires_ms).await {
                Ok(true) => {}
                Ok(false) => return invalid("continuation was already used".into()),
                Err(m) => return invalid(m),
            }
        }

        let clock = match header.clock {
            Some(t) => Clock::Virtual(t),
            None => Clock::Real,
        };
        // Limits are part of the sealed state: a resume can't widen them.
        let opts = RunOptions { limits: header.limits.clone(), ..opts };
        let mut guest = match Guest::restore(inner, &diff, clock, guest_memory_cap(inner, &opts.limits)).await {
            Ok(g) => g,
            Err(e) => return (fail(RunError::new(ErrorKind::Internal, "ModeboxError", format!("{e:#}"))), None),
        };
        guest.store.data_mut().sources = header.sources.iter().cloned().collect();
        let mut d = Driver::new(inner, tools, opts, guest, header.run_id.clone(), t0);
        d.stats.instantiate = t0.elapsed();
        d.calls_made = header.calls_made;
        d.configure_store(header.fuel_used, header.calls_emitted, header.log_bytes);

        let mut resolutions: HashMap<u32, Resolution> = resolutions.into_iter().collect();
        for p in &header.interrupts {
            if !resolutions.contains_key(&p.id) {
                return invalid(format!("no resolution for interrupt {} ({})", p.id, p.name));
            }
        }
        for mut p in header.interrupts {
            match resolutions.remove(&p.id).unwrap() {
                Resolution::Approve => d.start_call(p),
                Resolution::ResumeWith(v) => {
                    p.resume = Some(v.to_tagged());
                    d.start_call(p)
                }
                Resolution::Deny(reason) => d.settle_err(
                    p.id,
                    &ToolError::new(reason).with_name("PermissionDenied").with_code("PERMISSION_DENIED"),
                ),
                Resolution::Return(v) => d.settle_ok(p.id, &v),
                Resolution::Throw(e) => d.settle_err(p.id, &e),
            }
        }
        for p in header.deferred {
            d.dispatch(p, false);
        }
        let status = if d.settle.is_empty() {
            Ok(crate::engine::ST_WAITING)
        } else {
            let batch = d.take_settle_batch();
            d.guest_call(GuestOp::Settle(batch)).await
        };
        let (r, g) = d.drive(status).await;
        (r, Some(g))
    }
}

pub(crate) fn fail(e: RunError) -> RunResult {
    RunResult { outcome: Outcome::Failed(e), logs: vec![], stats: Stats::default() }
}

pub(crate) fn guest_memory_cap(inner: &EngineInner, limits: &Limits) -> usize {
    // QuickJS's own limit trips first (a catchable error); this is the backstop.
    (inner.base.len() + limits.memory_bytes + (32 << 20)).min(inner.config.max_memory_bytes)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- driver ---------------------------------------------------------------------------

type Inflight = BoxFuture<'static, (u32, ToolResult)>;

enum GuestOp {
    Start(Vec<u8>),
    Settle(Vec<u8>),
}

pub(crate) struct Driver<'a> {
    engine: &'a EngineInner,
    tools: &'a Toolbox,
    opts: RunOptions,
    pub(crate) guest: Guest,
    run_id: String,
    started: Instant,
    deadline: Instant,
    inflight: FuturesUnordered<Inflight>,
    inflight_meta: HashMap<u32, (PendingCall, Instant)>,
    aborts: Vec<AbortHandle>,
    /// (due µs as bits, id) → timer
    timers: BTreeMap<(u64, u32), PendingCall>,
    queued: VecDeque<PendingCall>,
    interrupts: Vec<PendingCall>,
    deferred: Vec<PendingCall>,
    settle: Vec<Json>,
    logs: Vec<LogEntry>,
    calls_made: u32,
    fuel_start: u64,
    fuel_used_before: u64,
    outcome: Option<Outcome>,
    stats: Stats,
}

impl<'a> Driver<'a> {
    fn new(
        engine: &'a EngineInner,
        tools: &'a Toolbox,
        opts: RunOptions,
        guest: Guest,
        run_id: String,
        started: Instant,
    ) -> Self {
        let deadline = started + opts.limits.timeout;
        Driver {
            engine,
            tools,
            opts,
            guest,
            run_id,
            started,
            deadline,
            inflight: FuturesUnordered::new(),
            inflight_meta: HashMap::new(),
            aborts: Vec::new(),
            timers: BTreeMap::new(),
            queued: VecDeque::new(),
            interrupts: Vec::new(),
            deferred: Vec::new(),
            settle: Vec::new(),
            logs: Vec::new(),
            calls_made: 0,
            fuel_start: 0,
            fuel_used_before: 0,
            outcome: None,
            stats: Stats::default(),
        }
    }

    fn configure_store(&mut self, fuel_used_before: u64, calls_emitted: u32, log_bytes: usize) {
        let limits = &self.opts.limits;
        let st = self.guest.store.data_mut();
        // Timers also go through `emit`; give them headroom so the cap is
        // about tool calls. The precise cap is enforced in `dispatch`.
        st.max_calls = limits.max_host_calls.saturating_mul(4).saturating_add(10_000);
        st.calls_emitted = calls_emitted;
        st.max_log_bytes = limits.max_log_bytes;
        st.log_bytes = log_bytes;
        self.fuel_used_before = fuel_used_before;
        if self.engine.config.metering {
            let budget = limits.fuel.map(|f| f.saturating_sub(fuel_used_before)).unwrap_or(u64::MAX);
            self.guest.set_fuel(budget);
            self.fuel_start = budget;
        }
    }

    fn fuel_used(&self) -> Option<u64> {
        if !self.engine.config.metering {
            return None;
        }
        let left = self.guest.fuel_left().unwrap_or(0);
        Some(self.fuel_used_before + self.fuel_start.saturating_sub(left))
    }

    fn emit_event(&self, e: Event) {
        if let Some(sink) = &self.opts.on_event {
            sink(&e);
        }
    }

    /// Call into the guest with the wall-clock deadline armed, mapping traps
    /// to run errors.
    async fn guest_call(&mut self, op: GuestOp) -> Result<u32, RunError> {
        if Instant::now() >= self.deadline {
            return Err(self.timeout_error());
        }
        self.guest.set_deadline(self.deadline);
        let t = Instant::now();
        let r = match op {
            GuestOp::Start(req) => self.guest.start(&req).await,
            GuestOp::Settle(batch) => self.guest.settle(&batch).await,
        };
        self.stats.guest += t.elapsed();
        r.map_err(|e| {
            self.stats.aborted = true;
            self.trap_error(e)
        })
    }

    fn timeout_error(&self) -> RunError {
        RunError::new(
            ErrorKind::Timeout,
            "TimeoutError",
            format!("run exceeded its {:?} time limit", self.opts.limits.timeout),
        )
    }

    fn trap_error(&self, e: wasmtime::Error) -> RunError {
        if e.downcast_ref::<HostCallLimit>().is_some() {
            return self.call_limit_error();
        }
        match e.downcast_ref::<wasmtime::Trap>() {
            Some(wasmtime::Trap::OutOfFuel) => RunError::new(
                ErrorKind::CpuLimit,
                "CpuLimitError",
                format!("run exceeded its CPU budget of {} fuel", self.opts.limits.fuel.unwrap_or(0)),
            ),
            Some(wasmtime::Trap::Interrupt) => self.timeout_error(),
            Some(wasmtime::Trap::StackOverflow) => {
                RunError::new(ErrorKind::StackOverflow, "RangeError", "Maximum call stack size exceeded")
            }
            Some(wasmtime::Trap::MemoryOutOfBounds) | Some(wasmtime::Trap::UnreachableCodeReached)
                if self.guest.memory_size() + (1 << 20) >= guest_memory_cap(self.engine, &self.opts.limits) =>
            {
                RunError::new(ErrorKind::MemoryLimit, "MemoryLimitError", "run exceeded its memory limit")
            }
            Some(trap) => RunError::new(ErrorKind::Internal, "SandboxTrap", format!("the sandbox crashed: {trap}")),
            None => RunError::new(ErrorKind::Internal, "SandboxTrap", format!("{:#}", anyhow::Error::from(e))),
        }
    }

    fn call_limit_error(&self) -> RunError {
        RunError::new(
            ErrorKind::HostCallLimit,
            "HostCallLimitError",
            format!("run exceeded its limit of {} host calls", self.opts.limits.max_host_calls),
        )
    }

    /// Run the event loop to an outcome, then hand back the instance (still
    /// holding the VM state) so sessions can keep using it.
    async fn drive(mut self, first: Result<u32, RunError>) -> (RunResult, Guest) {
        let result = self.drive_inner(first).await;
        // Calls the script never awaited don't outlive it.
        for a in &self.aborts {
            a.abort();
        }
        (result, self.guest)
    }

    async fn drive_inner(&mut self, first: Result<u32, RunError>) -> RunResult {
        let mut status = first;
        loop {
            let st = match status {
                Ok(s) => s,
                Err(e) => return self.finish(Outcome::Failed(e)),
            };
            if let Err(e) = self.process_events() {
                return self.finish(Outcome::Failed(e));
            }
            if let Some(o) = self.outcome.take() {
                return self.finish(o);
            }
            if st == ST_DONE {
                // Done without an outcome event can't happen; be defensive.
                return self.finish(Outcome::Completed(Value::Undefined));
            }
            self.pump_queue();
            // Results that are already known (unknown tool, cancelled timer)
            // go straight back in.
            if self.settle.is_empty() {
                if !self.interrupts.is_empty() && self.inflight.is_empty() {
                    return self.suspend().await;
                }
                if self.inflight.is_empty() && self.timers.is_empty() && self.queued.is_empty() {
                    return self.finish(Outcome::Failed(RunError::new(
                        ErrorKind::Internal,
                        "ModeboxError",
                        "the sandbox is waiting on host calls the host doesn't know about",
                    )));
                }
                if let Err(e) = self.wait().await {
                    return self.finish(Outcome::Failed(e));
                }
                if self.settle.is_empty() {
                    // e.g. a tool raised an interrupt; loop to re-check.
                    status = Ok(crate::engine::ST_WAITING);
                    continue;
                }
            }
            let batch = self.take_settle_batch();
            status = self.guest_call(GuestOp::Settle(batch)).await;
        }
    }

    fn take_settle_batch(&mut self) -> Vec<u8> {
        let batch = Json::Array(std::mem::take(&mut self.settle));
        serde_json::to_vec(&batch).unwrap()
    }

    fn process_events(&mut self) -> Result<(), RunError> {
        for ev in self.guest.take_events() {
            let msg: Json = serde_json::from_slice(&ev.data).unwrap_or(Json::Null);
            match ev.kind {
                EV_CALL => {
                    let call = PendingCall {
                        id: msg["id"].as_u64().unwrap_or(0) as u32,
                        name: msg["fn"].as_str().unwrap_or("").to_owned(),
                        args: msg["args"].clone(),
                        kind: None,
                        payload: None,
                        due_us: None,
                        resume: None,
                    };
                    if call.name != SLEEP {
                        self.calls_made += 1;
                        if self.calls_made > self.opts.limits.max_host_calls {
                            return Err(self.call_limit_error());
                        }
                    }
                    self.dispatch(call, true);
                }
                EV_LOG => {
                    let entry = LogEntry {
                        level: msg["level"].as_str().unwrap_or("log").to_owned(),
                        message: msg["msg"].as_str().unwrap_or("").to_owned(),
                    };
                    self.emit_event(Event::Log(entry.clone()));
                    self.logs.push(entry);
                }
                EV_DONE => {
                    self.outcome = Some(Outcome::Completed(Value::from_tagged(&msg["value"])));
                }
                EV_ERROR => {
                    self.outcome = Some(Outcome::Failed(classify(&msg)));
                }
                EV_CANCEL => {
                    let id = msg["id"].as_u64().unwrap_or(0) as u32;
                    self.cancel(id);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Route a call the guest just made (or a deferred one being replayed
    /// into a resumed run).
    fn dispatch(&mut self, mut call: PendingCall, fresh: bool) {
        if call.name == SLEEP {
            let due = match call.due_us {
                Some(d) => d,
                None => {
                    let ms = Value::from_tagged(&call.args)
                        .get_index(0)
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                        .max(0.0);
                    self.guest.store.data().clock.now_us() + ms * 1000.0
                }
            };
            call.due_us = Some(due);
            if !self.interrupts.is_empty() {
                self.deferred.push(call);
            } else {
                self.timers.insert((due.to_bits(), call.id), call);
            }
            return;
        }
        let Some(tool) = self.tools.get(&call.name) else {
            self.settle_err(call.id, &ToolError::new(format!("{} is not a tool", call.name)).with_name("ReferenceError"));
            return;
        };
        let probe = Call {
            name: call.name.clone(),
            args: call.args(),
            id: call.id,
            run_id: self.run_id.clone(),
            resume: None,
        };
        if let Some(reason) = tool.needs_approval(&probe).or_else(|| self.flow_check(tool)) {
            call.kind = Some("approval".into());
            call.payload = Some(Value::object([("reason", Value::from(reason))]).to_tagged());
            // Approvals raised while already suspending join the same batch,
            // so the user answers them together instead of one per resume.
            self.emit_event(Event::Interrupted { id: call.id, tool: call.name.clone(), kind: "approval".into() });
            self.interrupts.push(call);
            return;
        }
        let _ = fresh;
        if !self.interrupts.is_empty() {
            // Suspending: don't start new work; it runs after resume.
            self.deferred.push(call);
            return;
        }
        if self.inflight.len() >= self.opts.limits.max_concurrency {
            self.queued.push_back(call);
            return;
        }
        self.start_call(call);
    }

    fn pump_queue(&mut self) {
        while self.inflight.len() < self.opts.limits.max_concurrency && self.interrupts.is_empty() {
            let Some(call) = self.queued.pop_front() else { break };
            self.start_call(call);
        }
        if !self.interrupts.is_empty() {
            self.deferred.extend(self.queued.drain(..));
        }
    }

    /// A sink call after a source call needs approval: private data could
    /// be on its way out.
    fn flow_check(&self, tool: &Tool) -> Option<String> {
        let read = &self.guest.store.data().sources;
        if !tool.sink || read.is_empty() {
            return None;
        }
        let mut names: Vec<&str> = read.iter().map(String::as_str).take(3).collect();
        if read.len() > 3 {
            names.push("…");
        }
        Some(format!("{} can send data out, and this run has read private data from {}", tool.name, names.join(", ")))
    }

    fn start_call(&mut self, call: PendingCall) {
        let Some(tool) = self.tools.get(&call.name) else {
            self.settle_err(call.id, &ToolError::new(format!("{} is not a tool", call.name)).with_name("ReferenceError"));
            return;
        };
        if tool.source {
            self.guest.store.data_mut().sources.insert(tool.name.clone());
        }
        let args = call.args();
        self.emit_event(Event::CallStarted { id: call.id, tool: call.name.clone(), args: args.clone() });
        let c = Call {
            name: call.name.clone(),
            args,
            id: call.id,
            run_id: self.run_id.clone(),
            resume: call.resume.as_ref().map(Value::from_tagged),
        };
        let handle = tokio::spawn(tool.handler.call(c));
        self.aborts.push(handle.abort_handle());
        let id = call.id;
        self.inflight.push(
            async move {
                let r = match handle.await {
                    Ok(r) => r,
                    Err(e) => Err(ToolError::new(format!("tool task failed: {e}"))),
                };
                (id, r)
            }
            .boxed(),
        );
        self.inflight_meta.insert(id, (call, Instant::now()));
    }

    fn complete(&mut self, id: u32, result: ToolResult) {
        let Some((call, started)) = self.inflight_meta.remove(&id) else { return };
        match result {
            Err(ToolError { interrupt: Some((kind, payload)), .. }) => {
                self.emit_event(Event::Interrupted { id, tool: call.name.clone(), kind: kind.clone() });
                self.interrupts.push(PendingCall {
                    kind: Some(kind),
                    payload: Some(payload.to_tagged()),
                    resume: None,
                    ..call
                });
            }
            Ok(v) => {
                self.emit_event(Event::CallFinished { id, tool: call.name, ok: true, duration: started.elapsed() });
                self.settle_ok(id, &v);
            }
            Err(e) => {
                self.emit_event(Event::CallFinished { id, tool: call.name, ok: false, duration: started.elapsed() });
                self.settle_err(id, &e);
            }
        }
    }

    fn settle_ok(&mut self, id: u32, v: &Value) {
        let tagged = v.to_tagged();
        let size = serde_json::to_vec(&tagged).map(|b| b.len()).unwrap_or(usize::MAX);
        if size > self.opts.limits.max_value_bytes {
            let msg = format!(
                "tool result is {size} bytes, over the {} byte limit",
                self.opts.limits.max_value_bytes
            );
            self.settle_err(id, &ToolError::new(msg).with_name("RangeError"));
            return;
        }
        self.settle.push(json!({ "id": id, "ok": true, "v": tagged }));
    }

    fn settle_err(&mut self, id: u32, e: &ToolError) {
        let mut err = serde_json::Map::new();
        err.insert("name".into(), e.name.clone().into());
        err.insert("message".into(), e.message.clone().into());
        if let Some(code) = &e.code {
            err.insert("code".into(), code.clone().into());
        }
        if let Some(data) = &e.data {
            err.insert("data".into(), data.to_tagged());
        }
        self.settle.push(json!({ "id": id, "ok": false, "e": err }));
    }

    fn cancel(&mut self, id: u32) {
        let key = self.timers.keys().find(|(_, tid)| *tid == id).copied();
        if let Some(key) = key {
            self.timers.remove(&key);
            self.settle_ok(id, &Value::Undefined);
        } else if let Some(pos) = self.deferred.iter().position(|c| c.id == id) {
            self.deferred.remove(pos);
            self.settle_ok(id, &Value::Undefined);
        }
    }

    /// Wait for at least one host call or timer to finish.
    async fn wait(&mut self) -> Result<(), RunError> {
        if let Clock::Virtual(_) = self.guest.store.data().clock
            && let Some(&(due_bits, _)) = self.timers.keys().next() {
                self.fire_timers_until(f64::from_bits(due_bits));
                let due = f64::from_bits(due_bits);
                self.guest.store.data_mut().clock = Clock::Virtual(due);
                return Ok(());
            }
        let timer_wait = self.timers.keys().next().map(|&(due_bits, _)| {
            let delta_us = (f64::from_bits(due_bits) - now_epoch_us()).max(0.0);
            Duration::from_micros(delta_us as u64)
        });
        let has_timer = timer_wait.is_some();
        let timer_sleep = tokio::time::sleep(timer_wait.unwrap_or(Duration::from_secs(3600)));
        let has_inflight = !self.inflight.is_empty();
        tokio::select! {
            Some((id, r)) = self.inflight.next(), if has_inflight => {
                self.complete(id, r);
                while let Some(Some((id, r))) = self.inflight.next().now_or_never() {
                    self.complete(id, r);
                }
            }
            _ = timer_sleep, if has_timer => {
                self.fire_timers_until(now_epoch_us());
            }
            _ = tokio::time::sleep_until(self.deadline.into()) => {
                return Err(self.timeout_error());
            }
        }
        Ok(())
    }

    fn fire_timers_until(&mut self, now_us: f64) {
        let due: Vec<(u64, u32)> =
            self.timers.keys().take_while(|(bits, _)| f64::from_bits(*bits) <= now_us).copied().collect();
        for key in due {
            self.timers.remove(&key);
            self.settle_ok(key.1, &Value::Undefined);
        }
    }

    async fn suspend(&mut self) -> RunResult {
        self.deferred.extend(self.queued.drain(..));
        self.deferred.extend(std::mem::take(&mut self.timers).into_values());
        let st = self.guest.store.data();
        let now_ms = (now_epoch_us() / 1000.0) as u64;
        let header = Header {
            v: 1,
            image: self.engine.image_id.clone(),
            scope: self.tools.scope_hash(),
            run_id: self.run_id.clone(),
            nonce: hex(&rand::random::<[u8; 16]>()),
            created_ms: now_ms,
            expires_ms: now_ms + self.engine.config.continuation_ttl.as_millis() as u64,
            limits: self.opts.limits.clone(),
            clock: match st.clock {
                Clock::Virtual(t) => Some(t),
                Clock::Real => None,
            },
            fuel_used: self.fuel_used().unwrap_or(0),
            calls_made: self.calls_made,
            calls_emitted: st.calls_emitted,
            log_bytes: st.log_bytes,
            interrupts: self.interrupts.clone(),
            deferred: self.deferred.clone(),
            ext: self.tools.save_extensions(),
            sources: st.sources.iter().cloned().collect(),
        };
        let diff = self.guest.snapshot(&self.engine.base).await;
        let blob = snapshot::pack(&header, &diff);
        let token = self.engine.sealer.seal(&blob);
        let interrupts = self.interrupts.iter().map(PendingCall::interrupt).collect();
        self.stats.continuation_bytes = Some(token.len());
        self.finish(Outcome::Suspended(Suspension { interrupts, continuation: token }))
    }

    fn finish(&mut self, outcome: Outcome) -> RunResult {
        self.stats.wall = self.started.elapsed();
        self.stats.fuel_used = self.fuel_used();
        self.stats.host_calls = self.calls_made;
        self.stats.memory_bytes = self.guest.memory_size();
        self.stats.dropped_logs = self.guest.store.data().dropped_logs;
        if self.stats.dropped_logs > 0 {
            self.logs.push(LogEntry {
                level: "warn".into(),
                message: format!(
                    "[modebox] {} console messages dropped (over the {} byte log limit)",
                    self.stats.dropped_logs, self.opts.limits.max_log_bytes
                ),
            });
        }
        RunResult { outcome, logs: std::mem::take(&mut self.logs), stats: std::mem::take(&mut self.stats) }
    }
}

fn classify(msg: &Json) -> RunError {
    let name = msg["name"].as_str().unwrap_or("Error").to_owned();
    let message = msg["message"].as_str().unwrap_or("").to_owned();
    let kind = match name.as_str() {
        "SyntaxError" if msg.get("stack").is_none_or(|s| !s.as_str().unwrap_or("").contains("at ")) => {
            ErrorKind::Syntax
        }
        "DeadlockError" => ErrorKind::Deadlock,
        "InternalError" if message.contains("out of memory") => ErrorKind::MemoryLimit,
        "RangeError" if message.contains("call stack") || message.contains("stack overflow") => {
            ErrorKind::StackOverflow
        }
        "TypeError" if message.starts_with("the script's result") => ErrorKind::Unserializable,
        _ => ErrorKind::Uncaught,
    };
    RunError { kind, name, message, stack: msg["stack"].as_str().map(str::to_owned).filter(|s| !s.is_empty()) }
}

impl Value {
    fn get_index(&self, i: usize) -> Option<&Value> {
        match self {
            Value::Array(a) => a.get(i),
            _ => None,
        }
    }
}
