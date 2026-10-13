import type { Metadata } from "@farm.js/core";
import { Cmd, CodeBlock, Rich } from "../../components/code-block";
import { CopyDelegate } from "../../components/copy-delegate";
import { GitHubIcon } from "../../components/github-icon";
import { HarnessTabs, type Harness } from "../../components/harness-tabs";
import { REPO, SiteFooter, SiteHeader } from "../../components/site-chrome";
import HIGHLIGHTED from "../../content/highlighted.json";
import { SAMPLES, type SampleId } from "../../content/samples";

// The same content as docs/integrations.md in the repo, laid out for the site.

const TITLE = "Setup guide · modebox";
const DESCRIPTION = "Connect modebox to opencode, Claude Code, Codex, Cursor, T3 Code or any MCP client.";

export const metadata: Metadata = {
  title: TITLE,
  description: DESCRIPTION,
  openGraph: { url: "/setup", title: TITLE, description: DESCRIPTION },
  twitter: { card: "summary_large_image", title: TITLE, description: DESCRIPTION },
};

const GUIDE_SOURCE = `${REPO}/blob/main/docs/integrations.md`;

function block(file: string, id: SampleId) {
  return { file, code: SAMPLES[id].code, html: HIGHLIGHTED[id] };
}

const RELEASE = block("macOS, Apple Silicon", "release");

const CONFIG = block("modebox.json", "config");

const FIELDS = [
  { key: "mcpServers", what: "Servers whose tools become functions, like `github.create_issue()`. Local (stdio) servers only." },
  { key: "approve", what: "Patterns for calls that pause for your approval. `*` matches anything." },
  { key: "shell", what: "Turns on `sh()`. Allowed commands run; anything else pauses. `network` is off, on, or a list of hosts." },
  { key: "flows · fetch · fs · limits", what: "Data-flow rules, `fetch()`, file storage and resource limits." },
  { key: "mcp.approveTool", what: "Whether to offer the `approve` tool. See approvals below." },
];

function harness(h: Omit<Harness, "html">): Harness {
  return h.code ? { ...h, html: HIGHLIGHTED[h.id as SampleId] } : h;
}

const HARNESSES: Harness[] = [
  harness({
    id: "opencode",
    label: "opencode",
    file: "opencode.json",
    code: SAMPLES.opencode.code,
    notes: [
      "`timeout` raises opencode's 5-second MCP timeout to 5 minutes.",
      '`"modebox_approve": "ask"` makes opencode ask you before every `approve` call. Keep it: opencode allows every tool by default.',
      "Use `integrations/opencode/modebox.json` from the repo as your `modebox.json`. It turns on the `approve` tool.",
      "Optional: copy `integrations/opencode/.opencode/plugin/modebox-sandbox.ts` into `.opencode/plugin/`, and opencode's own `bash` tool runs in modebox's OS sandbox too.",
    ],
  }),
  harness({
    id: "claude",
    label: "Claude Code",
    file: ".mcp.json",
    code: SAMPLES.claude.code,
    cmd: "claude mcp add --scope project modebox -- modebox mcp -c modebox.json",
    notes: [
      "Put the file at your project root, or let Claude Code write it with the command above.",
      "Claude Code asks you to trust a project's MCP servers the first time it sees them.",
      "Don't add `mcp__modebox__approve` to your allowed tools.",
      "For long scripts, start it with `MCP_TOOL_TIMEOUT=300000 claude`. Check the connection with `claude mcp list` or `/mcp`.",
    ],
  }),
  harness({
    id: "codex",
    label: "Codex",
    file: "~/.codex/config.toml",
    code: SAMPLES.codex.code,
    notes: [
      "Codex's config is global, so point it at your project's `modebox.json` with an absolute path.",
      "`tool_timeout_sec` gives long scripts up to 5 minutes.",
    ],
  }),
  harness({
    id: "cursor",
    label: "Cursor",
    file: ".cursor/mcp.json",
    code: SAMPLES.cursor.code,
    notes: [
      "Cursor fills in `${workspaceFolder}`, so the path is absolute.",
      "To use modebox in every project, put the same entry in `~/.cursor/mcp.json`.",
    ],
  }),
  harness({
    id: "t3",
    label: "T3 Code",
    notes: [
      "T3 Code drives the agents installed on your machine, such as Codex, Claude Code and opencode. It doesn't start MCP servers itself.",
      "Set modebox up in the agent you use through T3 Code, using its tab here, and it's available in T3 Code too.",
      "In T3 Code's Full Access mode, keep `approveTool` off; see approvals below.",
    ],
  }),
  harness({
    id: "other",
    label: "Any MCP client",
    cmd: "modebox mcp -c /path/to/modebox.json",
    notes: [
      "Add a local (stdio) server that runs this command.",
      "Desktop apps may not see your shell's `PATH`. If so, use the full path that `which modebox` prints.",
    ],
  }),
];

const GETS = [
  { key: "types", what: "Shows the TypeScript signatures of everything a script can call." },
  { key: "execute", what: "Runs the agent's JavaScript or TypeScript in the sandbox." },
  { key: "approve", what: "Resumes a paused run with your decision. Only in some setups." },
];

const APPROVALS = [
  {
    key: "MCP 2026-07-28 clients",
    what: "The client asks you, and the paused run travels with the request, so your answer can come after a restart.",
  },
  { key: "Clients that can prompt", what: "You get a prompt inline, and the run continues." },
  {
    key: "Everyone else",
    what: 'The call is denied with a `PermissionDenied` error. `"mcp": { "approveTool": true }` offers an `approve` tool instead.',
  },
];

const FIXES = [
  {
    key: "The agent doesn't see modebox",
    what: "Run `modebox -c modebox.json types` in the same folder. If that works, use the full path to `modebox` as the command.",
  },
  { key: "Tool calls time out", what: "Raise your agent's MCP timeout; each tab above shows how." },
  { key: "A call was denied, not asked", what: "Your client can't ask you itself, and `approveTool` is off." },
  {
    key: "sh() has no OS sandbox",
    what: "On Linux, install bubblewrap and allow user namespaces. modebox never runs a command unconfined.",
  },
];

function Label({ n, children }: { n: string; children: string }) {
  return (
    <p className="label">
      <span>{n}</span> {children}
    </p>
  );
}

function Table({ rows, wide }: { rows: { key: string; what: string }[]; wide?: boolean }) {
  return (
    <dl className={wide ? "kv kv-wide" : "kv"}>
      {rows.map((r) => (
        <div key={r.key}>
          <dt>{r.key}</dt>
          <dd>
            <Rich text={r.what} />
          </dd>
        </div>
      ))}
    </dl>
  );
}

// Nothing here reads request data, so the page is prerendered to HTML.
export const dynamic = "force-static";

export default function SetupPage() {
  return (
    <div className="frame">
      <SiteHeader current="setup" />

      <main className="guide">
        <section className="guide-intro corners">
          <p className="eyebrow">setup guide</p>
          <h1>Connect modebox to your agent.</h1>
          <p className="lede">
            No changes to your agent: install the CLI, list your tools, and add modebox to your
            agent's MCP config. Your agent then gets three tools.
          </p>
          <Table rows={GETS} />
        </section>

        <section className="corners" aria-labelledby="install-title">
          <Label n="01">install</Label>
          <h2 id="install-title">Install the CLI</h2>
          <p className="section-copy">It's also the MCP server. Needs Rust 1.96 or later.</p>
          <Cmd cmd="cargo install modebox" />
          <p className="section-copy guide-gap">Or download it from the latest release:</p>
          <CodeBlock {...RELEASE} />
          <p className="section-copy guide-gap">
            On another machine, swap <code>aarch64-apple-darwin</code> for{" "}
            <code>x86_64-apple-darwin</code> (Intel Mac), <code>x86_64-unknown-linux-gnu</code> or{" "}
            <code>aarch64-unknown-linux-gnu</code>.
          </p>
          <p className="callout">
            <code>npm install modebox</code> and <code>pip install modebox</code> install the library
            for your own code. They don't include the <code>modebox</code> command.
          </p>
        </section>

        <section className="corners" aria-labelledby="config-title">
          <Label n="02">config</Label>
          <h2 id="config-title">List your tools in modebox.json</h2>
          <p className="section-copy">
            Put it at your project root. Shell commands can only write inside that folder.
          </p>
          <CodeBlock {...CONFIG} />
          <Table rows={FIELDS} wide />
          <p className="callout">
            Keep tokens out of the file. Your MCP servers inherit the environment your agent starts
            them with, so <code>export GITHUB_PERSONAL_ACCESS_TOKEN=…</code> there.
          </p>
          <p className="section-copy guide-gap">Check it before connecting anything:</p>
          <Cmd cmd="modebox -c modebox.json types" />
        </section>

        <section className="corners" aria-labelledby="connect-title">
          <Label n="03">connect</Label>
          <h2 id="connect-title">Connect your agent</h2>
          <p className="section-copy">Every agent runs the same command: modebox mcp.</p>
          <HarnessTabs items={HARNESSES} />
        </section>

        <section className="corners" aria-labelledby="approvals-title">
          <Label n="04">approvals</Label>
          <h2 id="approvals-title">How approvals reach you</h2>
          <p className="section-copy">modebox picks the best option your client supports.</p>
          <Table rows={APPROVALS} wide />
          <p className="callout callout-warn">
            The model is the one that calls <code>approve</code>. Turn on <code>approveTool</code> only
            if your agent asks you before every call to it, and keep it off in any mode that skips
            permission prompts.
          </p>
        </section>

        <section className="corners" aria-labelledby="fixes-title">
          <Label n="05">help</Label>
          <h2 id="fixes-title">If something's off</h2>
          <Table rows={FIXES} wide />
          <p className="more">
            <a href={GUIDE_SOURCE} className="gh">
              <GitHubIcon size={13} />
              This guide in the repo
            </a>
          </p>
        </section>
      </main>

      <SiteFooter />
      <CopyDelegate />
    </div>
  );
}
