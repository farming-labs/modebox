# Changelog

Notable changes to modebox. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/). Before 1.0, a minor version may break APIs.

## [Unreleased]

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
