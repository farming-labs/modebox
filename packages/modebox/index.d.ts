export type HostFunction = (...args: any[]) => unknown | Promise<unknown>;
export interface HostFunctions { [name: string]: HostFunction | HostFunctions }

export interface Limits {
  timeoutMs?: number;
  /** Deterministic CPU budget in wasm fuel units (cumulative across resumes). */
  fuel?: number;
  memoryLimitBytes?: number;
  maxHostCalls?: number;
  /** Host calls running at once; extra calls queue. */
  maxConcurrency?: number;
  maxValueBytes?: number;
  maxLogBytes?: number;
  /** Alternative names for the limits above. */
  maxBridgeRequests?: number;
  maxResultBytes?: number;
  maxHostFunctionOutputBytes?: number;
  maxConsoleOutputBytes?: number;
}

export interface ToolSchema { description?: string; inputSchema?: object; outputSchema?: object }

export interface RunOptions {
  source?: string;
  hostFunctions?: HostFunctions;
  limits?: Limits;
  /** Glob patterns of host function paths that need approval (`"bank.*"`). */
  approve?: string[];
  /** JSON Schemas per path, used for `declarations()`. */
  schemas?: Record<string, ToolSchema>;
  /** Resume this suspended run instead of starting `source`. */
  continuation?: string;
  resolutions?: Array<{ interruptionId: number; value?: unknown; action?: 'approve' | 'deny' | 'resume' | 'return'; message?: string }>;
  /** 64 hex chars: makes Math.random / crypto reproducible. */
  seed?: string;
  /** Virtual time: timers fire instantly; `true` starts at Date.now(). */
  virtualTime?: number | true;
  mode?: 'auto' | 'script' | 'function';
  filename?: string;
  typescript?: boolean;
  onLog?: (entry: LogEntry) => void;
  onCall?: (name: string, id: number) => void;
  /** Return `{ status: 'failed', error }` instead of throwing. */
  throwOnError?: boolean;
}

export interface LogEntry { level: string; message: string }
export interface Stats { wallMs: number; guestMs: number; instantiateMs: number; fuelUsed: number | null; hostCalls: number; memoryBytes: number; continuationBytes: number | null }

export interface Interruption { id: number; hostFunctionName: string; kind: string; args: unknown[]; payload: unknown }

export type RunResult =
  | { status: 'completed'; value: unknown; logs: LogEntry[]; stats: Stats }
  | { status: 'interrupted'; interruptions: Interruption[]; continuation: string; logs: LogEntry[]; stats: Stats }
  | { status: 'failed'; error: RunError; logs: LogEntry[]; stats: Stats };

export declare class RunError extends Error {
  kind: string;
  code: string;
  sandboxStack?: string;
  logs: LogEntry[];
  stats: Stats;
}

export interface HostFunctionContext {
  callId: number;
  hostFunctionName: string;
  resume?: { resolution: unknown };
  interrupt(payload?: { kind?: string; [k: string]: unknown }): never;
}

export declare function getHostFunctionContext(): HostFunctionContext;
export declare function requireApproval<F extends HostFunction>(fn: F): F;

export interface RunnerConfig { continuationSecret?: string; metering?: boolean; cacheDir?: string; continuationTtlMs?: number; limits?: Limits; approve?: string[] }
export declare class Runner {
  constructor(config?: RunnerConfig);
  run(options: RunOptions): Promise<RunResult>;
  declarations(hostFunctions: HostFunctions, schemas?: Record<string, ToolSchema>): string;
  close(): void;
}
export declare function createRunner(config?: RunnerConfig): Runner;
export declare function run(options: RunOptions): Promise<RunResult>;
