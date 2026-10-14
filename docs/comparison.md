# How modebox compares

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
