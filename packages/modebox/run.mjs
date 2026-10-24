// modebox/run: the API of Vercel's `run` package, on top of modebox.
//
// It takes the same inputs and returns the same shapes as `run`, so code written
// against it (such as @ai-sdk/code-mode) can use modebox as its runtime without
// changes. Interrupted runs resume from a VM snapshot instead of replaying the
// script. Differences:
//
// - Source is a function body. `sourceType: 'module'`, `moduleLoader` and
//   `syncHostFunctions` are not supported and throw.
// - Continuations come from `continuationSecret` or createSignedContinuationCodec;
//   custom codecs and `verificationSecrets` (key rotation) throw.
// - `maxInFlightBridgeRequests` queues extra host calls instead of failing them.
// - `maxStackSizeBytes` and `maxContinuationBytes` are validated but not enforced
//   separately (QuickJS still reports stack overflows).
// - setMaxWorkers() caps how many runs execute at once; without it there is no cap.

import { AsyncLocalStorage } from 'node:async_hooks';
import { Buffer } from 'node:buffer';
import { createHmac, randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import {
  RunError as ModeboxRunError,
  _encode,
  createRunner as createModeboxRunner,
  getHostFunctionContext as modeboxContext,
} from './index.mjs';

// ---- errors ---------------------------------------------------------------------

const BRAND = Symbol.for('modebox.run.RunError');

export class RunError extends Error {
  constructor(message, code = 'RUN_ERROR', details) {
    super(message);
    this.name = new.target.name;
    this.code = code;
    this.details = details;
    Object.defineProperty(this, BRAND, { value: true });
  }

  /** True for RunErrors, including ones from another copy of this module. */
  static isInstance(error) {
    return error !== null && typeof error === 'object' && error[BRAND] === true;
  }
}

export class RunAbortedError extends RunError {
  constructor() {
    super('The run was aborted.', 'RUN_ABORTED');
  }
}

export class RunTimeoutError extends RunError {
  constructor(timeoutMs) {
    super(`The run timed out after ${timeoutMs}ms.`, 'RUN_TIMEOUT', { timeoutMs });
  }
}

export class RunCpuTimeoutError extends RunError {
  constructor(cpuTimeoutMs) {
    super(`The run exceeded its CPU time limit of ${cpuTimeoutMs}ms.`, 'RUN_CPU_TIMEOUT', { cpuTimeoutMs });
  }
}

export class RunConcurrencyError extends RunError {
  constructor(maxWorkers) {
    super(`Too many runs at once: the limit is ${maxWorkers}.`, 'RUN_CONCURRENCY_LIMIT', { maxWorkers });
  }
}

export class RunSourceTooLargeError extends RunError {
  constructor(bytes, maxBytes) {
    super(`The source is ${bytes} bytes, over the ${maxBytes} byte limit.`, 'RUN_SOURCE_TOO_LARGE', { bytes, maxBytes });
  }
}

export class RunBridgeLimitError extends RunError {
  constructor(message, details) {
    super(message, 'RUN_BRIDGE_LIMIT', details);
  }
}

export class RunHostFunctionError extends RunError {
  constructor(message, details) {
    super(message, 'RUN_HOST_FUNCTION_ERROR', details);
  }
}

export class RunDetachedBridgeRequestError extends RunError {
  constructor(message, details) {
    super(message, 'RUN_DETACHED_BRIDGE_REQUEST', details);
  }
}

export class RunProtocolError extends RunError {
  constructor(message, details) {
    super(message, 'RUN_PROTOCOL_ERROR', details);
  }
}

// ---- limits ---------------------------------------------------------------------

const MAX_LIMIT = 2 ** 31 - 1;
const DEFAULT_LIMITS = {
  timeoutMs: 30_000,
  cpuTimeoutMs: 10_000,
  memoryLimitBytes: 64 << 20,
  maxStackSizeBytes: 2 << 20,
  maxResultBytes: 1 << 20,
  maxConsoleOutputBytes: 64 << 10,
  maxSourceBytes: 256 << 10,
  maxHostFunctionArgumentsBytes: 1 << 20,
  maxHostFunctionOutputBytes: 4 << 20,
  maxBridgeRequests: 256,
  maxInFlightBridgeRequests: 32,
  maxContinuationBytes: 32 << 20,
};

function positiveInteger(value, path, max = MAX_LIMIT) {
  if (!Number.isInteger(value) || value <= 0) throw new TypeError(`${path} must be a positive integer.`);
  if (value > max) throw new TypeError(`${path} must be at most ${max}.`);
  return value;
}

function resolveLimits(...layers) {
  const out = {};
  for (const [name, fallback] of Object.entries(DEFAULT_LIMITS)) {
    let value = fallback;
    for (const layer of layers) if (layer?.[name] != null) value = layer[name];
    out[name] = positiveInteger(value, `limits.${name}`);
  }
  return out;
}

function modeboxLimits(l) {
  return {
    timeoutMs: l.timeoutMs,
    cpuTimeoutMs: l.cpuTimeoutMs,
    memoryLimitBytes: l.memoryLimitBytes,
    maxHostCalls: l.maxBridgeRequests,
    maxConcurrency: l.maxInFlightBridgeRequests,
    maxLogBytes: l.maxConsoleOutputBytes,
    // The byte limits for each direction are checked here, so the native cap only
    // needs to sit above all of them.
    maxValueBytes: Math.min(
      MAX_LIMIT,
      2 * Math.max(l.maxResultBytes, l.maxHostFunctionArgumentsBytes, l.maxHostFunctionOutputBytes),
    ),
  };
}

const sizeOf = (value) => Buffer.byteLength(JSON.stringify(_encode(value)));

// ---- concurrency ----------------------------------------------------------------

let maxWorkers;
let activeRuns = 0;

/** Cap how many runs execute at once (extra runs fail with RunConcurrencyError). `undefined` removes the cap. */
export function setMaxWorkers(value) {
  maxWorkers = value === undefined ? undefined : positiveInteger(value, 'maxWorkers');
}

// ---- continuations --------------------------------------------------------------

const CODEC = Symbol('modebox.run.codec');
const TOKEN_PREFIX = 'mbx1.';
const DEFAULT_MAX_AGE_MS = 60 * 60 * 1000;

function keyBytes(secret, what) {
  if (typeof secret === 'string' && secret.length > 0) return Buffer.from(secret, 'utf8');
  if (secret instanceof Uint8Array && secret.length > 0) return Buffer.from(secret);
  throw new TypeError(`${what} must be a non-empty string or Uint8Array.`);
}

/** Signed continuations with a key you choose, so tokens can resume in another process. */
export function createSignedContinuationCodec(options = {}) {
  if (options.verificationSecrets !== undefined) {
    throw new TypeError('modebox/run does not support verificationSecrets (key rotation).');
  }
  const key = keyBytes(options.secret, 'Continuation secret');
  const maxAgeMs = positiveInteger(options.maxAgeMs ?? DEFAULT_MAX_AGE_MS, 'Continuation maxAgeMs');
  const unsupported = () => {
    throw new TypeError('This codec is used by modebox/run runners and has no encode/decode of its own.');
  };
  return { [CODEC]: { key, maxAgeMs }, encode: unsupported, decode: unsupported };
}

function subkey(key, label) {
  return createHmac('sha256', key).update(label).digest();
}

// JSON with sorted keys, so equal contexts give equal bytes.
function canonical(value) {
  if (value === undefined) return 'null';
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  return `{${Object.keys(value)
    .sort()
    .filter((k) => value[k] !== undefined)
    .map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`)
    .join(',')}}`;
}

// ---- runners --------------------------------------------------------------------

// One modebox engine per key and token lifetime, shared by every runner that uses them.
const engines = new Map();
function modeboxRunner(sealKey, maxAgeMs) {
  const id = `${sealKey.toString('hex')}:${maxAgeMs}`;
  let runner = engines.get(id);
  if (!runner) {
    runner = createModeboxRunner({ continuationSecret: sealKey.toString('hex'), continuationTtlMs: maxAgeMs });
    engines.set(id, runner);
  }
  return runner;
}

const als = new AsyncLocalStorage();

/** Inside a host function: details of the call, and interrupt() to pause the run. */
export function getHostFunctionContext() {
  const ctx = als.getStore();
  if (!ctx) throw new Error('getHostFunctionContext() can only be called inside a host function.');
  return ctx;
}

// Resolution value that leaves an interruption pending: its host function raises it again.
const STILL_PENDING = '__modebox_run_still_pending__';

const interruptionId = (callId) => `interrupt-${callId}`;
function callIdOf(id) {
  const match = /^interrupt-(\d+)$/u.exec(String(id));
  if (!match) throw new RunProtocolError(`Unknown interruption id "${id}".`);
  return Number(match[1]);
}

function wrapHostFunctions(hostFunctions, state, path = '') {
  if (hostFunctions == null) return {};
  if (typeof hostFunctions !== 'object') throw new TypeError('hostFunctions must be an object.');
  const out = {};
  for (const [name, value] of Object.entries(hostFunctions)) {
    const fullName = path ? `${path}.${name}` : name;
    if (typeof value === 'function') out[name] = wrapHostFunction(value, fullName, state);
    else if (value !== null && typeof value === 'object') out[name] = wrapHostFunctions(value, state, fullName);
    else throw new TypeError(`Host function "${fullName}" must be a function.`);
  }
  return out;
}

function wrapHostFunction(fn, name, state) {
  const { limits } = state;
  return async (...args) => {
    const call = modeboxContext();
    const resolution = call.resume?.resolution;
    if (resolution !== null && typeof resolution === 'object' && resolution[STILL_PENDING] === true) {
      call.interrupt(call.resume.payload);
    }
    try {
      const argBytes = sizeOf(args);
      if (argBytes > limits.maxHostFunctionArgumentsBytes) {
        throw new RunBridgeLimitError(
          `Arguments to host function "${name}" are ${argBytes} bytes, over the ${limits.maxHostFunctionArgumentsBytes} byte limit.`,
          { hostFunctionName: name, bytes: argBytes, maxBytes: limits.maxHostFunctionArgumentsBytes },
        );
      }
      const ctx = {
        abortSignal: call.abortSignal,
        invocationId: state.invocationId,
        logicalRunId: state.logicalRunId,
        requestId: `${state.invocationId}:${call.callId}`,
        requestIndex: call.callId,
        hostFunctionName: name,
        interrupt: (payload) => call.interrupt(payload),
        resume:
          call.resume === undefined
            ? undefined
            : { interruptionId: interruptionId(call.callId), payload: call.resume.payload, resolution },
      };
      const value = await als.run(ctx, () => fn(...args));
      const outBytes = sizeOf(value);
      if (outBytes > limits.maxHostFunctionOutputBytes) {
        throw new RunBridgeLimitError(
          `Host function "${name}" returned ${outBytes} bytes, over the ${limits.maxHostFunctionOutputBytes} byte limit.`,
          { hostFunctionName: name, bytes: outBytes, maxBytes: limits.maxHostFunctionOutputBytes },
        );
      }
      return value;
    } catch (error) {
      if (RunError.isInstance(error)) state.hostErrors.push(error);
      throw error;
    }
  };
}

function toRunError(error, state) {
  if (!(error instanceof ModeboxRunError)) return error;
  const { limits } = state;
  switch (error.kind) {
    case 'Aborted':
      return new RunAbortedError();
    case 'Timeout':
      return new RunTimeoutError(limits.timeoutMs);
    case 'CpuTimeout':
      return new RunCpuTimeoutError(limits.cpuTimeoutMs);
    case 'HostCallLimit':
      return new RunBridgeLimitError(`The run made more than ${limits.maxBridgeRequests} host function calls.`, {
        maxBridgeRequests: limits.maxBridgeRequests,
      });
    case 'InvalidContinuation':
      return new RunProtocolError(error.message);
  }
  // A RunError a host function threw that nothing caught: hand back that error.
  const thrown = state.hostErrors.findLast((e) => e.code === error.code && e.message === error.message);
  if (thrown) return thrown;
  const out = new RunError(error.message, error.code.startsWith('RUN_') ? 'RUN_ERROR' : error.code);
  out.name = error.name;
  return out;
}

class Runner {
  #defaults;
  #sealKey;
  #bindKey;
  #audience;
  #maxAgeMs;

  constructor(options = {}) {
    if (options.syncHostFunctions !== undefined) {
      throw new TypeError('modebox/run does not support syncHostFunctions.');
    }
    const codec = options.continuationCodec;
    if (codec !== undefined && options.continuationSecret !== undefined) {
      throw new TypeError('Pass continuationSecret or continuationCodec, not both.');
    }
    if (codec !== undefined && !codec?.[CODEC]) {
      throw new TypeError('modebox/run supports continuationSecret and createSignedContinuationCodec, not custom codecs.');
    }
    resolveLimits(options.limits);
    let key;
    if (codec) key = codec[CODEC].key;
    else if (options.continuationSecret !== undefined) key = keyBytes(options.continuationSecret, 'continuationSecret');
    else if (process.env.RUN_CONTINUATION_SECRET) key = keyBytes(process.env.RUN_CONTINUATION_SECRET, 'RUN_CONTINUATION_SECRET');
    else key = randomBytes(32); // tokens then resume only in this process
    this.#sealKey = subkey(key, 'modebox-run/seal');
    this.#bindKey = subkey(key, 'modebox-run/bind');
    this.#maxAgeMs = codec ? codec[CODEC].maxAgeMs : DEFAULT_MAX_AGE_MS;
    this.#audience = options.continuationAudience ?? '';
    this.#defaults = { limits: options.limits };
  }

  #sign(token, logicalRunId, pending, context) {
    return createHmac('sha256', this.#bindKey)
      .update(canonical({ token, logicalRunId, pending, audience: this.#audience, context: context ?? null }))
      .digest('base64url');
  }

  #wrap(token, logicalRunId, pending, context) {
    const body = { v: 1, token, logicalRunId, pending, mac: this.#sign(token, logicalRunId, pending, context) };
    return TOKEN_PREFIX + Buffer.from(JSON.stringify(body)).toString('base64url');
  }

  #unwrap(continuation, context) {
    if (typeof continuation !== 'string' || !continuation.startsWith(TOKEN_PREFIX)) {
      throw new RunProtocolError('The continuation is not a modebox/run token.');
    }
    let body;
    try {
      body = JSON.parse(Buffer.from(continuation.slice(TOKEN_PREFIX.length), 'base64url').toString('utf8'));
    } catch {
      throw new RunProtocolError('The continuation is malformed.');
    }
    const expected = Buffer.from(this.#sign(body.token, body.logicalRunId, body.pending, context));
    const actual = Buffer.from(String(body.mac ?? ''));
    if (body.v !== 1 || expected.length !== actual.length || !timingSafeEqual(expected, actual)) {
      throw new RunProtocolError('The continuation does not match this runner, audience or context.');
    }
    return body;
  }

  async run(input) {
    if (input === null || typeof input !== 'object') throw new TypeError('run() needs an input object.');
    if (input.sourceType === 'module' || input.moduleLoader !== undefined) {
      throw new TypeError('modebox/run runs function-body source only (no modules or moduleLoader).');
    }
    const limits = resolveLimits(this.#defaults.limits, input.limits);
    const resuming = input.continuation !== undefined;
    if (!resuming) {
      if (typeof input.source !== 'string') throw new TypeError('source must be a string.');
      const bytes = Buffer.byteLength(input.source);
      if (bytes > limits.maxSourceBytes) throw new RunSourceTooLargeError(bytes, limits.maxSourceBytes);
    }
    if (maxWorkers !== undefined && activeRuns >= maxWorkers) throw new RunConcurrencyError(maxWorkers);
    activeRuns++;
    try {
      return await this.#run(input, limits, resuming);
    } finally {
      activeRuns--;
    }
  }

  async #run(input, limits, resuming) {
    let token;
    let logicalRunId = randomUUID();
    let resolutions;
    if (resuming) {
      const body = this.#unwrap(input.continuation, input.continuationContext);
      token = body.token;
      logicalRunId = body.logicalRunId;
      if (!Array.isArray(input.resolutions) || input.resolutions.length === 0) {
        throw new RunProtocolError('Resuming a continuation needs at least one resolution.');
      }
      const given = new Map();
      for (const r of input.resolutions) {
        const id = callIdOf(r.interruptionId);
        if (!body.pending.includes(id)) throw new RunProtocolError(`Interruption "${r.interruptionId}" is not pending.`);
        given.set(id, r.value);
      }
      // Interruptions without a resolution stay pending.
      resolutions = body.pending.map((id) => ({
        interruptionId: id,
        action: 'resume',
        value: given.has(id) ? given.get(id) : { [STILL_PENDING]: true },
      }));
    }
    const state = { limits, invocationId: randomUUID(), logicalRunId, hostErrors: [] };
    let result;
    try {
      result = await modeboxRunner(this.#sealKey, this.#maxAgeMs).run({
        source: input.source,
        mode: 'function',
        filename: 'run.js',
        hostFunctions: wrapHostFunctions(input.hostFunctions, state),
        limits: modeboxLimits(limits),
        abortSignal: input.abortSignal,
        ...(resuming ? { continuation: token, resolutions } : {}),
      });
    } catch (error) {
      throw toRunError(error, state);
    }
    if (result.status === 'interrupted') {
      // In request order, whatever order the calls interrupted in.
      const interruptions = [...result.interruptions].sort((a, b) => a.id - b.id);
      const pending = interruptions.map((i) => i.id);
      return {
        status: 'interrupted',
        interruptions: interruptions.map((i) => ({
          id: interruptionId(i.id),
          hostFunctionName: i.hostFunctionName,
          arguments: i.args,
          payload: i.payload,
        })),
        continuation: this.#wrap(result.continuation, logicalRunId, pending, input.continuationContext),
      };
    }
    const bytes = sizeOf(result.value);
    if (bytes > limits.maxResultBytes) {
      throw new RunBridgeLimitError(`The result is ${bytes} bytes, over the ${limits.maxResultBytes} byte limit.`, {
        bytes,
        maxBytes: limits.maxResultBytes,
      });
    }
    return { status: 'completed', value: result.value };
  }
}

export function createRunner(options) {
  return new Runner(options);
}

let shared;
export function run(input) {
  shared ??= new Runner();
  return shared.run(input);
}

export function isRunInterruptedResult(result) {
  return result !== null && typeof result === 'object' && result.status === 'interrupted';
}
