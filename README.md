# modebox

[![CI](https://github.com/farming-labs/modebox/actions/workflows/ci.yml/badge.svg)](https://github.com/farming-labs/modebox/actions/workflows/ci.yml)

**Code mode, sandboxed.** Your agent writes one script that calls your tools. modebox runs it in a WebAssembly sandbox whose only authority is the functions you hand it, and pauses it mid-run when a call needs your approval. The paused run is sealed into an encrypted token and can resume minutes or days later, even in another process. Nothing runs twice.

[Website](https://modebox.farming-labs.dev) · [Setup guide](docs/integrations.md) · [How it works](docs/how-it-works.md) · [Security](SECURITY.md) · [Changelog](CHANGELOG.md)

```
            agent harness (opencode · Claude Code · Codex · AI SDK · your app)
                 │ MCP: execute / types / approve          │ Node · Python · Rust API
                 ▼                                          ▼
 ┌──────────────────────────── modebox host (Rust) ────────────────────────────┐
 │ tools: MCP servers bridged as typed fns · sh() · your functions             │
 │ policy: allow / approve (glob, predicate) · limits · fuel · virtual clock   │
 │ snapshots: XOR-delta vs base image → zstd → XChaCha20-Poly1305 token        │
 │  ┌──────────────── wasmtime (pooling, CoW, epoch + fuel) ───────────────┐   │
 │  │  QuickJS-ng + Rust bridge, pre-initialized with wizer                │   │
 │  │  imports: emit(event), clock()   ← no WASI, no fs, no net, no env    │   │
 │  └──────────────────────────────────────────────────────────────────────┘   │
 │ sh(): Seatbelt (macOS) / bubblewrap (Linux): workspace-only writes,         │
 │       secret dirs unreadable, no network unless enabled, scrubbed env       │
 └─────────────────────────────────────────────────────────────────────────────┘
```

## Why modebox

- **Pause for approval, resume later.** A call that needs approval stops before it runs. The whole VM is snapshotted into an encrypted, single-use token (17 KB after 200 host calls in our benchmark), and resuming picks up exactly where it stopped.
- **Fast.** A fresh sandbox starts in about 0.1 ms: the engine is pre-initialized once, and each run gets a copy-on-write instance of it.
- **Nothing ambient.** The sandbox has no files, sockets or environment. Your tools, typed for the model, are its only way out.
- **A shell and files when you want them.** `sh()` runs commands under Seatbelt (macOS) or bubblewrap and seccomp (Linux), with the network off or limited to listed hosts. `fs.*` reads and writes local folders, S3, GCS or Azure, and staged writes wait for approval.
- **Works with your agent.** One MCP server for opencode, Claude Code, Codex, Cursor and any other MCP client, plus Node, Python and Rust APIs.

## Install

modebox runs on macOS (arm64, x64) and Linux (x64, arm64, glibc). Windows isn't supported.

```bash
cargo install modebox      # the CLI and MCP server (needs Rust 1.96+)
npm install modebox        # the Node API; the binding for your platform comes with it
pip install modebox        # the Python API (Python 3.10+); a prebuilt wheel per platform
```

Prebuilt CLI binaries for each platform, with SHA-256 checksums, are attached to every [GitHub release](https://github.com/farming-labs/modebox/releases). On Linux, `sh()` needs bubblewrap; see [Linux](docs/configuration.md#linux).

## Use it with your agent

List the MCP servers whose tools the agent's code may call, and which calls need your approval, in `modebox.json`:

```json
{
  "mcpServers": { "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] } },
  "approve": ["github.create_*", "github.merge_*"]
}
```

Then add `modebox mcp -c modebox.json` to your agent as a local MCP server. The agent gets a `types` tool with the TypeScript signatures of everything it can call, and an `execute` tool that runs its code. The [setup guide](docs/integrations.md) has the config for each agent and explains how approvals reach you. The CLI also runs scripts on its own; see [the CLI and modebox.json](docs/configuration.md).

## Use it from code

Pausing for approval and resuming later, in each language. [docs/api.md](docs/api.md) has more: plain runs with logs and stats, and sessions that keep state between runs.

### Node

```js
import { createRunner, requireApproval } from 'modebox';

const runner = createRunner({ continuationSecret: process.env.SECRET });
const hostFunctions = { payments: { refund: requireApproval(refund) } };

const r = await runner.run({ source: script, hostFunctions });   // script: the code your agent wrote
if (r.status === 'interrupted') {
  // later, even in another process:
  await runner.run({ hostFunctions, continuation: r.continuation,
                     resolutions: [{ interruptionId: r.interruptions[0].id, value: true }] });
}
```

### Python

The same API in Python 3.10+, with `async def` or plain host functions. See [crates/modebox-python](crates/modebox-python/README.md) for how values map.

```python
import asyncio
import os

import modebox

runner = modebox.Runner(continuation_secret=os.environ["SECRET"])
host_functions = {"payments": {"refund": modebox.require_approval(refund)}}

async def main():
    r = await runner.run(script, host_functions=host_functions)   # script: the code your agent wrote
    if r.status == "interrupted":
        # later, even in another process:
        await runner.run(host_functions=host_functions, continuation=r.continuation,
                         resolutions=[modebox.approve(r.interruptions[0].id)])

asyncio.run(main())
```

### Rust

```rust
use modebox_core::*;

let engine = Engine::new()?;                     // compile/cache + pre-init, once per process
let mut tools = Toolbox::new();
tools.add(Tool::new("payments.refund", refund).approval(Approval::Always));

let r = engine.run(script, &tools, RunOptions::default()).await;   // script: the code your agent wrote
if let Outcome::Suspended(s) = r.outcome {
    // later, even in another process:
    let approve = [(s.interrupts[0].id, Resolution::Approve)];
    engine.resume(&s.continuation, approve, &tools, RunOptions::default()).await;
}
```

## How it compares

✅ yes · ⚠️ partly · ❌ no

| | **modebox** | Vercel Run | opencode code mode | Codex code mode | Cloudflare Code Mode |
|---|---|---|---|---|---|
| Pause mid-script for approval | ✅ VM snapshot, nothing re-runs | ⚠️ replays the script | ❌ | ⚠️ waits in memory | ⚠️ replays the script |
| Approval survives a restart | ✅ | ✅ | ❌ | ❌ | ✅ |
| Shell from agent code, in an OS sandbox | ✅ | ❌ | ❌ | ✅ | ❌ |
| Runs where | any host: Rust, Node, Python, CLI, MCP | Node / Bun | inside opencode | inside Codex | Cloudflare Workers |
| Fast CPU-heavy code (JIT) | ❌ interpreter | ❌ interpreter | ❌ interpreter | ✅ V8 | ✅ V8 |

V8-based tools run heavy computation much faster, and all of them are more mature. The [full comparison](docs/comparison.md) also covers Claude Code, file APIs, network control and pause tokens. It's based on each project's public source and docs as of October 2026; corrections are welcome.

## Benchmarks

Against Vercel Run, the closest package with the same model (QuickJS on wasm with host functions), running the identical script on Apple M-series with Node 24:

| workload | Run 2.1.7 | modebox (Node) | |
|---|---:|---:|---:|
| trivial run | 3.57 ms | **0.12 ms** | 29× |
| one host call | 4.13 ms | **0.18 ms** | 23× |
| 200 sequential host calls in one run | 26.0 ms | **7.8 ms** | 3.3× |
| token after 200 host calls | 1.13 MB | **17 KB** | 66× smaller |
| CPU: sum 1..10M | 488 ms | 511 ms | ≈ (same interpreter) |

Measured with modebox 0.1.0 on 2026-10-10. CPU-bound code isn't faster: both run the QuickJS interpreter. All rows, the Rust figures and the caveats are in [docs/benchmarks.md](docs/benchmarks.md).

## Docs

- [Setup guide](docs/integrations.md): connect opencode, Claude Code, Codex, Cursor, T3 Code or any MCP client, and how approvals reach you
- [The CLI and `modebox.json`](docs/configuration.md): commands, data-flow rules, shell rules, Linux setup
- [Using modebox from code](docs/api.md): the Node, Python and Rust APIs
- [How it works](docs/how-it-works.md): isolation, pause tokens, approvals over MCP, `fetch()`, the shell sandbox, files
- [Storage](docs/storage.md): the `fs` API and how to add a backend
- [Benchmarks](docs/benchmarks.md) and the [full comparison](docs/comparison.md)
- [Security policy](SECURITY.md): threat model, known limitations, reporting a vulnerability
- [Contributing](CONTRIBUTING.md): building, testing and releasing

## Limitations and roadmap

- **CPU-heavy JS** is much slower than on V8, because QuickJS is an interpreter. Next: an opt-in V8-isolate tier in a separate sandboxed process.
- **Web APIs are partial.** There's no `Intl`, dates are always UTC, and locale methods format like en-US; see [Web APIs](docs/how-it-works.md#web-apis).
- **macOS and Linux (glibc) only.** Windows and musl Linux aren't supported; for musl, build `crates/modebox-node` yourself and point `MODEBOX_NATIVE` at it.
- **Sandboxed code is JavaScript or TypeScript**, including when you call modebox from Python. A Monty or CPython-wasi guest would let it run Python too, reusing the same snapshot and resume machinery.
- **Shared policy file:** one policy format that imports/exports opencode, Codex and Claude Code permission configs.

Security-relevant gaps are listed under "Known limitations" in [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
