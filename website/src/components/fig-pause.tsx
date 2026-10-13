import type { CSSProperties } from "react";

// "How a pause works": four panels in two rows, animated on one shared
// timeline (see the pz-* keyframes in globals.css):
//   0–30% process A runs to line 4 · 30–50% snapshot · 50–70% seal · 70–96% process B resumes.

const CODE = [
  "const rows = await db.find({ overdue: true });",
  "for (const c of rows) {",
  "  const amt = refundFor(c);",
  "  await payments.refund(c.id, amt);",
  "}",
];
const LINE_Y = (n: number) => 72 + n * 24;

const COLS = 26;
const ROWS = 7;
const CHANGED = [3, 4, 30, 31, 57, 61, 88, 89, 115, 120, 141, 146, 167, 172];

function Frame({ n, title }: { n: number; title: string }) {
  return (
    <>
      <rect className={`pz-frame pz-frame-${n}`} x="0.5" y="0.5" width="459" height="207" />
      <text className="f-mono f-dim" x="16" y="24">
        <tspan className="f-hi">0{n}</tspan> · {title}
      </text>
      <line className="f-rule" x1="0" x2="460" y1="38.5" y2="38.5" />
    </>
  );
}

function Code({ resumed }: { resumed?: boolean }) {
  return (
    <>
      {CODE.map((line, n) => (
        <g key={n}>
          <text className="f-mono f-faint" x="16" y={LINE_Y(n)}>
            {n + 1}
          </text>
          <text
            className={`f-mono ${resumed && n < 3 ? "f-faint" : resumed && n === 3 ? "f-text pz-done-line" : "f-text"}`}
            x="38"
            y={LINE_Y(n)}
            xmlSpace="preserve"
          >
            {line}
          </text>
        </g>
      ))}
    </>
  );
}

export function FigPause() {
  return (
    <div className="pz" role="img" aria-label="How a pause works. 1: process A runs the script until line 4, a refund, needs approval. 2: modebox keeps only the memory pages that changed. 3: it compresses them with zstd and seals them with XChaCha20-Poly1305 into a token of about 17 KB. 4: after approval, process B resumes from line 4; lines 1 to 3 don't run again.">
      <svg viewBox="0 0 460 208" aria-hidden="true">
        <Frame n={1} title="running in process A" />
        <rect className="pz-cursor-a" x="12" y={LINE_Y(0) - 15} width="436" height="21" />
        <Code />
        <rect className="f-dash pz-hold" x="30.5" y={LINE_Y(3) - 15.5} width="280" height="22" />
        <text className="f-mono f-hi pz-hold" x="16" y="192">
          ❚❚ paused · payments.refund needs approval
        </text>
      </svg>

      <svg viewBox="0 0 460 208" aria-hidden="true">
        <Frame n={2} title="snapshot" />
        {Array.from({ length: COLS * ROWS }, (_, n) => {
          const k = CHANGED.indexOf(n);
          return (
            <rect
              key={n}
              className={k >= 0 ? "f-cell pz-page" : "f-cell"}
              style={k >= 0 ? ({ "--i": k } as CSSProperties) : undefined}
              x={18 + (n % COLS) * 16.5}
              y={54 + Math.floor(n / COLS) * 16.5}
              width="11"
              height="11"
            />
          );
        })}
        <text className="f-mono f-dim" x="16" y="192">
          only the 4 KiB pages that changed · 14 of 182
        </text>
      </svg>

      <svg viewBox="0 0 460 208" aria-hidden="true">
        <Frame n={3} title="sealed" />
        <g className="pz-bundle">
          {CHANGED.map((_, k) => (
            <rect key={k} className="f-cell f-on" x={18 + (k % 7) * 15} y={60 + Math.floor(k / 7) * 15} width="10" height="10" />
          ))}
        </g>
        <path className="f-arrow" d="M128 70 H166" markerEnd="url(#pz-head)" />
        <rect className="f-chip pz-chip1" x="174.5" y="56.5" width="74" height="28" />
        <text className="f-mono f-text" x="190" y="75">
          zstd
        </text>
        <path className="f-arrow" d="M252 70 H280" markerEnd="url(#pz-head)" />
        <rect className="f-chip pz-chip2" x="288.5" y="56.5" width="156" height="28" />
        <text className="f-mono f-text" x="302" y="75">
          XChaCha20-Poly1305
        </text>
        <g className="pz-token">
          <text className="f-mono f-hi pz-token-text" x="16" y="136">
            mb1.q8Zt3fX0kR…
          </text>
          <text className="f-mono f-dim" x="16" y="160">
            ~17 KB after 200 tool calls · single-use
          </text>
        </g>
        <defs>
          <marker id="pz-head" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto">
            <path d="M2 1L8 5L2 9" fill="none" stroke="#6e6e6e" strokeWidth="1.4" />
          </marker>
        </defs>
      </svg>

      <svg viewBox="0 0 460 208" aria-hidden="true">
        <Frame n={4} title="resumed in process B" />
        <rect className="pz-cursor-b" x="12" y={LINE_Y(3) - 15} width="436" height="21" />
        <Code resumed />
        <text className="f-mono f-faint" x="444" y={LINE_Y(1)} textAnchor="end">
          not run again
        </text>
        <text className="f-mono f-hi pz-done" x="16" y="192">
          ✓ approved · picked up at line 4
        </text>
      </svg>
    </div>
  );
}
