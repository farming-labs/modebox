# The CLI and `modebox.json`

The `modebox` command runs scripts, resumes paused runs, runs shell commands in the OS sandbox, and serves MCP. Everything it can reach is set in `modebox.json`. To connect it to an agent, see the [setup guide](integrations.md).

## Commands

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

For files (`fs.mounts`), see [storage.md](storage.md).

## Linux

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
