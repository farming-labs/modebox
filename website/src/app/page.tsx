import { CodeTabs, type CodeTab } from "../components/code-tabs";
import { Command } from "../components/command";
import { FigPause } from "../components/fig-pause";
import { FigRuntime } from "../components/fig-runtime";
import { MemoryField } from "../components/memory-field";

const REPO = "https://github.com/farming-labs/modebox";
const GUIDE = `${REPO}/blob/main/docs/integrations.md`;

// The features, laid out like a memory map: address, segment, what it does, what it costs.
const MAP = [
  {
    addr: "0x0000",
    segment: "engine",
    what: "QuickJS-ng compiled to WebAssembly and run by wasmtime, inside your process. Booted once; every run gets a copy-on-write copy.",
    cost: "~0.1 ms",
  },
  {
    addr: "0x4000",
    segment: "boundary",
    what: "The script can only call the tools you hand it. MCP servers show up as typed functions. No files, network or environment.",
    cost: "2 imports",
  },
  {
    addr: "0x8000",
    segment: "pause",
    what: "A call that needs approval seals the run into an encrypted token. Resume it later, on any machine. Nothing runs twice.",
    cost: "~17 KB",
  },
  {
    addr: "0xC000",
    segment: "files",
    what: "One API over local folders, S3, R2, MinIO, GCS and Azure. Writes are staged and land after you approve.",
    cost: "staged",
  },
  {
    addr: "0x10000",
    segment: "shell",
    what: "sh() runs real commands under Seatbelt on macOS, or bubblewrap and seccomp on Linux, reaching only the hosts you list.",
    cost: "per host",
  },
];

const TABS: CodeTab[] = [
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

const CLIENTS = ["Claude Code", "opencode", "Codex", "Cursor", "Node", "Python", "Rust"];

function Logo() {
  return (
    <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
      <rect x="0.5" y="0.5" width="17" height="17" rx="3" fill="none" stroke="currentColor" />
      <rect x="6" y="5" width="2" height="8" fill="currentColor" />
      <rect x="10" y="5" width="2" height="8" fill="currentColor" />
    </svg>
  );
}

function Label({ n, children }: { n: string; children: string }) {
  return (
    <p className="label">
      <span>{n}</span> {children}
    </p>
  );
}

export default function HomePage() {
  return (
    <div className="frame">
      <header className="top">
        <a className="brand" href="/">
          <Logo />
          <span>modebox</span>
        </a>
        <nav>
          <a href={GUIDE}>Setup guide</a>
          <a href={REPO}>GitHub ↗</a>
        </nav>
      </header>

      <main>
        <section className="intro corners">
          <p className="eyebrow">v0.1 · MIT · code mode for AI agents</p>
          <h1>Code mode with a pause button.</h1>
          <p className="lede">
            Your agent writes one script that calls your tools. modebox runs it in a WebAssembly
            sandbox inside your process, about 0.1 ms per run, and can freeze it mid-run until you
            approve.
          </p>
          <div className="install">
            <Command>cargo install modebox</Command>
            <p>
              Or <code>npm install modebox</code> · <code>pip install modebox</code>
            </p>
          </div>
        </section>

        <section className="live corners" aria-labelledby="live-title">
          <Label n="01">live</Label>
          <p className="section-copy" id="live-title">
            This is the sandbox's memory, one square per 4 KiB page. Press pause, or wait for the
            refund to ask for approval.
          </p>
          <MemoryField />
        </section>

        <section className="explain corners" aria-labelledby="pause-title">
          <Label n="02">pause</Label>
          <h2 id="pause-title">How a pause works</h2>
          <p className="section-copy">
            When a call needs your approval, modebox doesn't keep the process waiting, and doesn't
            replay the script later. It saves the run itself: only the memory pages that changed,
            compressed and sealed into a token. Approve it whenever and wherever you like, and the
            script carries on from that exact line.
          </p>
          <FigPause />
        </section>

        <section className="explain corners" aria-labelledby="runtime-title">
          <Label n="03">inside</Label>
          <h2 id="runtime-title">What runs where</h2>
          <p className="section-copy">
            Your script never becomes native code. QuickJS-ng, a small JavaScript engine, is
            compiled to WebAssembly once, booted once with Wizer, and turned into machine code by
            Cranelift. Every run gets a copy-on-write copy of that booted engine. The sandbox has
            exactly two ways out: <code>emit()</code> to call your tools, and a clock.
          </p>
          <FigRuntime />
        </section>

        <section className="map corners" aria-labelledby="map-title">
          <Label n="04">at a glance</Label>
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
          <Label n="05">use it</Label>
          <h2 id="use-title">Pick how you run it</h2>
          <p className="section-copy">
            Plug it into your agent over MCP, or call it from your own code. Each example pauses on a
            refund and resumes after approval.
          </p>
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

      <footer className="bottom">
        <span>end of snapshot · MIT · farming-labs</span>
        <span>
          Built with <a href="https://farmjs.dev">Farm.js</a> · <a href={REPO}>GitHub</a>
        </span>
      </footer>
    </div>
  );
}
