//! The wasmtime side.
//!
//! At startup the QuickJS guest is pre-initialized with wizer (runtime,
//! context and prelude already built), compiled once, and cached on disk. Each
//! run then instantiates that image: with the pooling allocator and
//! copy-on-write memory this is microseconds, not the milliseconds it takes to
//! boot a JS engine.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow};
use wasmtime::{
    Caller, Config, InstanceAllocationStrategy, InstancePre, Linker, Memory, Module, PoolingAllocationConfig,
    Store, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

use crate::snapshot::{MemDiff, Sealer};

static GUEST_WASM: &[u8] = include_bytes!("../guest/modebox_guest.wasm");

pub(crate) const EV_CALL: u32 = 1;
pub(crate) const EV_LOG: u32 = 2;
pub(crate) const EV_DONE: u32 = 3;
pub(crate) const EV_ERROR: u32 = 4;
pub(crate) const EV_CANCEL: u32 = 5;

/// Epoch ticks per second (1 tick = 1 ms).
const TICK: Duration = Duration::from_millis(1);

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Count wasm fuel: deterministic CPU accounting and limits. Costs some
    /// throughput; wall-clock timeouts work either way.
    pub metering: bool,
    /// Use wasmtime's pooling allocator (fast instantiation, bounded memory).
    pub pooling: bool,
    /// Upper bound on concurrently live sandboxes (pool size).
    pub max_instances: u32,
    /// Hard cap on one sandbox's linear memory, in bytes.
    pub max_memory_bytes: usize,
    /// Where compiled code is cached between processes. `None` disables it.
    pub cache_dir: Option<PathBuf>,
    /// Key material for sealing continuation tokens (any length; 32+ random
    /// bytes recommended). Defaults to `$MODEBOX_SECRET`, else a random
    /// per-process key, which means tokens don't survive a restart.
    pub secret: Option<Vec<u8>>,
    /// How long a continuation token stays valid.
    pub continuation_ttl: Duration,
    /// Reject a continuation token that was already resumed once.
    pub single_use_continuations: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            metering: true,
            pooling: true,
            max_instances: 256,
            max_memory_bytes: 512 << 20,
            cache_dir: default_cache_dir(),
            secret: std::env::var("MODEBOX_SECRET").ok().map(String::into_bytes),
            continuation_ttl: Duration::from_secs(24 * 3600),
            single_use_continuations: true,
        }
    }
}

fn default_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MODEBOX_CACHE_DIR") {
        return Some(dir.into());
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".cache").join("modebox"))
}

/// A compiled, pre-initialized sandbox image. Cheap to clone; share one per process.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<EngineInner>,
}

pub(crate) struct EngineInner {
    pub wasm: wasmtime::Engine,
    pub pre: InstancePre<HostState>,
    /// Linear memory right after pre-initialization; snapshots diff against it.
    pub base: Vec<u8>,
    /// Hash of `base`. Continuations only resume on an identical image.
    pub image_id: String,
    pub config: EngineConfig,
    pub sealer: Sealer,
    /// Consumed continuation nonces -> expiry (ms), for single-use tokens.
    pub consumed: Mutex<HashMap<String, u64>>,
    stop_ticker: Arc<AtomicBool>,
    pub startup: Duration,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        self.stop_ticker.store(true, Ordering::Relaxed);
    }
}

impl Engine {
    pub fn new() -> Result<Engine> {
        Engine::with_config(EngineConfig::default())
    }

    pub fn with_config(config: EngineConfig) -> Result<Engine> {
        let t0 = Instant::now();
        let mut wc = Config::new();
        wc.consume_fuel(config.metering)
            .epoch_interruption(true)
            .max_wasm_stack(8 << 20)
            .async_stack_size(9 << 20)
            .memory_init_cow(true);
        if config.pooling {
            let mut pool = PoolingAllocationConfig::new();
            pool.total_memories(config.max_instances)
                .total_core_instances(config.max_instances)
                .total_tables(config.max_instances)
                .total_stacks(config.max_instances)
                .max_memory_size(config.max_memory_bytes)
                .linear_memory_keep_resident(256 << 10);
            wc.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        }
        let wasm = wasmtime::Engine::new(&wc).map_err(wt)?;
        let module = load_module(&wasm, config.cache_dir.as_deref())?;

        let mut linker: Linker<HostState> = Linker::new(&wasm);
        linker
            .func_wrap("modebox", "emit", host_emit)
            .map_err(wt)?
            .func_wrap("env", "__rquickjs_host_now_us", |caller: Caller<'_, HostState>| -> f64 {
                caller.data().clock.now_us()
            })
            .map_err(wt)?;
        let pre = linker.instantiate_pre(&module).map_err(wt)?;

        // Capture the base memory image for snapshot diffs.
        let mut store = Store::new(&wasm, HostState::new(config.max_memory_bytes, Clock::real()));
        store.set_epoch_deadline(u64::MAX / 2);
        if config.metering {
            store.set_fuel(u64::MAX).map_err(wt)?;
        }
        let instance = pre.instantiate(&mut store).map_err(wt)?;
        let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| anyhow!("guest has no memory"))?;
        let base = memory.data(&store).to_vec();
        let image_id = blake3::hash(&base).to_hex()[..32].to_owned();

        let secret = config.secret.clone().unwrap_or_else(|| rand::random::<[u8; 32]>().to_vec());
        let stop_ticker = Arc::new(AtomicBool::new(false));
        {
            let wasm = wasm.clone();
            let stop = stop_ticker.clone();
            std::thread::Builder::new()
                .name("modebox-epoch".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(TICK);
                        wasm.increment_epoch();
                    }
                })
                .context("spawning epoch thread")?;
        }

        Ok(Engine {
            inner: Arc::new(EngineInner {
                wasm,
                pre,
                base,
                image_id,
                sealer: Sealer::new(&secret),
                consumed: Mutex::new(HashMap::new()),
                stop_ticker,
                startup: t0.elapsed(),
                config,
            }),
        })
    }

    /// Time spent creating this engine (compile or cache load + pre-init).
    pub fn startup_time(&self) -> Duration {
        self.inner.startup
    }

    /// Identifier of the sandbox image; continuations are bound to it.
    pub fn image_id(&self) -> &str {
        &self.inner.image_id
    }

    pub fn config(&self) -> &EngineConfig {
        &self.inner.config
    }
}

/// wasmtime errors aren't `std::error::Error`; route them through anyhow.
pub(crate) fn wt(e: wasmtime::Error) -> anyhow::Error {
    anyhow::Error::from(e)
}

fn load_module(engine: &wasmtime::Engine, cache_dir: Option<&std::path::Path>) -> Result<Module> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut h);
    let key = format!("{}-{:016x}", &blake3::hash(GUEST_WASM).to_hex()[..24], h.finish());

    if let Some(dir) = cache_dir {
        let path = dir.join(format!("{key}.cwasm"));
        if path.exists() {
            // SAFETY: we wrote this file ourselves from a module compiled by a
            // compatible engine (the key covers the engine's compatibility hash).
            if let Ok(m) = unsafe { Module::deserialize_file(engine, &path) } {
                return Ok(m);
            }
        }
        let module = compile(engine)?;
        if std::fs::create_dir_all(dir).is_ok()
            && let Ok(bytes) = module.serialize() {
                let tmp = dir.join(format!("{key}.{}.tmp", std::process::id()));
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        return Ok(module);
    }
    compile(engine)
}

/// Pre-initialize the guest with wizer and compile the result.
fn compile(engine: &wasmtime::Engine) -> Result<Module> {
    let wizened = preinitialize(GUEST_WASM)?;
    Module::new(engine, &wizened).map_err(wt).context("compiling pre-initialized guest")
}

fn preinitialize(wasm: &[u8]) -> Result<Vec<u8>> {
    let engine = wasmtime::Engine::new(&Config::new()).map_err(wt)?;
    let mut store = Store::new(&engine, ());
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .func_wrap("modebox", "emit", |_: Caller<'_, ()>, _k: u32, _p: u32, _l: u32| {})
        .map_err(wt)?
        // Fixed clock during init keeps the image deterministic.
        .func_wrap("env", "__rquickjs_host_now_us", |_: Caller<'_, ()>| -> f64 { 0.0 })
        .map_err(wt)?;
    let mut wizer = wasmtime_wizer::Wizer::new();
    wizer.init_func("mb_init");
    let out = futures::executor::block_on(wizer.run(&mut store, wasm, async |store, module| {
        linker.instantiate_async(store, module).await
    }))
    .map_err(wt)
    .context("pre-initializing guest")?;
    Ok(out)
}

// ---- per-instance state ---------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub(crate) enum Clock {
    Real,
    /// Virtual microseconds; only advances when timers fire.
    Virtual(f64),
}

impl Clock {
    pub fn real() -> Clock {
        Clock::Real
    }

    pub fn now_us(&self) -> f64 {
        match self {
            Clock::Real => now_epoch_us(),
            Clock::Virtual(t) => *t,
        }
    }
}

pub(crate) fn now_epoch_us() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as f64).unwrap_or(0.0)
}

pub(crate) struct GuestEvent {
    pub kind: u32,
    pub data: Vec<u8>,
}

pub(crate) struct HostState {
    pub events: Vec<GuestEvent>,
    /// Wall-clock deadline for the current guest call.
    pub deadline: Instant,
    pub memory: Option<Memory>,
    limits: StoreLimits,
    pub clock: Clock,
    pub calls_emitted: u32,
    pub max_calls: u32,
    pub log_bytes: usize,
    pub max_log_bytes: usize,
    pub dropped_logs: u32,
}

impl HostState {
    pub fn new(max_memory: usize, clock: Clock) -> HostState {
        HostState {
            events: Vec::new(),
            deadline: Instant::now() + Duration::from_secs(3600),
            memory: None,
            limits: StoreLimitsBuilder::new().memory_size(max_memory).instances(1).build(),
            clock,
            calls_emitted: 0,
            max_calls: u32::MAX,
            log_bytes: 0,
            max_log_bytes: usize::MAX,
            dropped_logs: 0,
        }
    }
}

/// The guest exceeded its host-call budget (raised from inside `emit`).
#[derive(Debug)]
pub(crate) struct HostCallLimit;

impl std::fmt::Display for HostCallLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("host call limit exceeded")
    }
}

impl std::error::Error for HostCallLimit {}

fn host_emit(mut caller: Caller<'_, HostState>, kind: u32, ptr: u32, len: u32) -> wasmtime::Result<()> {
    let memory = caller.data().memory.expect("memory set after instantiation");
    let (start, end) = (ptr as usize, ptr as usize + len as usize);
    let data = memory
        .data(&caller)
        .get(start..end)
        .ok_or_else(|| wasmtime::format_err!("guest emitted an out-of-bounds event"))?
        .to_vec();
    let st = caller.data_mut();
    match kind {
        EV_CALL => {
            st.calls_emitted += 1;
            if st.calls_emitted > st.max_calls {
                return Err(HostCallLimit.into());
            }
        }
        EV_LOG => {
            if st.log_bytes + data.len() > st.max_log_bytes {
                st.dropped_logs += 1;
                return Ok(());
            }
            st.log_bytes += data.len();
        }
        _ => {}
    }
    st.events.push(GuestEvent { kind, data });
    Ok(())
}

pub(crate) const ST_DONE: u32 = 0;
pub(crate) const ST_WAITING: u32 = 1;

/// One live sandbox instance.
pub(crate) struct Guest {
    pub store: Store<HostState>,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    start: TypedFunc<(u32, u32), u32>,
    settle: TypedFunc<(u32, u32), u32>,
    gc: TypedFunc<(), u32>,
    reset_run: TypedFunc<(), ()>,
}

impl Guest {
    pub async fn new(engine: &EngineInner, clock: Clock, memory_cap: usize) -> Result<Guest> {
        let mut store = Store::new(&engine.wasm, HostState::new(memory_cap, clock));
        store.limiter(|s| &mut s.limits);
        // Guest code runs on a wasmtime fiber (its own stack, not the caller's).
        // Every ~10ms it yields to the async executor so long CPU-bound scripts
        // don't starve other tasks, and traps once the wall-clock deadline passes.
        store.epoch_deadline_callback(|ctx| {
            if Instant::now() >= ctx.data().deadline {
                return Err(wasmtime::Trap::Interrupt.into());
            }
            Ok(wasmtime::UpdateDeadline::Yield(10))
        });
        store.set_epoch_deadline(10);
        if engine.config.metering {
            store.set_fuel(u64::MAX).map_err(wt)?;
        }
        let instance = engine.pre.instantiate_async(&mut store).await.map_err(wt)?;
        let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| anyhow!("guest has no memory"))?;
        store.data_mut().memory = Some(memory);
        let f = |store: &mut Store<HostState>, name: &str| {
            instance.get_func(&mut *store, name).ok_or_else(|| anyhow!("guest export {name} missing"))
        };
        let alloc = f(&mut store, "mb_alloc")?.typed(&store).map_err(wt)?;
        let start = f(&mut store, "mb_start")?.typed(&store).map_err(wt)?;
        let settle = f(&mut store, "mb_settle")?.typed(&store).map_err(wt)?;
        let gc = f(&mut store, "mb_gc")?.typed(&store).map_err(wt)?;
        let reset_run = f(&mut store, "mb_reset_run")?.typed(&store).map_err(wt)?;
        Ok(Guest { store, memory, alloc, start, settle, gc, reset_run })
    }

    /// Recreate an instance from a snapshot taken with [`Guest::snapshot`].
    pub async fn restore(engine: &EngineInner, diff: &MemDiff, clock: Clock, memory_cap: usize) -> Result<Guest> {
        let mut g = Guest::new(engine, clock, memory_cap).await?;
        let cur = g.memory.data_size(&g.store);
        if diff.size < cur {
            return Err(anyhow!("snapshot is smaller than the base image"));
        }
        if diff.size > cur {
            let pages = ((diff.size - cur) / 65536) as u64;
            g.memory.grow(&mut g.store, pages).map_err(wt).context("growing memory for restore")?;
        }
        diff.apply(g.memory.data_mut(&mut g.store));
        Ok(g)
    }

    pub async fn snapshot(&mut self, base: &[u8]) -> MemDiff {
        self.store.data_mut().deadline = Instant::now() + Duration::from_secs(10);
        let _ = self.gc.call_async(&mut self.store, ()).await;
        MemDiff::capture(self.memory.data(&self.store), base)
    }

    /// Drop an unfinished run's state so the instance can start another.
    pub async fn reset_run(&mut self) -> wasmtime::Result<()> {
        self.store.data_mut().deadline = Instant::now() + Duration::from_secs(5);
        self.store.data_mut().events.clear();
        self.reset_run.call_async(&mut self.store, ()).await
    }

    pub fn memory_size(&self) -> usize {
        self.memory.data_size(&self.store)
    }

    async fn call_with(&mut self, f: TypedFunc<(u32, u32), u32>, input: &[u8]) -> wasmtime::Result<u32> {
        let len = input.len() as u32;
        let ptr = self.alloc.call_async(&mut self.store, len).await?;
        self.memory.write(&mut self.store, ptr as usize, input)?;
        f.call_async(&mut self.store, (ptr, len)).await
    }

    pub async fn start(&mut self, request: &[u8]) -> wasmtime::Result<u32> {
        self.call_with(self.start.clone(), request).await
    }

    pub async fn settle(&mut self, batch: &[u8]) -> wasmtime::Result<u32> {
        self.call_with(self.settle.clone(), batch).await
    }

    pub fn take_events(&mut self) -> Vec<GuestEvent> {
        std::mem::take(&mut self.store.data_mut().events)
    }

    /// Bound the next guest call by wall-clock time.
    pub fn set_deadline(&mut self, deadline: Instant) {
        self.store.data_mut().deadline = deadline;
        self.store.set_epoch_deadline(1);
    }

    pub fn fuel_left(&self) -> Option<u64> {
        self.store.get_fuel().ok()
    }

    pub fn set_fuel(&mut self, fuel: u64) {
        let _ = self.store.set_fuel(fuel);
    }
}
