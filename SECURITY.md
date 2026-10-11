# Security policy

modebox runs code that a model wrote, maybe under the influence of a prompt injection. So a bug that lets that code do more than it should is a security bug, and we want to hear about it.

## Reporting a vulnerability

Please report privately, not in a public issue:

- Use GitHub's private vulnerability reporting: the **Security** tab of [farming-labs/modebox](https://github.com/farming-labs/modebox/security), then **Report a vulnerability**.
- If that option isn't there, open an issue asking for a private contact. Leave out any details of the problem.

Include what you ran, what happened and what you expected, and the modebox commit you were on. Fixes land on `main`; there are no maintained release branches yet.

## Threat model

### What is untrusted

- The JavaScript/TypeScript run in the sandbox, and every value it produces: results, tool arguments, logs and errors.
- Results from downstream MCP servers. They're passed into the sandbox as data, never as instructions to the host.
- Continuation tokens from a client. They're opened only if they authenticate.

### What is trusted

- The host process and its configuration (`modebox.json`, `EngineConfig`, `RunOptions`).
- The tools you register. They run with your privileges, so a tool is exactly as safe as what it does with its arguments.
- The person who answers approval prompts.
- The machine's OS and kernel. The shell sandbox relies on them.

### What modebox enforces

- **Isolation of sandboxed code.** It runs in QuickJS compiled to WebAssembly, under wasmtime. The module imports two functions: an event channel and a clock. There's no WASI, so no files, sockets, environment or ambient randomness. Each run has caps on memory, wall-clock time, fuel (optional), host calls, concurrency, value size and log size. A crash inside the guest ends that run and nothing else.
- **Authority only through tools.** Code can call only the functions in its toolbox. Calls that need approval stop *before* the host function runs.
- **Continuations.**
  - Tokens are sealed with XChaCha20-Poly1305 and expire.
  - Each token is bound to the engine image and the tool set, and carries the run's limits.
  - A token resumes once. The CLI and MCP server record used tokens on disk, so this holds across processes. Library users choose where that record lives (`EngineConfig::continuations`).
- **The shell tool.** `sh()` runs under Seatbelt (macOS) or bubblewrap (Linux):
  - Writes are allowed only in the workspace and temp dirs.
  - Common credential stores can't be read.
  - Network is off unless enabled, the environment is scrubbed, and a timeout kills the whole process group.
  - If the sandbox can't start, `sh()` refuses to run rather than running unconfined.
- **Files.** The `fs` capability sees only the configured mounts. Deny globs hide secrets such as `.env`, keys and `.git`. Writes are staged, and they commit only if the file hasn't changed since it was read.

## Known limitations

These are known gaps. Reports that show how they can be exploited in practice are still welcome.

- **An allowed tool is allowed for any purpose.**
  - A run that can read secrets through one tool and send data through another can send the secrets.
  - Nothing yet tracks data flow between tools. Require approval for tools that send data out of your trust boundary.
- **Approvals are only as good as the approver.**
  - The prompt shows the call and its arguments. Someone has to read them.
  - For MCP clients without elicitation, the `approve` tool lets the model answer. So it's off unless `mcp.approveTool` is set, and without it those calls are denied. Turn it on only if the harness asks the user before every call to that tool. Prefer clients that support elicitation or MRTR, where the prompt goes to the user.
- **Retries.** A token is used up when a resume starts. The MCP server answers a retried resume with the first result for ten minutes. A retry that reaches a restarted server is refused.
- **The shell sandbox is a process sandbox, not a VM.**
  - Commands share the kernel with the host.
  - Linux has no seccomp filter yet.
  - Network access is all or nothing.
  - Reads are allowed everywhere except a deny list, so secrets outside those directories can be read: project `.env` files, for example. Keep secrets out of reach, or don't enable the shell.
  - On macOS, Apple has deprecated the `sandbox-exec` command. It still works, and Codex uses it too.
  - Windows has no shell sandbox.
- **Local files and races.** The local directory store checks paths before it opens them. Another local process that swaps a path for a symlink between the check and the open can redirect the access. Hard links to files outside the root aren't detected.
- **Denial of service on the host.** Limits bound a single run. Many concurrent runs, or tools that are expensive to call, can still exhaust the host. Put your own limits on concurrency and on what tools do.
- **Side channels.** Sandboxed code shares a process with the host. Timing and resource-usage side channels aren't defended against beyond what wasmtime provides.
- **Keys and tokens.**
  - Anyone who can read the continuation key (`$MODEBOX_SECRET`, or `~/.config/modebox/secret`) can open and forge tokens.
  - A token is a bearer capability until it's used or expires. Store and send tokens the way you would credentials.
- **Dependencies.** Isolation also depends on QuickJS, wasmtime, and the platform sandboxes being correct. None of them is formally verified.
