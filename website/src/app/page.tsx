import { CodeTabs, type CodeTab } from "../components/code-tabs";
import { FigPause } from "../components/fig-pause";
import { FigRuntime } from "../components/fig-runtime";
import { InstallSwitch } from "../components/install-switch";
import { MemoryField } from "../components/memory-field";
import { SiteFooter, SiteHeader } from "../components/site-chrome";
import { highlight, type Lang } from "../lib/highlight";

const GUIDE = "/setup";

// The features, laid out like a memory map: address, segment, what it does, what it costs.
const MAP = [
  { addr: "0x0000", segment: "engine", what: "QuickJS-ng in WebAssembly. Booted once, copied per run.", cost: "~0.1 ms" },
  { addr: "0x4000", segment: "boundary", what: "Only the tools you hand it. MCP servers become typed functions.", cost: "2 imports" },
  { addr: "0x8000", segment: "pause", what: "Seals the run into a token. Resume anywhere; nothing re-runs.", cost: "~17 KB" },
  { addr: "0xC000", segment: "files", what: "One API over local, S3, R2, GCS and Azure. Writes wait for you.", cost: "staged" },
  { addr: "0x10000", segment: "shell", what: "sh() under Seatbelt or bubblewrap + seccomp, listed hosts only.", cost: "per host" },
];

const SOURCES: (Omit<CodeTab, "html"> & { lang: Lang })[] = [
  {
    id: "mcp",
    label: "MCP",
    install: "cargo install modebox",
    file: "modebox.json",
    lang: "json",
    code: `{
  "mcpServers": {
    "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] }
  },
  "approve": ["github.create_*", "github.merge_*"]
}`,
    then: "claude mcp add --scope project modebox -- modebox mcp -c modebox.json",
  },
  {
    id: "node",
    label: "Node",
    install: "npm install modebox",
    file: "agent.ts",
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
  {
    id: "python",
    label: "Python",
    install: "pip install modebox",
    file: "agent.py",
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
  {
    id: "rust",
    label: "Rust",
    install: "cargo add modebox-core",
    file: "main.rs",
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
];

const TABS: CodeTab[] = SOURCES.map(({ lang, ...tab }) => ({ ...tab, html: highlight(tab.code, lang) }));

const CLIENTS = ["Claude Code", "opencode", "Codex", "Cursor", "Node", "Python", "Rust"];

function Label({ n, children }: { n: string; children: string }) {
  return (
    <p className="label">
      <span>{n}</span> {children}
    </p>
  );
}

// Nothing here reads request data, so the page is prerendered to HTML.
export const dynamic = "force-static";

export default function HomePage() {
  return (
    <div className="frame">
      <SiteHeader />

      <main>
        <section className="hero corners">
          <div className="hero-copy">
            <p className="eyebrow">v0.1 · MIT · open source</p>
            <h1>Code mode, sandboxed.</h1>
            <p className="lede">
              Your agent writes one script that calls your tools. modebox runs it in about 0.1 ms,
              and pauses it mid-run when a call needs you.
            </p>
            <InstallSwitch />
          </div>
          <MemoryField />
        </section>

        <section className="explain corners" aria-labelledby="pause-title">
          <Label n="01">pause</Label>
          <h2 id="pause-title" className="fig-h2">How a pause works</h2>
          <FigPause />
        </section>

        <section className="explain corners" aria-labelledby="runtime-title">
          <Label n="02">inside</Label>
          <h2 id="runtime-title" className="fig-h2">What runs where</h2>
          <FigRuntime />
        </section>

        <section className="map corners" aria-labelledby="map-title">
          <Label n="03">at a glance</Label>
          <h2 id="map-title">Memory map</h2>
          <div className="map-table" role="table" aria-label="What modebox is made of">
            <div className="map-row map-head" role="row">
              <span role="columnheader">address</span>
              <span role="columnheader">segment</span>
              <span role="columnheader">what it does</span>
              <span role="columnheader">cost</span>
            </div>
            {MAP.map((row) => (
              <div className="map-row" role="row" key={row.segment}>
                <span className="addr" role="cell">
                  {row.addr}
                </span>
                <span className="seg" role="cell">
                  {row.segment}
                </span>
                <span className="what" role="cell">
                  {row.what}
                </span>
                <span className="cost" role="cell">
                  {row.cost}
                </span>
              </div>
            ))}
          </div>
        </section>

        <section className="use corners" aria-labelledby="use-title">
          <Label n="04">use it</Label>
          <h2 id="use-title">Pick how you run it</h2>
          <p className="section-copy">Over MCP, or from your own code.</p>
          <CodeTabs tabs={TABS} />
          <p className="more">
            opencode, Codex, Cursor and other agents: <a href={GUIDE}>the setup guide</a>.
          </p>
          <ul className="clients" aria-label="Works with">
            {CLIENTS.map((c) => (
              <li key={c}>{c}</li>
            ))}
          </ul>
        </section>
      </main>

      <SiteFooter />
    </div>
  );
}
