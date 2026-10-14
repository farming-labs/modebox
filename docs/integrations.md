# Using modebox with your agent harness

modebox works with any harness that supports MCP servers: opencode, Claude Code, Codex, Cursor, and the apps that drive them, such as T3 Code. Nothing in the harness has to change. You install the `modebox` command, list your tools in `modebox.json`, and add modebox to the harness's MCP config like any other server.

Ready-made config files for opencode, Claude Code and Codex are in [`integrations/`](../integrations).

## What the agent gets

The harness starts `modebox mcp`, which offers the agent up to three tools:

| Tool | What it does |
| --- | --- |
| `types` | Shows the TypeScript signatures of everything a script can call: your MCP servers' tools as typed functions, plus `sh()`, `fs` and `fetch()` when they're on. Read-only. |
| `execute` | Runs JavaScript or TypeScript in the sandbox. State carries over from earlier calls unless the agent passes `fresh: true`. |
| `approve` | Resumes a run that paused for approval, with your decision. Only offered in some setups; see [Approvals](#approvals). |

A typical turn: the agent calls `types` to see what's available, writes one script that calls several tools, and runs it with `execute`. The script can only call the functions `types` lists. It has no files, network or environment of its own.

## 1. Install the `modebox` command

```bash
cargo install modebox    # needs Rust 1.96 or later
```

Or download it from the [latest release](https://github.com/farming-labs/modebox/releases/latest), which has a tarball and a SHA-256 checksum for each platform: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`. For example, on an Apple Silicon Mac:

```bash
curl -LO https://github.com/farming-labs/modebox/releases/latest/download/modebox-aarch64-apple-darwin.tar.gz
curl -LO https://github.com/farming-labs/modebox/releases/latest/download/modebox-aarch64-apple-darwin.tar.gz.sha256
shasum -a 256 -c modebox-aarch64-apple-darwin.tar.gz.sha256
tar xzf modebox-aarch64-apple-darwin.tar.gz
sudo mkdir -p /usr/local/bin
sudo mv modebox-aarch64-apple-darwin/modebox /usr/local/bin/
modebox --version
```

`npm install modebox` and `pip install modebox` install the library, for embedding modebox in your own app. They don't include the `modebox` command.

On Linux, `sh()` also needs bubblewrap; see [Linux](configuration.md#linux) in the configuration docs.

## 2. Write `modebox.json`

Put it at the root of your project. The folder that holds it is the shell's workspace: shell commands can write there and in temp folders, and nowhere else.

A starting point:

```json
{
  "mcpServers": {
    "github": {
      "command": "docker",
      "args": ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]
    }
  },
  "approve": ["github.*"],
  "shell": { "enabled": true, "network": false, "allow": ["ls", "rg", "git status", "git diff"] },
  "limits": { "timeoutMs": 60000 }
}
```

- **`mcpServers`**: the servers whose tools become functions, such as `github.create_issue()`. Each entry takes `command`, `args`, `env` and `cwd`, the same shape as `.mcp.json`. Only local (stdio) servers are supported for now, not remote URLs.
- **`approve`**: patterns for calls that pause for your approval. `*` matches anything.
- **`shell`**: turns on `sh()`. Commands that start with an `allow` entry and use plain arguments run without asking; anything else pauses for approval. `network` is `false`, `true`, or a list of hosts such as `["registry.npmjs.org", "*.github.com"]`.
- **`flows`**, **`fetch`**, **`fs`** and **`limits`**: data-flow rules, `fetch()`, file storage and resource limits. See [configuration.md](configuration.md) and [storage.md](storage.md).
- **`mcp.approveTool`**: whether to offer the `approve` tool. See [Approvals](#approvals).

modebox rejects fields it doesn't know, so a typo fails with an error instead of being ignored.

`"approve": ["github.*"]` makes every GitHub call ask you first. Once you've seen the tool names with `modebox -c modebox.json types`, you can narrow it to the ones that write, such as `github.create_*`, `github.update_*` and `github.merge_*`.

**Keep secrets out of the file.** Your MCP servers inherit modebox's environment, which comes from the harness. Set tokens where you start the harness, for example `export GITHUB_PERSONAL_ACCESS_TOKEN=...`. Some harnesses pass MCP servers only a few variables (Codex does; see below). Then forward the token in the harness's modebox entry, or set it in that entry's `env`.

**Check the config before connecting a harness:**

```bash
modebox -c modebox.json types
```

This starts your MCP servers and prints the TypeScript API the agent will see. If your servers' functions are listed, the config works.

## 3. Connect your harness

Every harness runs the same command: `modebox mcp -c modebox.json`. A relative path is resolved from the folder the harness starts the server in, which is usually where you started the harness. If that isn't your project root, use an absolute path.

One `execute` call can make many tool calls, so it can take longer than a typical MCP tool. Where a harness has an MCP timeout, the setups below raise it.

### opencode

Copy [`integrations/opencode/opencode.json`](../integrations/opencode/opencode.json) to your project, or merge its two blocks into your `opencode.json`:

```json
{
  "mcp": {
    "modebox": {
      "type": "local",
      "command": ["modebox", "mcp", "-c", "modebox.json"],
      "enabled": true,
      "timeout": 300000
    }
  },
  "permission": {
    "modebox_approve": "ask"
  }
}
```

- `timeout`, in milliseconds, applies to each MCP request, including tool calls. `300000` gives long scripts 5 minutes.
- `"modebox_approve": "ask"` makes opencode ask you before every call to modebox's `approve` tool. opencode allows every tool by default, so keep this line.

Then copy [`integrations/opencode/modebox.json`](../integrations/opencode/modebox.json) as your `modebox.json` and add your MCP servers to it. It turns on the `approve` tool, which is safe because of the `ask` line above.

**Optional: sandbox opencode's own shell.** opencode runs its built-in `bash` tool without an OS sandbox. Copy [`integrations/opencode/.opencode/plugin/modebox-sandbox.ts`](../integrations/opencode/.opencode/plugin/modebox-sandbox.ts) into your project's `.opencode/plugin/` folder, and every `bash` call runs through `modebox sh` instead:
- writes are limited to the project and temp folders;
- secret stores such as `~/.ssh` and `~/.aws` are unreadable;
- network is off. Set `MODEBOX_NETWORK=1` to turn it on, or `MODEBOX_ALLOW_HOSTS=registry.npmjs.org,*.github.com` to allow only those hosts.

### Claude Code

Copy [`integrations/claude-code/.mcp.json`](../integrations/claude-code/.mcp.json) to your project root:

```json
{
  "mcpServers": {
    "modebox": { "command": "modebox", "args": ["mcp", "-c", "modebox.json"] }
  }
}
```

Or let Claude Code write it:

```bash
claude mcp add --scope project modebox -- modebox mcp -c modebox.json
```

- In an interactive session, Claude Code asks you to trust a project's MCP servers the first time it sees them.
- The tools appear as `mcp__modebox__types`, `mcp__modebox__execute` and `mcp__modebox__approve`. Claude Code asks before using MCP tools unless you've allowed them; don't add `mcp__modebox__approve` to your allowed tools.
- Check the connection with `claude mcp list`, or `/mcp` inside a session.

### Codex

Add this to `~/.codex/config.toml`, or to `.codex/config.toml` in a trusted project (also in [`integrations/codex/config.toml`](../integrations/codex/config.toml)):

```toml
[mcp_servers.modebox]
command = "modebox"
args = ["mcp", "-c", "/absolute/path/to/your/project/modebox.json"]
env_vars = ["GITHUB_PERSONAL_ACCESS_TOKEN"]
tool_timeout_sec = 300
```

- Point it at your project's `modebox.json` with an absolute path.
- Codex starts MCP servers with only a few environment variables, such as `HOME` and `PATH`. `env_vars` forwards the ones your tools need, like the GitHub token.
- `tool_timeout_sec` gives long scripts up to 5 minutes; the default is 60 seconds.

### Cursor

Add modebox to `.cursor/mcp.json` in your project:

```json
{
  "mcpServers": {
    "modebox": { "command": "modebox", "args": ["mcp", "-c", "${workspaceFolder}/modebox.json"] }
  }
}
```

Cursor replaces `${workspaceFolder}` with your project's path, so the config path is absolute. To use modebox in every project, put the same entry in `~/.cursor/mcp.json`.

### T3 Code

T3 Code drives the agents installed on your machine, such as Codex, Claude Code and opencode, and they load their own MCP servers. Set modebox up in the agent you use through T3 Code, as described above, and it's available in T3 Code too. Whether you're asked before a tool runs follows T3 Code's permission mode. New threads start in Full access, which doesn't ask; see [Approvals](#approvals) for what that means for the `approve` tool.

### Any other MCP client

Add a local (stdio) server with the command `modebox` and the arguments `mcp -c /path/to/modebox.json`. If the client is a desktop app that doesn't see your shell's `PATH`, use the full path to the binary as the command; `which modebox` prints it.

## Approvals

When a script calls something that needs approval, modebox pauses the whole run. How the question reaches you depends on what the client supports, and modebox picks the best option on its own:

1. **Clients on MCP 2026-07-28:** the client shows the question, and the paused run travels with the request. modebox keeps no state, so your answer can arrive after a restart.
2. **Clients with elicitation:** you get a prompt inline, and the run continues.
3. **Everyone else:** the call is denied. The script gets a `PermissionDenied` error, and the result explains why.

For clients in the third group, `"mcp": { "approveTool": true }` in `modebox.json` offers the `approve` tool instead. The run pauses and returns an id, the agent asks you, and then calls `approve` with your answer.

**Only turn on `approveTool` if your harness asks you before every call to `approve`.** The model is the one that calls that tool. If the harness lets it through without asking, the model can approve its own actions. That means:
- **opencode:** safe with `"modebox_approve": "ask"`, as in the ready-made config.
- **Claude Code:** safe as long as `mcp__modebox__approve` isn't in your allowed tools.
- **Any mode that skips permission prompts** (opencode with every tool allowed, Claude Code with permissions bypassed, T3 Code's Full access mode, its default): keep `approveTool` off.

modebox marks `execute` and `approve` as destructive, so harnesses that run read-only tools without asking still ask about these two. `types` is read-only.

## Troubleshooting

- **The harness doesn't show modebox's tools.** Run `modebox -c modebox.json types` from the same folder. It shows errors in `modebox.json`, a missing `modebox` command, or an MCP server that fails to start. If it works there, the harness may not see the same `PATH`; use the full path to `modebox` as the command.
- **Tool calls time out.** Raise the harness's MCP timeout: `timeout` for opencode, `tool_timeout_sec` for Codex. Claude Code's default is already long; for other clients, check their MCP settings.
- **A call was denied instead of asking you.** Your client is in the third group under [Approvals](#approvals), and `approveTool` is off.
- **`sh()` says no OS sandbox is available.** On Linux, install bubblewrap and allow user namespaces; see [Linux](configuration.md#linux) in the configuration docs. modebox never runs a command unconfined.
