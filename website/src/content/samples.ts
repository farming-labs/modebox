// Every code sample on the site. `scripts/highlight.ts` turns these into
// colored HTML (src/content/highlighted.json) before dev and build, so the
// highlighter itself never reaches the browser.

import type { Lang } from "../lib/highlight";

export const SAMPLES = {
  mcp: {
    lang: "json",
    code: `{
  "mcpServers": {
    "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] }
  },
  "approve": ["github.create_*", "github.merge_*"]
}`,
  },
  node: {
    lang: "ts",
    code: `import { createRunner, requireApproval } from "modebox";

const runner = createRunner({ continuationSecret: process.env.SECRET });
const hostFunctions = {
  payments: { refund: requireApproval(refund) },
};

// \`script\` is the code your agent wrote.
const r = await runner.run({ source: script, hostFunctions });

if (r.status === "interrupted") {
  // Later, even in another process:
  await runner.run({
    hostFunctions,
    continuation: r.continuation,
    resolutions: [{ interruptionId: r.interruptions[0].id, value: true }],
  });
}`,
  },
  python: {
    lang: "py",
    code: `import os
import modebox

runner = modebox.Runner(continuation_secret=os.environ["SECRET"])
host_functions = {"payments": {"refund": modebox.require_approval(refund)}}

# \`script\` is the code your agent wrote.
r = await runner.run(script, host_functions=host_functions)

if r.status == "interrupted":
    # Later, even in another process:
    await runner.run(
        host_functions=host_functions,
        continuation=r.continuation,
        resolutions=[modebox.approve(r.interruptions[0].id)],
    )`,
  },
  rust: {
    lang: "rs",
    code: `use modebox_core::*;

let engine = Engine::new()?;
let mut tools = Toolbox::new();
tools.add(Tool::new("payments.refund", refund).approval(Approval::Always));

// \`script\` is the code your agent wrote.
let r = engine.run(script, &tools, RunOptions::default()).await;

if let Outcome::Suspended(s) = r.outcome {
    // Later, even in another process:
    let approve = [(s.interrupts[0].id, Resolution::Approve)];
    engine.resume(&s.continuation, approve, &tools, RunOptions::default()).await;
}`,
  },
  release: {
    lang: "sh",
    code: `curl -LO https://github.com/farming-labs/modebox/releases/latest/download/modebox-aarch64-apple-darwin.tar.gz
tar xzf modebox-aarch64-apple-darwin.tar.gz
sudo mv modebox-aarch64-apple-darwin/modebox /usr/local/bin/
modebox --version`,
  },
  config: {
    lang: "json",
    code: `{
  "mcpServers": {
    "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] }
  },
  "approve": ["github.create_*", "github.merge_*"],
  "shell": { "enabled": true, "network": false, "allow": ["ls", "rg", "git status", "git diff"] },
  "limits": { "timeoutMs": 60000 }
}`,
  },
  opencode: {
    lang: "json",
    code: `{
  "$schema": "https://opencode.ai/config.json",
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
}`,
  },
  claude: {
    lang: "json",
    code: `{
  "mcpServers": {
    "modebox": { "command": "modebox", "args": ["mcp", "-c", "modebox.json"] }
  }
}`,
  },
  codex: {
    lang: "toml",
    code: `[mcp_servers.modebox]
command = "modebox"
args = ["mcp", "-c", "/absolute/path/to/your/project/modebox.json"]
tool_timeout_sec = 300`,
  },
  cursor: {
    lang: "json",
    code: `{
  "mcpServers": {
    "modebox": { "command": "modebox", "args": ["mcp", "-c", "\${workspaceFolder}/modebox.json"] }
  }
}`,
  },
} satisfies Record<string, { lang: Lang; code: string }>;

export type SampleId = keyof typeof SAMPLES;
