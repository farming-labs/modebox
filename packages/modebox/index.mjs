// modebox for Node.js: run agent-written JS/TS in a WebAssembly sandbox.
//
// API: run / createRunner to execute code with host functions,
// getHostFunctionContext().interrupt() to pause a run, continuation +
// resolutions to resume it, plus approvals, captured console output, stats
// and deterministic options.

import { AsyncLocalStorage } from 'node:async_hooks';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const native = loadNative();

// The native binding: `MODEBOX_NATIVE`, a local build next to this file, or
// the package for this platform (an optional dependency, so npm installs only
// the one that matches).
function loadNative() {
  if (process.env.MODEBOX_NATIVE) return require(process.env.MODEBOX_NATIVE);
  try {
    return require('./modebox.node');
  } catch (e) {
    if (e.code !== 'MODULE_NOT_FOUND') throw e;
  }
  let target = `${process.platform}-${process.arch}`;
  if (process.platform === 'linux') {
    target += process.report?.getReport?.().header?.glibcVersionRuntime ? '-gnu' : '-musl';
  }
  try {
    return require(`modebox-${target}`);
  } catch (e) {
    if (e.code !== 'MODULE_NOT_FOUND') throw e;
    throw new Error(
      `modebox: no native binding for ${target}. Prebuilt bindings cover darwin-arm64, darwin-x64, ` +
        `linux-x64-gnu and linux-arm64-gnu; elsewhere, build crates/modebox-node and set MODEBOX_NATIVE to it.`,
    );
  }
}

// ---- tagged-JSON codec (mirrors the guest's codec.rs) ------------------------

function encode(v, seen) {
  if (v === undefined) return { $t: 'u' };
  if (v === null) return null;
  switch (typeof v) {
    case 'boolean':
    case 'string':
      return v;
    case 'number':
      if (Number.isNaN(v)) return { $t: 'n', v: 'NaN' };
      if (v === Infinity) return { $t: 'n', v: 'Infinity' };
      if (v === -Infinity) return { $t: 'n', v: '-Infinity' };
      if (Object.is(v, -0)) return { $t: 'n', v: '-0' };
      return v;
    case 'bigint':
      return { $t: 'bi', v: v.toString() };
    case 'function':
    case 'symbol':
      throw new TypeError(`a ${typeof v} cannot cross the sandbox boundary`);
  }
  seen ??= new Set();
  if (seen.has(v)) throw new TypeError('a circular structure cannot cross the sandbox boundary');
  seen.add(v);
  try {
    if (Array.isArray(v)) return v.map((x) => encode(x, seen));
    if (v instanceof Date) return { $t: 'd', v: Number.isNaN(v.getTime()) ? null : v.getTime() };
    if (v instanceof Map) return { $t: 'm', v: [...v].map(([k, x]) => [encode(k, seen), encode(x, seen)]) };
    if (v instanceof Set) return { $t: 's', v: [...v].map((x) => encode(x, seen)) };
    if (v instanceof RegExp) return { $t: 're', src: v.source, flags: v.flags };
    if (v instanceof Error) {
      const e = { $t: 'e', name: v.name, message: v.message };
      if (v.code !== undefined) e.code = encode(v.code, seen);
      return e;
    }
    if (v instanceof ArrayBuffer) return { $t: 'b', k: 'ArrayBuffer', v: Buffer.from(v).toString('base64') };
    if (ArrayBuffer.isView(v)) {
      const k = v instanceof Buffer ? 'Uint8Array' : v.constructor.name;
      const b64 = Buffer.from(v.buffer, v.byteOffset, v.byteLength).toString('base64');
      return k === 'Uint8Array' ? { $t: 'b', v: b64 } : { $t: 'b', k, v: b64 };
    }
    const out = {};
    for (const k of Object.keys(v)) out[k] = encode(v[k], seen);
    return Object.hasOwn(v, '$t') ? { $t: 'o', v: out } : out;
  } finally {
    seen.delete(v);
  }
}

const VIEWS = { Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array };
const ERRORS = { Error, TypeError, RangeError, SyntaxError, ReferenceError, EvalError, URIError };

function decode(j) {
  if (j === null || typeof j !== 'object') return j;
  if (Array.isArray(j)) return j.map(decode);
  switch (j.$t) {
    case undefined: {
      const out = {};
      for (const k of Object.keys(j)) out[k] = decode(j[k]);
      return out;
    }
    case 'u': return undefined;
    case 'n': return j.v === 'Infinity' ? Infinity : j.v === '-Infinity' ? -Infinity : j.v === '-0' ? -0 : NaN;
    case 'bi': return BigInt(j.v);
    case 'd': return new Date(j.v ?? NaN);
    case 'b': {
      const buf = Buffer.from(j.v, 'base64');
      const bytes = new Uint8Array(buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));
      if (!j.k || j.k === 'Uint8Array') return bytes;
      if (j.k === 'ArrayBuffer') return bytes.buffer;
      return VIEWS[j.k] ? new VIEWS[j.k](bytes.buffer) : bytes;
    }
    case 'm': return new Map(j.v.map(([k, v]) => [decode(k), decode(v)]));
    case 's': return new Set(j.v.map(decode));
    case 're': return new RegExp(j.src, j.flags);
    case 'e': {
      const e = new (ERRORS[j.name] ?? Error)(j.message);
      if (!ERRORS[j.name]) e.name = j.name;
      if (j.stack) e.stack = j.stack;
      if (j.code !== undefined) e.code = decode(j.code);
      return e;
    }
    case 'o': return decode(j.v);
    default: {
      const out = {};
      for (const k of Object.keys(j)) out[k] = decode(j[k]);
      return out;
    }
  }
}

// ---- host function context --------------------------------------------------

const als = new AsyncLocalStorage();
const APPROVAL = Symbol('modebox.approval');

/**
 * What `interrupt()` throws to unwind the host function. It's an Error so that
 * host code which catches errors and rethrows them (as most wrappers do) lets
 * it through; code that swallows it cancels the interrupt.
 */
export class HostFunctionInterruptSignal extends Error {
  constructor() {
    super('the host function was interrupted');
    this.name = 'HostFunctionInterruptSignal';
  }
}

/** Inside a host function: info about the call, and `interrupt()` to pause the run. */
export function getHostFunctionContext() {
  const ctx = als.getStore();
  if (!ctx) throw new Error('getHostFunctionContext() can only be called inside a host function');
  return ctx;
}

/** Mark a host function as needing approval before every call. */
export function requireApproval(fn) {
  fn[APPROVAL] = true;
  return fn;
}

export class RunError extends Error {
  constructor(e, extra = {}) {
    super(e.message);
    this.name = e.name;
    this.kind = e.kind;
    // The uncaught error's own code when it had one (say, from a host function), else the kind.
    this.code = e.code ?? 'RUN_' + e.kind.replace(/([a-z])([A-Z])/g, '$1_$2').toUpperCase();
    this.sandboxStack = e.stack ?? undefined;
    Object.assign(this, extra);
  }
}

function flatten(hostFunctions, prefix = '', out = new Map()) {
  for (const [k, v] of Object.entries(hostFunctions ?? {})) {
    const path = prefix ? `${prefix}.${k}` : k;
    if (typeof v === 'function') out.set(path, v);
    else if (v && typeof v === 'object') flatten(v, path, out);
  }
  return out;
}

function globMatch(pattern, text) {
  const re = new RegExp('^' + pattern.split('*').map((s) => s.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('.*') + '$');
  return re.test(text);
}

function toolSpecs(fns, approve = [], schemas = {}, sources = [], sinks = []) {
  return [...fns].map(([name, fn]) => ({
    name,
    approval: !!fn[APPROVAL] || approve.some((p) => globMatch(p, name)),
    source: sources.some((p) => globMatch(p, name)),
    sink: sinks.some((p) => globMatch(p, name)),
    description: schemas[name]?.description,
    inputSchema: schemas[name]?.inputSchema,
    outputSchema: schemas[name]?.outputSchema,
  }));
}

function nativeOptions(o = {}) {
  const l = o.limits ?? {};
  return JSON.stringify({
    timeoutMs: l.timeoutMs,
    fuel: l.fuel,
    memoryLimitBytes: l.memoryLimitBytes,
    maxHostCalls: l.maxHostCalls ?? l.maxBridgeRequests,
    maxConcurrency: l.maxConcurrency,
    maxValueBytes: l.maxValueBytes ?? (Math.max(l.maxResultBytes ?? 0, l.maxHostFunctionOutputBytes ?? 0) || undefined),
    maxLogBytes: l.maxLogBytes ?? l.maxConsoleOutputBytes,
    seed: o.seed,
    virtualTimeMs: o.virtualTime === true ? Date.now() : o.virtualTime,
    mode: o.mode,
    filename: o.filename,
    typescript: o.typescript,
  });
}

function makeDispatch(fns, onCall) {
  return async (msg) => {
    const m = JSON.parse(msg);
    const { name, id, args, resume } = m;
    const fn = fns.get(name);
    let signal;
    let interruptPayload;
    const ctx = {
      callId: id,
      hostFunctionName: name,
      resume:
        resume === null || resume === undefined
          ? undefined
          : { interruptionId: id, resolution: decode(resume), payload: 'payload' in m ? decode(m.payload) : undefined },
      interrupt(payload = {}) {
        interruptPayload = payload;
        signal = new HostFunctionInterruptSignal();
        throw signal;
      },
    };
    try {
      onCall?.(name, id);
      const value = await als.run(ctx, () => fn(...decode(args)));
      return JSON.stringify({ ok: true, v: encode(value) });
    } catch (e) {
      if (signal !== undefined && e === signal) {
        return JSON.stringify({ interrupt: { kind: interruptPayload?.kind ?? 'interrupt', payload: encode(interruptPayload ?? null) } });
      }
      return JSON.stringify({ ok: false, e: { name: e?.name ?? 'Error', message: String(e?.message ?? e), code: typeof e?.code === 'string' ? e.code : undefined } });
    }
  };
}

function shape(out, { throwOnError = true } = {}) {
  const r = JSON.parse(out);
  const base = { logs: r.logs, stats: r.stats };
  if (r.status === 'completed') return { status: 'completed', value: decode(r.value), ...base };
  if (r.status === 'suspended') {
    return {
      status: 'interrupted',
      interruptions: r.interrupts.map((i) => ({
        id: i.id,
        hostFunctionName: i.tool,
        kind: i.kind,
        args: decode(i.args),
        payload: decode(i.payload),
      })),
      continuation: r.continuation,
      ...base,
    };
  }
  if (throwOnError) throw new RunError(r.error, base);
  return { status: 'failed', error: new RunError(r.error, base), ...base };
}

export class Runner {
  #engine;
  #defaults;

  /**
   * @param {object} [config]
   * @param {string} [config.continuationSecret] key for sealing continuations (defaults to $MODEBOX_SECRET / $RUN_CONTINUATION_SECRET, else random per process)
   * @param {boolean} [config.metering] deterministic fuel accounting (default true)
   * @param {string} [config.continuationsDir] directory recording used continuations, shared by every
   *   process that resumes the same tokens (default: in memory, so single-use only within this process)
   */
  constructor(config = {}) {
    const secret = config.continuationSecret ?? process.env.MODEBOX_SECRET ?? process.env.RUN_CONTINUATION_SECRET;
    this.#engine = native.createEngine(JSON.stringify({
      secret, metering: config.metering, cacheDir: config.cacheDir,
      continuationTtlMs: config.continuationTtlMs, continuationsDir: config.continuationsDir,
    }));
    this.#defaults = { limits: config.limits, approve: config.approve, sources: config.sources, sinks: config.sinks };
  }

  /**
   * Run code, or resume a suspended run when `continuation` is given.
   * Resolves `{ status: 'completed', value }` or `{ status: 'interrupted', interruptions, continuation }`;
   * rejects with RunError when the code fails (unless `throwOnError: false`).
   */
  async run(options) {
    const o = { ...this.#defaults, ...options, limits: { ...this.#defaults.limits, ...options.limits } };
    const fns = flatten(o.hostFunctions);
    const tools = JSON.stringify(toolSpecs(fns, o.approve, o.schemas, o.sources, o.sinks));
    const dispatch = makeDispatch(fns, o.onCall);
    const opts = nativeOptions(o);
    let out;
    if (o.continuation) {
      const resolutions = (o.resolutions ?? []).map((r) => {
        const id = Number(r.interruptionId ?? r.id);
        const action = r.action ?? (r.value === false ? 'deny' : r.value === true ? 'approve' : 'resume');
        return { id, action, value: encode(r.value), message: r.message };
      });
      out = await native.resume(this.#engine, o.continuation, JSON.stringify(resolutions), tools, opts, dispatch);
    } else {
      out = await native.run(this.#engine, o.source, tools, opts, dispatch);
    }
    const result = shape(out, o);
    if (o.onLog) for (const l of result.logs) o.onLog(l);
    return result;
  }

  /** TypeScript declarations for a set of host functions (for the model's prompt). */
  declarations(hostFunctions, schemas = {}) {
    return native.declarations(JSON.stringify(toolSpecs(flatten(hostFunctions), [], schemas)));
  }

  close() {
    native.dropEngine(this.#engine);
  }
}

export function createRunner(config) {
  return new Runner(config);
}

let shared;
/** Run with a process-wide default runner. */
export function run(options) {
  shared ??= new Runner();
  return shared.run(options);
}

export { encode as _encode, decode as _decode };
