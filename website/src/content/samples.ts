// Every code sample on the site. `scripts/highlight.ts` turns these into
// colored HTML (src/content/highlighted.json) before dev and build, so the
// highlighter itself never reaches the browser.

import type { Lang } from "../lib/highlight";

export const SAMPLES = {
  mcp: {
    lang: "json",
    code: `{
  "mcpServers": {
    "github": {
      "command": "docker",
      "args": ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]
    }
  },
  "approve": ["github.*"]
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
    code: `import asyncio
import os
import modebox

runner = modebox.Runner(continuation_secret=os.environ["SECRET"])
host_functions = {"payments": {"refund": modebox.require_approval(refund)}}

async def main():
    # \`script\` is the code your agent wrote.
    r = await runner.run(script, host_functions=host_functions)

    if r.status == "interrupted":
        # Later, even in another process:
        await runner.run(
            host_functions=host_functions,
            continuation=r.continuation,
            resolutions=[modebox.approve(r.interruptions[0].id)],
        )

asyncio.run(main())`,
  },
  rust: {
    lang: "rs",
    code: `use modebox_core::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let engine = Engine::new()?; // seals tokens with $MODEBOX_SECRET
    let mut tools = Toolbox::new();
    tools.add(Tool::new("payments.refund", refund).approval(Approval::Always));

    // \`script\` is the code your agent wrote.
    let r = engine.run(script, &tools, RunOptions::default()).await;

    if let Outcome::Suspended(s) = r.outcome {
        // Later, even in another process with the same $MODEBOX_SECRET:
        let approve = [(s.interrupts[0].id, Resolution::Approve)];
        engine.resume(&s.continuation, approve, &tools, RunOptions::default()).await;
    }
    Ok(())
}`,
  },
  release: {
    lang: "sh",
    code: `curl -LO https://github.com/farming-labs/modebox/releases/latest/download/modebox-aarch64-apple-darwin.tar.gz
curl -LO https://github.com/farming-labs/modebox/releases/latest/download/modebox-aarch64-apple-darwin.tar.gz.sha256
shasum -a 256 -c modebox-aarch64-apple-darwin.tar.gz.sha256
tar xzf modebox-aarch64-apple-darwin.tar.gz
sudo mkdir -p /usr/local/bin
sudo mv modebox-aarch64-apple-darwin/modebox /usr/local/bin/
modebox --version`,
  },
  config: {
    lang: "json",
    code: `{
  "mcpServers": {
    "github": {
      "command": "docker",
      "args": ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]
    }
  },
  "approve": ["github.*"],
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
env_vars = ["GITHUB_PERSONAL_ACCESS_TOKEN"]
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
