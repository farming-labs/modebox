// "What runs where": your process holds swc, wasmtime and your tools; the
// WebAssembly sandbox holds QuickJS-ng, your script and its memory, and
// reaches out only through emit() and the clock.

const COLS = 16;
const ROWS = 10;
const ON = new Set([5, 6, 22, 37, 38, 54, 70, 71, 86, 101, 117, 118, 133, 150]);

function Box({
  x,
  y,
  w,
  h,
  title,
  sub,
  strong,
}: {
  x: number;
  y: number;
  w: number;
  h: number;
  title: string;
  sub: string;
  strong?: boolean;
}) {
  return (
    <g>
      <rect className={strong ? "f-box f-box-hi" : "f-box"} x={x + 0.5} y={y + 0.5} width={w - 1} height={h - 1} />
      <text className="f-sans f-title" x={x + 14} y={y + 24}>
        {title}
      </text>
      <text className="f-sans f-muted" x={x + 14} y={y + 43}>
        {sub}
      </text>
    </g>
  );
}

export function FigRuntime() {
  return (
    <figure className="fig">
      <svg viewBox="0 0 990 336" role="img" aria-labelledby="fig-runtime-title fig-runtime-desc">
        <title id="fig-runtime-title">What runs where</title>
        <desc id="fig-runtime-desc">
          Your process runs swc, which strips TypeScript, and wasmtime, which runs the sandbox as
          machine code compiled by Cranelift. Inside the WebAssembly sandbox, QuickJS-ng, booted
          once and copied per run, compiles your script to bytecode and interprets it, using the
          sandbox's own memory. The sandbox reaches your tools only through two imports: emit()
          and a clock.
        </desc>
        <defs>
          <marker id="f-rt-head" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M2 1L8 5L2 9" fill="none" stroke="#6e6e6e" strokeWidth="1.4" />
          </marker>
        </defs>

        {/* Your process */}
        <rect className="f-outer" x={0.5} y={24.5} width={989} height={310} />
        <text className="f-mono f-dim" x={0} y={14}>
          your process · CLI, Node, Python or Rust
        </text>

        <Box x={24} y={52} w={280} h={64} title="swc" sub="Strips TypeScript, natively." />
        <Box x={24} y={132} w={280} h={64} title="wasmtime + Cranelift" sub="Runs the sandbox as machine code." />
        <Box x={24} y={246} w={280} h={64} title="Your tools" sub="MCP servers, functions, sh(), fs." strong />

        {/* The sandbox */}
        <rect className="f-sandbox" x={344.5} y={52.5} width={621} height={258} />
        <text className="f-mono f-text" x={360} y={74}>
          WebAssembly sandbox · no files, network or env
        </text>

        <Box x={364} y={92} w={300} h={64} title="QuickJS-ng" sub="Booted once; every run gets a copy." />
        <Box x={364} y={226} w={300} h={64} title="Your script" sub="Compiled to bytecode, interpreted." strong />
        <path className="f-arrow" d="M514 160 V222" markerEnd="url(#f-rt-head)" />
        <text className="f-mono f-dim" x={524} y={195}>
          runs
        </text>

        {/* Linear memory */}
        <text className="f-mono f-dim" x={694} y={104}>
          linear memory · 4 KiB pages
        </text>
        {Array.from({ length: COLS * ROWS }, (_, n) => (
          <rect
            key={n}
            className={ON.has(n) ? "f-cell f-on" : "f-cell"}
            x={694 + (n % COLS) * 15.5}
            y={118 + Math.floor(n / COLS) * 15.5}
            width={10.5}
            height={10.5}
          />
        ))}
        <text className="f-mono f-dim" x={694} y={292}>
          white = changed since boot
        </text>

        {/* The only two ways out */}
        <path className="f-arrow f-arrow-hi" d="M362 248 H308" markerEnd="url(#f-rt-head)" />
        <path className="f-arrow" d="M308 274 H362" markerEnd="url(#f-rt-head)" />
        <text className="f-mono f-hi" x={306} y={242}>
          emit()
        </text>
        <text className="f-mono f-dim" x={306} y={290}>
          clock
        </text>
      </svg>
    </figure>
  );
}
