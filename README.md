# modebox

[![CI](https://github.com/farming-labs/modebox/actions/workflows/ci.yml/badge.svg)](https://github.com/farming-labs/modebox/actions/workflows/ci.yml)

Run untrusted, agent-written JavaScript/TypeScript in a WebAssembly sandbox whose only authority is the functions you hand it, and plug it into any agent harness as **code mode** over MCP ([setup guide](docs/integrations.md)).

When a call needs your approval, the whole VM is snapshotted into an encrypted token. The run can resume minutes or days later, even in another process, and nothing re-runs.

```
            agent harness (opencode · Claude Code · Codex · AI SDK · your app)
                 │ MCP: execute / types / approve          │ Node · Rust API
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

## How it compares

✅ yes · ⚠️ partly · ❌ no · — not applicable

| | **modebox** | Vercel Run | opencode code mode | Codex code mode | Claude Code | Cloudflare Code Mode |
|---|---|---|---|---|---|---|
| Status | prototype (v0.1) | released | experimental flag | experimental, off by default | released | released |
| Agent code runs in | QuickJS on wasm (wasmtime) | QuickJS on wasm (Node workers) | interpreter for a JS subset | V8, separate process | — (no code mode) | V8 isolates |
| Runs where | any host: Rust, Node, Python, CLI, MCP | Node / Bun | inside opencode | inside Codex | — | Cloudflare Workers |
| MCP tools as typed functions | ✅ | ⚠️ via AI SDK code mode | ✅ | ✅ | — | ✅ |
| Shell from agent code | ✅ `sh()` | ❌ | ❌ | ✅ | — | ❌ |
| Shell commands in an OS sandbox | ✅ Seatbelt / bubblewrap + seccomp | — | ❌ | ✅ Seatbelt / bubblewrap + seccomp | ✅ Seatbelt / bubblewrap | — |
| Network control for the shell | ✅ per-host proxy | — | ❌ | ✅ per-domain proxy | ✅ per-domain proxy | — |
| File / storage API for agent code | ✅ local, S3, GCS, Azure, staged writes | ❌ | ❌ | ⚠️ through its shell and file tools | — | ⚠️ through your own bindings |
| Pause mid-script for approval | ✅ VM snapshot, nothing re-runs | ⚠️ replays the script | ❌ | ⚠️ waits in memory | — | ⚠️ replays the script |
| Approval survives a restart | ✅ | ✅ | ❌ | ❌ | ❌ | ✅ |
| Pause token | encrypted, single-use | signed but readable, reusable | — | — | — | stored server-side |
| State kept between runs | ✅ sessions, savable | ❌ | ❌ | ✅ store / load | — | ❌ |
| Fast CPU-heavy code (JIT) | ❌ interpreter | ❌ interpreter | ❌ interpreter | ✅ V8 | — | ✅ V8 |

Where others are ahead today: V8-based tools (Codex, Cloudflare) run heavy computation much faster, and all of them are more mature.

Based on each project's public source and docs as of October 2026. These tools change quickly; corrections are welcome.

## Benchmarks

Measured against Vercel Run, the closest package with the same model (QuickJS on wasm with host functions). Apple M-series, Node 24. `bench/compare.mjs` runs the identical script against both packages, with JS host functions on both sides.

| workload | Run 2.1.7 | modebox (Node) | |
|---|---:|---:|---:|
| trivial run | 3.57 ms | **0.12 ms** | 29× |
| one host call | 4.13 ms | **0.18 ms** | 23× |
| 200 sequential host calls in one run | 26.0 ms | **7.8 ms** | 3.3× |
| 8 parallel runs, per run | 1.48 ms | **0.06 ms** | 24× |
| return a 1 MB string | 531 ms | **11 ms** | 47× |
| host returns 5,000 rows | 62 ms | **23 ms** | 2.7× |
| interrupt + resume | 18.5 ms | **4.0 ms** | 4.7× |
| token after 200 host calls | 1.13 MB | **17 KB** | 66× smaller |
| resume after 200 host calls | 30.9 ms | **2.2 ms** | 14× |
| CPU: sum 1..10M | 488 ms | 511 ms | ≈ (same interpreter) |

These numbers are from modebox 0.1.0 (wasmtime 44), measured on 2026-10-10; 0.1.1 hasn't been re-measured yet. The two Node resume rows are too high: the benchmark's resume made the tool pause a second time, so they time two pauses instead of one. The Rust figures below time a single resume.

From Rust directly (`cargo run --release -p modebox-core --example bench`), a trivial run is 0.096 ms, interrupt + resume is 1.8 ms, and resume after 200 calls is 0.28 ms.

**Where it isn't faster:** CPU-bound JS. Both run the QuickJS bytecode interpreter compiled to wasm. Fuel metering costs about 10%; turn it off with `EngineConfig { metering: false }`. If heavy compute matters, a V8-isolate tier is the next step (see the roadmap).

## Install

modebox runs on macOS (arm64, x64) and Linux (x64, arm64, glibc). Windows isn't supported.

```bash
cargo install modebox      # the CLI and MCP server (needs Rust 1.96+)
npm install modebox        # the Node API; the binding for your platform comes with it
pip install modebox        # the Python API (Python 3.10+); a prebuilt wheel per platform
```

Prebuilt CLI binaries for each platform, with SHA-256 checksums, are attached to every [GitHub release](https://github.com/farming-labs/modebox/releases). To build from source:

```bash
cargo build --release -p modebox      # ./target/release/modebox
pip install ./crates/modebox-python   # the Python API (needs Rust 1.96+)
./scripts/build-guest.sh              # only if you change guest/: rebuilds the QuickJS wasm (needs wasi-sdk 24)
```

## Quick start

### CLI

```bash
modebox run -e 'const xs = [3, 1, 2]; xs.sort()'
modebox -c modebox.example.json types            # TypeScript API the model sees
modebox -c modebox.example.json run script.ts    # prompts y/N for approvals
modebox run --json script.ts > paused.json       # or pause, then decide later:
modebox inspect "$(jq -r .continuation paused.json)"                   # the pending calls
modebox resume "$(jq -r .continuation paused.json)" --approve-only 3   # approve call 3, deny the rest
modebox sh -- 'npm test'                         # one command in the OS sandbox
modebox -c modebox.json mcp                      # MCP server (stdio)
```

`modebox.json` (`mcpServers` has the same shape as `.mcp.json`):

```json
{
  "mcpServers": { "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] } },
  "approve": ["github.create_*", "github.merge_*"],
  "shell": { "enabled": true, "network": ["registry.npmjs.org", "*.github.com"], "allow": ["ls", "rg", "git status", "git diff"] },
  "fetch": { "allow": ["api.github.com"] },
  "limits": { "timeoutMs": 60000, "maxHostCalls": 500 }
}
```

**Data flow.** Mark tools that return private data as `sources`, and tools that can send data out as `sinks`. Once a run (or session) has called a source, every call to a sink pauses for approval, even if it's allowed otherwise:

```json
"flows": { "sources": ["gmail.*", "fs.get"], "sinks": ["slack.post_*", "http.*"] }
```

Shell commands suspend for approval unless they start with an `allow` entry (word for word), use plain arguments only (no quotes, `$`, `&`, `|`, `;`, redirects, globs or braces), and avoid write/exec flags such as `find -delete` or `rg --pre`.

#### Linux

`sh()` needs bubblewrap and unprivileged user namespaces. modebox checks both on first use. If either is missing, `modebox sh` and `sh()` fail with the reason instead of running anything unconfined.

```bash
sudo apt-get install bubblewrap        # or dnf / pacman / apk
```

Ubuntu 23.10 and later block unprivileged user namespaces through AppArmor. Give bwrap a profile that allows them:

```bash
sudo tee /etc/apparmor.d/bwrap <<'EOF'
abi <abi/4.0>,
include <tunables/global>

profile bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,
}
EOF
sudo apparmor_parser -r /etc/apparmor.d/bwrap
```

This is the narrow fix, but it still lets any local user run bwrap with user namespaces. The broad one is `sysctl kernel.apparmor_restrict_unprivileged_userns=0`. Inside Docker, user namespaces are usually off unless the container runs with a seccomp profile that allows them.

### Rust

```rust
use modebox_core::*;

let engine = Engine::new()?;                     // compile/cache + pre-init, once per process
let mut tools = Toolbox::new();
tools.func("db.users", |_c: Call| async { Ok(Value::from(serde_json::json!([{"name": "ada"}]))) });
tools.add(Tool::new("bank.transfer", transfer).approval(Approval::Always));

let r = engine.run("const u = await db.users(); u.length", &tools, RunOptions::default()).await;
match r.outcome {
    Outcome::Completed(v) => println!("{v:?}"),
    Outcome::Failed(e) => eprintln!("{e}"),
    Outcome::Suspended(s) => {
        // ask a human, later, maybe in another process:
        let r = engine.resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &tools, RunOptions::default()).await;
    }
}

// notebook-style session
let mut s = engine.session(tools, RunOptions::default());
s.run("const users = await db.users()").await;
s.run("users.filter(u => u.name.startsWith('a'))").await;
let token = s.save().await;                      // restore with engine.restore_session(token, ...)
```

### Node

```js
import { run, createRunner, getHostFunctionContext, requireApproval } from 'modebox';

const { value, logs, stats } = await run({
  source: 'const d = await tools.double(21); console.log(d); d',
  hostFunctions: { tools: { double: (x) => x * 2 } },
});

const runner = createRunner({ continuationSecret: process.env.SECRET });
const r = await runner.run({ source, hostFunctions: { bank: { send: requireApproval(send) } } });
if (r.status === 'interrupted') {
  await runner.run({ hostFunctions, continuation: r.continuation,
                     resolutions: [{ interruptionId: r.interruptions[0].id, value: true }] });
}
```

### Python

The same API in Python 3.10+, with `async def` or plain host functions. See [crates/modebox-python](crates/modebox-python/README.md) for how values map.

```python
import os

import modebox

result = await modebox.run(
    "const d = await tools.double(21); console.log(d); d",
    host_functions={"tools": {"double": lambda x: x * 2}},
)
print(result.value, result.logs, result.stats)

runner = modebox.Runner(continuation_secret=os.environ["SECRET"])
r = await runner.run(source, host_functions={"bank": {"send": modebox.require_approval(send)}})
if r.status == "interrupted":
    await runner.run(host_functions=host_functions, continuation=r.continuation,
                     resolutions=[modebox.approve(r.interruptions[0].id)])
```

## Files: any storage as one object store

`fs.*` gives sandboxed code one object-store API over **mounts**. A mount can be a local folder, memory, S3 / R2 / MinIO, GCS, Azure, HTTP, or your own backend implementing the six-method `Store` trait. Each mount is `read-only`, `staged` (writes wait for an approved commit) or `direct`. The full contract, the conformance suite, and a proposed out-of-process driver protocol are in [docs/storage.md](docs/storage.md).

```ts
const { objects } = await fs.list({ prefix: 'src/', glob: '**/*.ts' });   // respects .gitignore
const cfg = await fs.get({ key: 'package.json', as: 'json' });            // text | bytes | json, byte ranges
const hits = await fs.grep({ pattern: 'oldApi', glob: 'src/**' });        // searches staged versions too
await fs.put({ key: 'src/a.ts', body: newSource, ifMatch: etag });        // staged, optimistic concurrency
await fs.changes();                                                       // unified diffs of what's staged
await fs.commit({ message: 'rename oldApi' });                            // pauses for approval, then writes
```

- **Mounts.** Configure them in `modebox.json` under `fs.mounts`, for example `{"datasets/": {"url": "s3://bucket/prefix", "readOnly": true}}`. Credentials are given as `$ENV` references and stay on the host.
- **Jail.** `..` is rejected, and symlinks are resolved and checked against the mount root. Files are then opened refusing any symlink, so a path swapped for one after the check fails instead of escaping. Writes replace files rather than editing them, so a hard link can't carry a write outside the root. `.env*`, keys, `.git/` and similar are denied by default.
- **Staging.** Staged writes are content-addressed blobs, capped at 64 MiB of uncommitted changes (and 64 MiB for `tmp/`); past that, writes fail with `ENOSPC`. They're sealed into continuation tokens and saved sessions, so a run paused in one process and approved in another applies exactly what was staged.
- **Commit.** Commits use conditional writes (`IfMatch` / `IfNotExists`). Anything that changed in its store since the code first read it is reported as a conflict instead of being overwritten.

## Integrations

[docs/integrations.md](docs/integrations.md) is the step-by-step setup for opencode, Claude Code, Codex, Cursor, T3 Code and any other MCP client: installing the CLI, writing `modebox.json`, each harness's config, and how approvals reach you. The ready-made files it uses are in `integrations/`.

**opencode**, in `integrations/opencode/`:
- `opencode.json` adds the MCP server. MCP requests time out after 5 s by default, so the snippet raises the timeout. It also sets `"modebox_approve": "ask"`, because opencode allows every tool by default. That prompt is what makes it safe to turn on the `approve` tool with `"mcp": { "approveTool": true }` in `modebox.json`.
- `modebox.json` is a starting config with the `approve` tool on for that setup. Add your MCP servers under `mcpServers`.
- `.opencode/plugin/modebox-sandbox.ts` routes every built-in `bash` call through `modebox sh`. opencode has no OS sandbox of its own, and this adds one without changing how the agent works. Network stays off, unless `MODEBOX_NETWORK=1` turns it on fully or `MODEBOX_ALLOW_HOSTS=registry.npmjs.org,*.github.com` lists the hosts it may reach.

**Claude Code**: `integrations/claude-code/.mcp.json`. **Codex**: `integrations/codex/config.toml`.

How approvals reach the user, per client capability (`tests/fixtures/drive-mcp.mjs` exercises each one):
1. **MCP 2026-07-28 clients**: the server returns `input_required` with an elicitation, and the sealed VM goes in `requestState`. The server keeps no state, and an approval can arrive after a restart.
2. **Clients with elicitation**: the user gets an inline prompt and the run continues.
3. **Everyone else**: by default the call is denied. The code gets a `PermissionDenied` error, and the result explains why. Only the model could answer an `approve` tool, so it's off unless `modebox.json` sets `"mcp": { "approveTool": true }`. Set it only if the harness asks the user before each call to that tool. Then the run pauses and returns an id, and `approve` resumes it.

`execute` and `approve` are annotated as destructive and open-world, so harnesses that auto-allow read-only tools still ask about them. `types` is read-only.

## Security model

The full threat model, what isn't defended against, and how to report a vulnerability are in [SECURITY.md](SECURITY.md).

- **The guest can't reach anything.** The wasm module imports exactly two functions: `modebox.emit` (events to the host) and a clock. There's no WASI at all, so no files, sockets, env or randomness except what the host seeds. `require`, `process` and `import()` don't exist. `fetch` exists only if the host enables it (below).
- **The only authority is your tools.** The host validates every call name against the toolbox. Arguments and results are size-limited and depth-limited, with cycle detection.
- **Defense in depth.** A memory-safety bug in QuickJS stays inside wasm linear memory, behind wasmtime guard pages, Cranelift bounds checks, and a per-instance memory cap.
- **Tokens.** They are XChaCha20-Poly1305 sealed, so they're confidential and tamper-proof, and they expire. The `modebox` CLI and MCP server seal them with `$MODEBOX_SECRET`, or else with a key generated on first use and saved at `~/.config/modebox/secret` (owner-only), so they still open after a restart. Library users choose their own key (`EngineConfig::secret`); without one, tokens only open in the process that made them. Each is bound to the exact engine image (a hash of the base memory) and to a hash of the tool set. They're single-use. The CLI and MCP server record used tokens under `~/.local/state/modebox/continuations` (or `$MODEBOX_STATE_DIR`), so that holds across processes too. Library users get an in-memory record by default. Processes that resume each other's tokens should share a `StoreContinuations` over any store with atomic create, such as a directory or a GCS/Azure bucket (`continuationsDir` in Node).
- **Resumes can't widen limits.** The limits are sealed into the token.
- **`fetch()` goes through the host.** With `"fetch"` in `modebox.json`, `fetch`, `Request`, `Response` and `Headers` work as in browsers. The host makes each request: listed hosts go through, others ask for approval (or are refused with `"askForOthers": false`). Names are resolved and checked by the host, so only exact entries can reach private addresses, and redirects stay within reach. Responses are capped (8 MiB by default). Embedders can provide their own `$fetch` tool instead; the contract is in `crates/modebox-core/src/fetch.rs`.
- **`sh()` runs under Seatbelt or bubblewrap.** On Linux, a seccomp filter also refuses syscalls commands don't need, such as creating namespaces, mounting, `ptrace`, BPF and io_uring. Writes are allowed only under the workspace and temp dirs. `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.kube`, `~/.docker`, `~/.config/gh` and `.netrc` can't be read. Network is off by default. It can be fully on, or limited to listed hosts through a proxy on the host. That proxy resolves names itself, and only exact entries may reach private addresses. On Linux, a bridge inside the sandbox's network namespace forwards to it. The environment is scrubbed, and the whole process group is killed on timeout.

## Limitations and roadmap

Security-relevant gaps are listed under "Known limitations" in [SECURITY.md](SECURITY.md).

- **CPU-heavy JS:** QuickJS is an interpreter, so heavy computation is much slower than on V8. Next: an opt-in V8-isolate tier in a separate sandboxed process.
- **Web APIs:** `URL` and `URLSearchParams` follow the WHATWG standard; the `url` crate parses them. There's no `Intl`. Dates are always UTC, with no local time zone. `toLocaleString()` and the other locale methods ignore the locale and format like en-US, numbers aren't grouped, and `localeCompare` compares UTF-16 code units.
- **Platforms:** macOS (arm64, x64) and Linux (x64, arm64, glibc). The shell sandbox is Seatbelt on macOS and bubblewrap plus a seccomp filter on Linux, and CI runs both on every change. Windows and musl Linux aren't supported; for musl, build `crates/modebox-node` yourself and point `MODEBOX_NATIVE` at it.
- **Python inside the sandbox:** sandboxed code is JavaScript or TypeScript, including when you call modebox from Python. A Monty or CPython-wasi guest would let it run Python too, reusing the same snapshot and resume machinery.
- **Shared policy file:** one policy format that imports/exports opencode, Codex and Claude Code permission configs.

## Development

Releases are cut by pushing a `vX.Y.Z` tag; see [docs/releasing.md](docs/releasing.md). Changes are listed in [CHANGELOG.md](CHANGELOG.md). To contribute, see [CONTRIBUTING.md](CONTRIBUTING.md).

```
guest/                 QuickJS guest (wasm32-unknown-unknown): event loop, codec, prelude
crates/modebox-core    engine, run loop, snapshots, sessions, tools, TS, d.ts, shell
crates/modebox-cli     the `modebox` crate and binary: run / resume / inspect / sh / types / mcp
crates/modebox-node    napi-rs binding; packages/modebox is the JS API
crates/modebox-python  PyO3 binding; python/modebox is the Python API
bench/                 compare.mjs (Run vs modebox), results
tests/fixtures         notes-mcp.mjs (downstream MCP server), drive-mcp.mjs (harness simulator)
```

```bash
cargo test --release --workspace                  # release: some tests assert timings
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p modebox -p modebox-node -p modebox-python
cp target/release/libmodebox_node.dylib packages/modebox/modebox.node   # .so on Linux
(cd packages/modebox && npm test)
cp target/release/lib_native.dylib crates/modebox-python/python/modebox/_native.abi3.so   # .so on Linux
(cd crates/modebox-python && PYTHONPATH=python python3 -m unittest discover -s tests)
node tests/fixtures/drive-mcp.mjs legacy|elicit|mrtr|deny
node bench/compare.mjs run && node bench/compare.mjs modebox
```

CI (`.github/workflows/ci.yml`) runs these on Linux and macOS (the Python tests on Python 3.10, the oldest supported), checks the workspace builds on Rust 1.96 (the minimum), fails on RustSec advisories against any dependency, and rebuilds the guest to confirm the committed wasm matches byte for byte.

## License

MIT. See [LICENSE](LICENSE).
