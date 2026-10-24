// modebox/run: the API of Vercel's `run` package, on top of modebox. See run.mjs for the differences.

export type HostFunction<ARGUMENTS extends unknown[] = unknown[], OUTPUT = unknown> = (
  ...args: ARGUMENTS
) => OUTPUT | Promise<OUTPUT>;
export type HostFunctionGroup = Record<string, HostFunction<never[], unknown>>;
export type HostFunctions = Record<string, HostFunctionGroup>;

export interface HostFunctionResumeContext {
  interruptionId: string;
  /** The payload the call interrupted with. */
  payload: unknown;
  resolution: unknown;
}

export interface HostFunctionContext {
  /** Aborted when the run is aborted, times out or fails. */
  abortSignal: AbortSignal;
  /** Identifies this invocation (a fresh run or one resume). */
  invocationId: string;
  /** Identifies the logical run across every resume. */
  logicalRunId: string;
  requestId: string;
  /** One-based order of this host function request within the run. */
  requestIndex: number;
  /** Fully qualified path, for example `tools.search`. */
  hostFunctionName: string;
  /** Suspend the run; it resumes when this interruption gets a resolution. */
  interrupt(payload: unknown): never;
  /** Set when the call runs again after an interruption was resolved. */
  resume?: HostFunctionResumeContext;
}

export declare function getHostFunctionContext(): HostFunctionContext;

export interface RunLimits {
  /** Wall-clock limit for one invocation, including waits on host functions. @default 30_000 */
  timeoutMs?: number;
  /** Time spent executing guest code in one invocation; waits don't count. @default 10_000 */
  cpuTimeoutMs?: number;
  /** @default 64 MiB */
  memoryLimitBytes?: number;
  /** Validated; QuickJS reports stack overflows on its own. @default 2 MiB */
  maxStackSizeBytes?: number;
  /** @default 1 MiB */
  maxResultBytes?: number;
  /** @default 64 KiB */
  maxConsoleOutputBytes?: number;
  /** @default 256 KiB */
  maxSourceBytes?: number;
  /** @default 1 MiB */
  maxHostFunctionArgumentsBytes?: number;
  /** @default 4 MiB */
  maxHostFunctionOutputBytes?: number;
  /** @default 256 */
  maxBridgeRequests?: number;
  /** Host calls running at once; extra calls queue. @default 32 */
  maxInFlightBridgeRequests?: number;
  /** Validated, not enforced. @default 32 MiB */
  maxContinuationBytes?: number;
}

export interface SignedContinuationCodecOptions {
  secret: string | Uint8Array;
  /** @default 3_600_000 */
  maxAgeMs?: number;
}

export interface ContinuationCodec {
  readonly __modeboxCodec?: never;
}

export declare function createSignedContinuationCodec(options: SignedContinuationCodecOptions): ContinuationCodec;

export interface RunnerOptions {
  limits?: RunLimits;
  /** Key for continuations; defaults to $RUN_CONTINUATION_SECRET, else a random key for this process. */
  continuationSecret?: string | Uint8Array;
  /** From createSignedContinuationCodec. */
  continuationCodec?: ContinuationCodec;
  /** Bound into every continuation; resuming with another audience fails. */
  continuationAudience?: string;
}

export type RunSourceType = 'function-body';

export interface RunInput<TOKEN = string> {
  /** JavaScript or TypeScript function body, with top-level `await` and `return`. */
  source: string;
  sourceType?: RunSourceType;
  hostFunctions?: HostFunctions;
  abortSignal?: AbortSignal;
  limits?: RunLimits;
  continuation?: TOKEN;
  /** Resolutions for some or all pending interruptions; the rest stay pending. */
  resolutions?: RunResolution[];
  /** Bound into the continuation and required unchanged when resuming. */
  continuationContext?: unknown;
}

export interface RunCompletedResult<OUTPUT = unknown> {
  status: 'completed';
  value: OUTPUT;
}

export interface RunInterruption<PAYLOAD = unknown> {
  id: string;
  hostFunctionName: string;
  arguments: unknown[];
  payload: PAYLOAD;
}

export interface RunInterruptedResult<TOKEN = string> {
  status: 'interrupted';
  interruptions: RunInterruption[];
  /** Opaque, single-use. Store it as is. */
  continuation: TOKEN;
}

export interface RunResolution<VALUE = unknown> {
  interruptionId: string;
  value: VALUE;
}

export type RunResult<OUTPUT = unknown, TOKEN = string> = RunCompletedResult<OUTPUT> | RunInterruptedResult<TOKEN>;

export interface Runner {
  run<OUTPUT = unknown>(input: RunInput): Promise<RunResult<OUTPUT>>;
}

export declare function createRunner(options?: RunnerOptions): Runner;
export declare function run<OUTPUT = unknown>(input: RunInput): Promise<RunResult<OUTPUT>>;
export declare function isRunInterruptedResult(result: unknown): result is RunInterruptedResult;
/** Cap how many runs execute at once; `undefined` removes the cap. */
export declare function setMaxWorkers(maxWorkers?: number): void;

export declare class RunError extends Error {
  code: string;
  readonly details?: unknown;
  constructor(message: string, code?: string, details?: unknown);
  static isInstance(error: unknown): error is RunError;
}
export declare class RunAbortedError extends RunError {
  constructor();
}
export declare class RunTimeoutError extends RunError {
  constructor(timeoutMs: number);
}
export declare class RunCpuTimeoutError extends RunError {
  constructor(cpuTimeoutMs: number);
}
export declare class RunConcurrencyError extends RunError {
  constructor(maxWorkers: number);
}
export declare class RunSourceTooLargeError extends RunError {
  constructor(bytes: number, maxBytes: number);
}
export declare class RunBridgeLimitError extends RunError {
  constructor(message: string, details?: unknown);
}
export declare class RunHostFunctionError extends RunError {
  constructor(message: string, details?: unknown);
}
export declare class RunDetachedBridgeRequestError extends RunError {
  constructor(message: string, details?: unknown);
}
export declare class RunProtocolError extends RunError {
  constructor(message: string, details?: unknown);
}
