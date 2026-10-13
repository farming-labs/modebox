import { CodeTabs, type CodeTab } from "../components/code-tabs";
import { FigPause } from "../components/fig-pause";
import { FigRuntime } from "../components/fig-runtime";
import { InstallSwitch } from "../components/install-switch";
import { MemoryField } from "../components/memory-field";
import { REPO, SiteFooter, SiteHeader } from "../components/site-chrome";
import HIGHLIGHTED from "../content/highlighted.json";
import { SAMPLES, type SampleId } from "../content/samples";

const GUIDE = "/setup";

// The features, laid out like a memory map: address, segment, what it does, what it costs.
const MAP = [
  { addr: "0x0000", segment: "engine", what: "QuickJS-ng in WebAssembly. Booted once, copied per run.", cost: "~0.1 ms" },
  { addr: "0x4000", segment: "boundary", what: "Only the tools you hand it. MCP servers become typed functions.", cost: "2 imports" },
  { addr: "0x8000", segment: "pause", what: "Seals the run into a token, ~17 KB in our benchmark. Resume anywhere; nothing re-runs.", cost: "single-use" },
  { addr: "0xC000", segment: "files", what: "One API over local, S3, R2, GCS and Azure. Writes wait for you.", cost: "staged" },
  { addr: "0x10000", segment: "shell", what: "sh() under Seatbelt or bubblewrap + seccomp. Network off, or listed hosts only.", cost: "per host" },
];

const SOURCES: Omit<CodeTab, "html">[] = [
  {
    id: "mcp",
    label: "MCP",
    install: "cargo install modebox",
    file: "modebox.json",
    code: SAMPLES.mcp.code,
    then: "claude mcp add --scope project modebox -- modebox mcp -c modebox.json",
  },
  {
    id: "node",
    label: "Node",
    install: "npm install modebox",
    file: "agent.ts",
    code: SAMPLES.node.code,
  },
  {
    id: "python",
    label: "Python",
    install: "pip install modebox",
    file: "agent.py",
    code: SAMPLES.python.code,
  },
  {
    id: "rust",
    label: "Rust",
    install: "cargo add modebox-core",
    file: "main.rs",
    code: SAMPLES.rust.code,
  },
];

const TABS: CodeTab[] = SOURCES.map((tab) => ({ ...tab, html: HIGHLIGHTED[tab.id as SampleId] }));

const CLIENTS = ["Claude Code", "opencode", "Codex", "Cursor", "T3 Code", "Node", "Python", "Rust"];

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
            <p className="eyebrow">v0.1 prototype · MIT · open source</p>
            <h1>Code mode, sandboxed.</h1>
            <p className="lede">
              Code mode means your agent writes one script that calls your tools. modebox runs that
              script in a WebAssembly sandbox and pauses it mid-run when a call needs you. A fresh
              sandbox starts in{" "}
              <a className="inline" href={`${REPO}#benchmarks`}>
                about 0.1 ms
              </a>
              .
            </p>
            <InstallSwitch />
            <p className="platforms">macOS and Linux · Windows isn't supported yet</p>
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
