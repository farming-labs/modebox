import { Command } from "../components/command";
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

const STEPS = [
  { cmd: "cargo install modebox", note: "The CLI, which is also the MCP server." },
  { cmd: "modebox -c modebox.json types", note: "See the typed API your agent will get." },
  {
    cmd: "claude mcp add --scope project modebox -- modebox mcp -c modebox.json",
    note: "Connect Claude Code.",
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
          <h1>
            Code mode with a pause button.
          </h1>
          <div className="intro-row">
            <p className="lede">
              Your agent writes one script that calls your tools. modebox runs it in a WebAssembly
              sandbox inside your process, and can freeze it mid-run until you approve.
            </p>
            <div className="install">
              <Command>cargo install modebox</Command>
              <p>
                Or <code>npm install modebox</code> · <code>pip install modebox</code>
              </p>
            </div>
          </div>
        </section>

        <section className="live corners" aria-labelledby="live-title">
          <p className="eyebrow" id="live-title">
            Live · this is the sandbox's memory. Press pause.
          </p>
          <MemoryField />
        </section>

        <section className="map corners" aria-labelledby="map-title">
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

        <section className="start corners" aria-labelledby="start-title">
          <h2 id="start-title">Run it</h2>
          <ol className="steps">
            {STEPS.map((step, i) => (
              <li key={step.cmd}>
                <span className="step-n">{String(i + 1).padStart(2, "0")}</span>
                <div className="step-body">
                  <Command>{step.cmd}</Command>
                  <p>{step.note}</p>
                </div>
              </li>
            ))}
          </ol>
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
