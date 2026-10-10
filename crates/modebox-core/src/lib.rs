//! # modebox
//!
//! Run untrusted, agent-written JavaScript/TypeScript in a WebAssembly
//! sandbox whose only authority is the host functions you hand it.
//!
//! - **Isolation**: QuickJS-ng compiled to wasm, run by wasmtime. The guest
//!   imports exactly two functions (an event channel and a clock), with no WASI,
//!   files, network or env.
//! - **Fast starts**: the engine is pre-initialized once (wizer) and each run
//!   instantiates that image copy-on-write.
//! - **True continuations**: a run can suspend on an approval or interrupt and
//!   be resumed later, even in another process, from an encrypted snapshot of
//!   the whole VM. Nothing re-executes and side effects never repeat.
//! - **Deterministic metering**: optional fuel accounting, seeded randomness
//!   and virtual time.
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use modebox_core::{Engine, RunOptions, Toolbox, Value};
//!
//! let engine = Engine::new()?;
//! let mut tools = Toolbox::new();
//! tools.func("math.double", |call: modebox_core::Call| async move {
//!     Ok(Value::from(call.input().as_f64().unwrap_or(0.0) * 2.0))
//! });
//! let result = engine.run("await math.double(21)", &tools, RunOptions::default()).await;
//! assert_eq!(result.value(), Some(&Value::from(42)));
//! # Ok(()) }
//! ```

mod dts;
mod engine;
mod run;
mod session;
mod snapshot;
mod tools;
mod ts;
mod value;

pub use dts::declarations;
pub use engine::{Engine, EngineConfig};
pub use run::{
    ClockMode, ErrorKind, Event, EventSink, Interrupt, Limits, LogEntry, Mode, Outcome, Resolution, RunError,
    RunOptions, RunResult, Stats, Suspension,
};
pub use session::Session;
pub use tools::{Approval, ApprovalFn, Call, Extension, Handler, Tool, ToolError, ToolResult, Toolbox};
pub use value::{ErrorValue, Value};
