// "How a pause works": four panels, left to right, in one SVG.

const W = 210; // panel width
const GAP = 40;
const TOP = 30;
const H = 168; // panel height
const x = (i: number) => i * (W + GAP);

const CODE = [
  "const rows = await db.find();",
  "for (const c of rows) {",
  "  const amt = refundFor(c);",
  "  await pay.refund(c, amt);",
  "}",
];

const COLS = 14;
const ROWS = 7;
const ON = new Set([2, 3, 16, 25, 26, 39, 47, 48, 61, 70, 84, 92]);

function Panel({ i, title }: { i: number; title: string }) {
  return (
    <g>
      <rect className="f-box" x={x(i) + 0.5} y={TOP + 0.5} width={W - 1} height={H - 1} />
      <text className="f-mono f-dim" x={x(i) + 12} y={TOP + 19}>
        {title}
      </text>
      <line className="f-rule" x1={x(i)} x2={x(i) + W} y1={TOP + 30.5} y2={TOP + 30.5} />
    </g>
  );
}

function Code({ i, resumed }: { i: number; resumed?: boolean }) {
  return (
    <g>
      {CODE.map((line, n) => {
        const y = TOP + 56 + n * 20;
        const done = resumed && n < 3;
        const current = n === 3;
        return (
          <g key={n}>
            <text className="f-mono f-faint" x={x(i) + 12} y={y}>
              {n + 1}
            </text>
            <text
              className={`f-mono ${done ? "f-faint" : current ? "f-hi" : "f-text"}`}
              x={x(i) + 28}
              y={y}
              xmlSpace="preserve"
            >
              {line}
            </text>
            {current ? (
              <rect
                className="f-dash"
                x={x(i) + 24.5}
                y={y - 12.5}
                width={W - 36}
                height={18}
              />
            ) : null}
          </g>
        );
      })}
    </g>
  );
}

function Caption({ i, lines }: { i: number; lines: string[] }) {
  return (
    <text className="f-sans f-muted" x={x(i)} y={TOP + H + 24}>
      {lines.map((l, n) => (
        <tspan key={n} x={x(i)} dy={n === 0 ? 0 : 16}>
          {l}
        </tspan>
      ))}
    </text>
  );
}

function Arrow({ i }: { i: number }) {
  const y = TOP + H / 2;
  return (
    <path
      className="f-arrow"
      d={`M${x(i) + W + 8} ${y} H${x(i + 1) - 10}`}
      markerEnd="url(#f-pause-head)"
    />
  );
}

export function FigPause() {
  return (
    <figure className="fig">
      <svg viewBox="0 0 990 250" role="img" aria-labelledby="fig-pause-title fig-pause-desc">
        <title id="fig-pause-title">How a pause works</title>
        <desc id="fig-pause-desc">
          Process A runs the script until line 4 needs approval. modebox keeps only the memory
          pages that changed, compresses them with zstd and seals them with XChaCha20-Poly1305
          into a token of about 17 KB. Process B resumes from line 4 after approval; lines 1 to 3
          don't run again.
        </desc>
        <defs>
          <marker id="f-pause-head" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto">
            <path d="M2 1L8 5L2 9" fill="none" stroke="#6e6e6e" strokeWidth="1.4" />
          </marker>
        </defs>

        <Panel i={0} title="process A · running" />
        <Code i={0} />
        <text className="f-mono f-hi" x={x(0) + 12} y={TOP + H - 14}>
          ❚❚ needs approval
        </text>

        <Panel i={1} title="snapshot" />
        {Array.from({ length: COLS * ROWS }, (_, n) => (
          <rect
            key={n}
            className={ON.has(n) ? "f-cell f-on" : "f-cell"}
            x={x(1) + 16 + (n % COLS) * 13}
            y={TOP + 46 + Math.floor(n / COLS) * 13}
            width={9}
            height={9}
          />
        ))}
        <text className="f-mono f-dim" x={x(1) + 16} y={TOP + H - 14}>
          12 of 98 pages changed
        </text>

        <Panel i={2} title="sealed token" />
        <rect className="f-chip" x={x(2) + 12.5} y={TOP + 44.5} width={W - 25} height={24} />
        <text className="f-mono f-text" x={x(2) + 22} y={TOP + 60}>
          zstd
        </text>
        <rect className="f-chip" x={x(2) + 12.5} y={TOP + 76.5} width={W - 25} height={24} />
        <text className="f-mono f-text" x={x(2) + 22} y={TOP + 92}>
          XChaCha20-Poly1305
        </text>
        <text className="f-mono f-hi" x={x(2) + 12} y={TOP + 128}>
          mb1.q8Zt3fX0kR…
        </text>
        <text className="f-mono f-dim" x={x(2) + 12} y={TOP + H - 14}>
          ~17 KB after 200 calls
        </text>

        <Panel i={3} title="process B · resumed" />
        <Code i={3} resumed />
        <text className="f-mono f-hi" x={x(3) + 12} y={TOP + H - 14}>
          ✓ approved · from line 4
        </text>

        <Arrow i={0} />
        <Arrow i={1} />
        <Arrow i={2} />

        <Caption i={0} lines={["The script reaches a call", "that needs your approval."]} />
        <Caption i={1} lines={["Only the 4 KiB memory pages", "that changed are kept."]} />
        <Caption i={2} lines={["Compressed, encrypted,", "single-use."]} />
        <Caption i={3} lines={["Approve any time, anywhere.", "Lines 1–3 don't run again."]} />
      </svg>
    </figure>
  );
}
