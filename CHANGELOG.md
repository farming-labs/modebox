# Changelog

Notable changes to modebox. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/). Before 1.0, a minor version may break APIs.

## [Unreleased]

## [0.2.0] - 2026-10-24

### Added

- **A resumed host function sees the payload it interrupted with.** When a call runs again after `interrupt()`, its resume context carries the original payload next to the resolution: `resume.payload` in Node and Python, and `Call::interrupt_payload` in Rust. A host function that raises several kinds of interrupt no longer has to keep its own record of which question it asked. Node's `resume` also has the `interruptionId` it answers.
- **Cancel a run.** Node's `run()` takes an `abortSignal`: aborting it stops the run within a few milliseconds, whether the script is computing or waiting on host calls, and it fails with kind `Aborted` and code `RUN_ABORTED`. Host functions get `getHostFunctionContext().abortSignal`, which fires when the run is aborted or fails, so work they started can stop. In Rust, pass a `Cancel` in `RunOptions::cancel`.
- **A CPU time limit in milliseconds.** `cpuTimeoutMs` (Node and `modebox.json`), `cpu_timeout_ms` (Python) and `Limits::cpu_time` (Rust) cap the time the script spends executing, not counting time spent waiting on host calls. Each resume gets a fresh budget. A run that exceeds it fails with kind `CpuTimeout` and code `RUN_CPU_TIMEOUT`. Unlike `fuel`, it isn't deterministic, but it's set in milliseconds and needs no metering.
- **`modebox/run`: the API of Vercel's `run` package.** `run`, `createRunner`, `getHostFunctionContext`, `createSignedContinuationCodec`, `setMaxWorkers` and the `RunError` classes take the same inputs and return the same shapes, so code written against `run`, such as `@ai-sdk/code-mode`, can use modebox as its runtime. `@ai-sdk/code-mode`'s test suite passes with it. See [docs/run-compat.md](docs/run-compat.md) for the differences.

### Changed

- **A run that fails on an uncaught error with a `code` reports that code.** If a host function throws an error with a `code` (or the script throws one) and nothing catches it, `RunError.code` is now that code, for example `E_QUOTA`, instead of `RUN_UNCAUGHT`. `kind` is still `Uncaught`, and errors without a code are unchanged. Rust's `RunError` has a new `code` field.
- **Node: limits must be positive integers.** `run()` and `createRunner()` now throw a `TypeError` such as `limits.timeoutMs must be a positive integer.` for a zero, negative, fractional or non-number limit, or one above 2³¹−1. Such values used to be ignored, except `0`, which was applied as written.
- **Rust: new enum variants and struct fields.** `ErrorKind` has `Aborted` and `CpuTimeout`, and `Limits`, `RunOptions`, `RunError` and `Call` have new fields (`cpu_time`, `cancel`, `code` and `interrupt_payload`). A `match` that lists every `ErrorKind` needs arms for the new variants, and a struct literal without `..Default::default()` needs the new fields.

### Fixed

- **Node: `interrupt()` survives host code that catches errors.** It used to throw a symbol, so a host function wrapped in a `try`/`catch` that rethrows only `Error`s swallowed the interrupt and returned normally. It now throws a `HostFunctionInterruptSignal` (an `Error`), which such wrappers pass through.

### Security

- Updated wasmtime to 49.0.4, rustls-webpki to 0.103.16, webpki-root-certs to 1.0.11 and tokio to 1.54.1, for [RUSTSEC-2026-0378](https://rustsec.org/advisories/RUSTSEC-2026-0378), [0379](https://rustsec.org/advisories/RUSTSEC-2026-0379), [0382](https://rustsec.org/advisories/RUSTSEC-2026-0382), [0383](https://rustsec.org/advisories/RUSTSEC-2026-0383) and [0386](https://rustsec.org/advisories/RUSTSEC-2026-0386). The one that applies most directly is 0379: on aarch64, which includes Apple Silicon, Cranelift could compile a large `br_table` to jump to the wrong target. The certificate fixes cover `fetch()` and the S3, GCS and Azure stores. 0378 is in the component model and 0386 in tokio's `AsyncFd`, neither of which modebox uses directly.

### Notes

- The guest is unchanged, so continuation tokens and saved sessions from 0.1.3 resume in 0.2.0.

## [0.1.3] - 2026-10-14

### Security

- Updated rustls to 0.23.46 and wasmtime to 49.0.3, for [RUSTSEC-2026-0346](https://rustsec.org/advisories/RUSTSEC-2026-0346) to [0349](https://rustsec.org/advisories/RUSTSEC-2026-0349) (rustls) and [RUSTSEC-2026-0353](https://rustsec.org/advisories/RUSTSEC-2026-0353) to [0364](https://rustsec.org/advisories/RUSTSEC-2026-0364) (wasmtime). rustls is used by `fetch()` and the S3, GCS and Azure stores. Most of the wasmtime advisories are in the component model, which modebox doesn't use.

### Notes

- The guest is unchanged, so continuation tokens and saved sessions from 0.1.2 resume in 0.1.3.

## [0.1.2] - 2026-10-13

### Changed

- **`sh()` is opt-in in the CLI and MCP server.** Without a `"shell"` key in `modebox.json`, sandboxed code has no `sh()`. Its default allow list includes file readers such as `cat`, `head` and `rg`, and the OS sandbox blocks reads only from credential folders. Reading files outside the project should be something a config asks for. `"shell": {}` turns it on with the same defaults as before, and `modebox sh` is unchanged.

### Notes

- The guest is unchanged, so continuation tokens and saved sessions from 0.1.1 resume in 0.1.2.
- modebox has a website, [modebox.farming-labs.dev](https://modebox.farming-labs.dev). The README is shorter, and the details moved to `docs/`.

## [0.1.1] - 2026-10-12

### Added

- **Python** (`crates/modebox-python`): `run`, `run_sync`, `Runner`, `get_host_function_context` and `require_approval`, the same API as the Node package, for Python 3.10 and later. Host functions can be plain functions or `async def`; they run on the caller's event loop. Install with `pip install modebox`. There are wheels for macOS (arm64, x64) and Linux (x64, arm64, glibc 2.34 or later), and an sdist that builds with Rust 1.96 or later.

### Changed

- Releases are published from GitHub Actions with trusted publishing, so no registry token is stored anywhere. npm packages carry provenance, and PyPI files carry attestations.

### Notes

- The guest is unchanged, so continuation tokens and saved sessions from 0.1.0 resume in 0.1.1.

## [0.1.0] - 2026-10-11

The first release.

modebox runs untrusted, agent-written JavaScript and TypeScript in a WebAssembly sandbox whose only authority is the functions you give it. It plugs into agent harnesses as "code mode" over MCP. Paused runs are sealed into encrypted continuation tokens that resume later, even in another process, without running anything twice.

### Added

- **Sandbox engine** (`modebox-core`):
  - QuickJS compiled to WebAssembly and run by wasmtime. The guest imports two functions and has no WASI, so there are no files, sockets or environment.
  - Pre-initialized snapshots and copy-on-write instances.
  - Limits on memory, time, fuel (optional), host calls, concurrency, value size and log size.
  - Seeded randomness and virtual time.
- **Host functions and approvals:**
  - Typed host functions, with TypeScript declarations generated from their JSON Schemas.
  - Approval policies: never, always, or a predicate.
  - Per-call decisions, including `Engine::inspect_continuation` to see what a token is waiting on without using it.
- **Data-flow approvals:** tools can be marked as `sources` or `sinks`. Once a run has read from a source, every sink call needs approval.
- **Continuations:**
  - Encrypted, expiring and single-use, bound to the engine image and the tool set.
  - A shared `ContinuationStore` keeps tokens single-use across processes.
- **Sessions:** a VM that keeps its state across runs (REPL semantics). It can be saved to a token and restored.
- **TypeScript:** types are stripped before running, keeping source positions.
- **MCP server** (`modebox mcp`):
  - `execute` and `types` tools, with downstream MCP servers bridged in as typed functions.
  - Approvals through MRTR (`input_required`), elicitation, or an opt-in `approve` tool.
  - Retried resumes return the first result.
- **CLI** (`modebox`): `run`, `resume`, `inspect`, `sh`, `types` and `mcp`.
- **Shell sandbox** (`sh()`):
  - Seatbelt on macOS; bubblewrap plus a seccomp filter on Linux.
  - Writes only in the workspace, a read deny list for common secret stores, and a scrubbed environment.
  - Network off, fully on, or limited to listed hosts through a host-side proxy.
- **Files** (`fs.*`):
  - One object-store API over mounts: a local folder, memory, S3/R2/MinIO, GCS, Azure or HTTP.
  - Read-only, staged and direct modes, deny globs, and conditional commits.
  - A public conformance suite for new backends.
- **Web APIs:** `console`, timers, `TextEncoder` and `TextDecoder`, `atob` and `btoa`, `structuredClone`, `crypto.getRandomValues`, `URL` and `URLSearchParams`. `fetch` is available only when the host enables it.
- **Node.js** (`modebox` on npm): `run`, `createRunner`, `getHostFunctionContext` and `requireApproval`, with prebuilt bindings for macOS and Linux.
- **Integrations:** opencode (an MCP server, plus a plugin that runs `bash` under `modebox sh`), Claude Code and Codex.

### Security

- The guest never crashes on hostile values. Oversized or malformed results, logs, errors and host responses become catchable errors, not a wasm trap.
- The local directory store refuses symlinks that appear on a path after it was checked, so a race can't redirect a read or write outside the root.
- The Linux shell sandbox applies a seccomp filter on top of bubblewrap.
- `SECURITY.md` covers the threat model, the known limitations and how to report a vulnerability.

### Notes

- Building needs Rust 1.96 or later.
- Supported platforms are macOS (arm64, x64) and Linux (x64, arm64, glibc). Windows isn't supported.
- Continuation tokens and saved sessions only resume on the modebox build that made them. A new version that changes the guest makes older tokens fail with "created by a different sandbox image".
