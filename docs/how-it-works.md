# How modebox works

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

## Isolation and authority

The full threat model, what isn't defended against, and how to report a vulnerability are in [SECURITY.md](../SECURITY.md).

- **The guest can't reach anything.** The wasm module imports exactly two functions: `modebox.emit` (events to the host) and a clock. There's no WASI at all, so no files, sockets, env or randomness except what the host seeds. `require`, `process` and `import()` don't exist. `fetch` exists only if the host enables it (below).
- **The only authority is your tools.** The host validates every call name against the toolbox. Arguments and results are size-limited and depth-limited, with cycle detection.
- **Defense in depth.** A memory-safety bug in QuickJS stays inside wasm linear memory, behind wasmtime guard pages, Cranelift bounds checks, and a per-instance memory cap.
- **Tokens.** They are XChaCha20-Poly1305 sealed, so they're confidential and tamper-proof, and they expire. The `modebox` CLI and MCP server seal them with `$MODEBOX_SECRET`, or else with a key generated on first use and saved at `~/.config/modebox/secret` (owner-only), so they still open after a restart. Library users choose their own key (`EngineConfig::secret`); without one, tokens only open in the process that made them. Each is bound to the exact engine image (a hash of the base memory) and to a hash of the tool set. They're single-use. The CLI and MCP server record used tokens under `~/.local/state/modebox/continuations` (or `$MODEBOX_STATE_DIR`), so that holds across processes too. Library users get an in-memory record by default. Processes that resume each other's tokens should share a `StoreContinuations` over any store with atomic create, such as a directory or a GCS/Azure bucket (`continuationsDir` in Node).
- **Resumes can't widen limits.** The limits are sealed into the token.
- **`fetch()` goes through the host.** With `"fetch"` in `modebox.json`, `fetch`, `Request`, `Response` and `Headers` work as in browsers. The host makes each request: listed hosts go through, others ask for approval (or are refused with `"askForOthers": false`). Names are resolved and checked by the host, so only exact entries can reach private addresses, and redirects stay within reach. Responses are capped (8 MiB by default). Embedders can provide their own `$fetch` tool instead; the contract is in `crates/modebox-core/src/fetch.rs`.
- **`sh()` runs under Seatbelt or bubblewrap.** On Linux, a seccomp filter also refuses syscalls commands don't need, such as creating namespaces, mounting, `ptrace`, BPF and io_uring. Writes are allowed only under the workspace and temp dirs. `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.kube`, `~/.docker`, `~/.config/gh` and `.netrc` can't be read. Network is off by default. It can be fully on, or limited to listed hosts through a proxy on the host. That proxy resolves names itself, and only exact entries may reach private addresses. On Linux, a bridge inside the sandbox's network namespace forwards to it. The environment is scrubbed, and the whole process group is killed on timeout.

## Approvals over MCP

The [setup guide](integrations.md#approvals) explains this for users. How approvals reach the user, per client capability (`tests/fixtures/drive-mcp.mjs` exercises each one):

1. **MCP 2026-07-28 clients**: the server returns `input_required` with an elicitation, and the sealed VM goes in `requestState`. The server keeps no state, and an approval can arrive after a restart.
2. **Clients with elicitation**: the user gets an inline prompt and the run continues.
3. **Everyone else**: by default the call is denied. The code gets a `PermissionDenied` error, and the result explains why. Only the model could answer an `approve` tool, so it's off unless `modebox.json` sets `"mcp": { "approveTool": true }`. Set it only if the harness asks the user before each call to that tool. Then the run pauses and returns an id, and `approve` resumes it.

`execute` and `approve` are annotated as destructive and open-world, so harnesses that auto-allow read-only tools still ask about them. `types` is read-only.

## Files

`fs.*` gives sandboxed code one object-store API over **mounts**. A mount can be a local folder, memory, S3 / R2 / MinIO, GCS, Azure, HTTP, or your own backend implementing the six-method `Store` trait. Each mount is `read-only`, `staged` (writes wait for an approved commit) or `direct`. The full contract, the conformance suite, and a proposed out-of-process driver protocol are in [storage.md](storage.md).

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

## Web APIs

`URL` and `URLSearchParams` follow the WHATWG standard; the `url` crate parses them. There's no `Intl`. Dates are always UTC, with no local time zone. `toLocaleString()` and the other locale methods ignore the locale and format like en-US, numbers aren't grouped, and `localeCompare` compares UTF-16 code units.
