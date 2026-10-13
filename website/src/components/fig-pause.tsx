import type { CSSProperties, JSX } from "react";

// "How a pause works", told as four steps a non-specialist can follow:
// the script stops before a refund, modebox saves what changed, locks it
// into a token, and a person approves hours later on another computer.
// One shared 10 s timeline drives every part (pz-* keyframes in globals.css):
//   0–28% runs · 30–46% saves · 48–62% locks · 62–96% approved and continues.

const PW = 460;
const PH = 210;

const CODE = [
  "const rows = await db.find({ overdue: true });",
  "for (const c of rows) {",
  "  const amt = refundFor(c);",
  "  await payments.refund(c.id, amt);",
  "}",
];

const GRID_COLS = 26;
const GRID_ROWS = 6;
const CHANGED = [3, 4, 30, 31, 57, 61, 88, 89, 115, 120, 141, 146, 133, 108];

function Frame({ n, title, aside }: { n: number; title: string; aside: string }) {
  return (
    <>
      <rect className={`pz-frame pz-frame-${n}`} x="0.5" y="0.5" width={PW - 1} height={PH - 1} />
      <text className="pz-n" x="16" y="25">
        {n}
      </text>
      <text className="pz-title" x="34" y="25">
        {title}
      </text>
      <text className="f-mono f-dim" x={PW - 16} y="25" textAnchor="end">
        {aside}
      </text>
      <line className="f-rule" x1="0" x2={PW} y1="38.5" y2="38.5" />
    </>
  );
}

function Runs() {
  const y = (n: number) => 64 + n * 22;
  return (
    <>
      <Frame n={1} title="The agent's script runs" aside="computer A" />
      <rect className="pz-cursor-a" x="12" y={y(0) - 15} width={PW - 24} height="21" />
      {CODE.map((line, n) => (
        <g key={n}>
          <text className="f-mono f-faint" x="16" y={y(n)}>
            {n + 1}
          </text>
          <text className="f-mono f-text" x="38" y={y(n)} xmlSpace="preserve">
            {line}
          </text>
        </g>
      ))}
      <g className="pz-hold">
        <rect className="f-dash" x="34.5" y={y(3) - 15.5} width="268" height="22" />
        <rect className="pz-badge" x="314" y={y(3) - 16} width="130" height="24" rx="3" />
        <text className="pz-badge-text" x="379" y={y(3) + 0.5} textAnchor="middle">
          ❚❚ needs your OK
        </text>
        <text className="f-sans f-muted" x="16" y="192">
          It stops right before the refund, and waits.
        </text>
      </g>
    </>
  );
}

function Saves() {
  return (
    <>
      <Frame n={2} title="It saves only what changed" aside="memory pages" />
      {Array.from({ length: GRID_COLS * GRID_ROWS }, (_, n) => {
        const k = CHANGED.indexOf(n);
        return (
          <rect
            key={n}
            className={k >= 0 ? "f-cell pz-page" : "f-cell"}
            style={k >= 0 ? ({ "--i": k } as CSSProperties) : undefined}
            x={18 + (n % GRID_COLS) * 16.5}
            y={56 + Math.floor(n / GRID_COLS) * 16.5}
            width="11"
            height="11"
          />
        );
      })}
      <text className="f-sans f-muted pz-saved" x="16" y="182">
        14 of 156 pages changed. Only those are kept.
      </text>
    </>
  );
}

function Locks({ head }: { head: string }) {
  return (
    <>
      <Frame n={3} title="It locks them into a token" aside="encrypted" />
      <g className="pz-bundle">
        {CHANGED.map((_, k) => (
          <rect key={k} className="f-cell f-on" x={18 + (k % 7) * 14} y={66 + Math.floor(k / 7) * 14} width="10" height="10" />
        ))}
      </g>
      <path className="f-arrow" d="M122 78 H158" markerEnd={`url(#${head})`} />
      <rect className="pz-card" x="166.5" y="52.5" width="278" height="56" />
      <g className="pz-lock">
        <rect className="pz-lock-body" x="180" y="77" width="16" height="12" rx="2" />
        <path className="pz-shackle" d="M183.5 77 V72.5 a4.5 4.5 0 0 1 9 0 V77" />
      </g>
      <g className="pz-token">
        <text className="f-mono f-hi pz-token-text" x="208" y="78">
          mb1.q8Zt3fX0kR…
        </text>
        <text className="f-mono f-dim" x="208" y="97">
          ~17 KB · works once
        </text>
      </g>
      {[
        { x: 16.5, w: 98, t: "compressed" },
        { x: 122.5, w: 90, t: "encrypted" },
        { x: 220.5, w: 94, t: "single-use" },
      ].map((c) => (
        <g key={c.t}>
          <rect className="f-chip pz-chip" x={c.x} y="128.5" width={c.w} height="26" />
          <text className="f-sans f-text" x={c.x + c.w / 2} y="146" textAnchor="middle">
            {c.t}
          </text>
        </g>
      ))}
      <text className="f-mono f-faint" x="16" y="186">
        zstd · XChaCha20-Poly1305
      </text>
    </>
  );
}

function Continues() {
  return (
    <>
      <Frame n={4} title="You approve. It carries on." aside="computer B · later" />
      <g className="pz-ask">
        <rect className="pz-card" x="16.5" y="50.5" width="428" height="44" />
        <text className="f-sans pz-ask-text" x="30" y="77">
          Refund $40.00 to customer c_1042?
        </text>
        <circle className="pz-ripple" cx="395" cy="72.5" r="14" />
        <rect className="pz-approve" x="356.5" y="60.5" width="78" height="24" rx="3" />
        <text className="pz-approve-text" x="395.5" y="76.5" textAnchor="middle">
          Approve
        </text>
      </g>
      <rect className="pz-cursor-b" x="12" y="133" width={PW - 24} height="21" />
      <text className="f-mono f-faint" x="16" y="124">
        1–3
      </text>
      <text className="f-mono f-faint pz-struck" x="48" y="124">
        already done · not run again
      </text>
      <line className="pz-strike" x1="48" x2="250" y1="120" y2="120" />
      <text className="f-mono f-faint" x="16" y="148">
        4
      </text>
      <text className="f-mono pz-resume-line" x="48" y="148" xmlSpace="preserve">
        {CODE[3]}
      </text>
      <text className="f-mono f-faint" x="16" y="170">
        5
      </text>
      <text className="f-mono f-text" x="48" y="170">
        {"}"}
      </text>
      <text className="f-sans f-hi pz-done" x="16" y="196">
        ✓ Approved. It picked up at line 4; nothing ran twice.
      </text>
    </>
  );
}

const PANELS: ((p: { head: string }) => JSX.Element)[] = [Runs, Saves, Locks, Continues];

function Head({ id }: { id: string }) {
  return (
    <defs>
      <marker id={id} viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto">
        <path d="M2 1L8 5L2 9" fill="none" stroke="#9a9a9a" strokeWidth="1.4" />
      </marker>
    </defs>
  );
}

function Link({ n, d, label, lx, ly, head, anchor = "middle" }: { n: number; d: string; label: string; lx: number; ly: number; head: string; anchor?: "middle" | "start" }) {
  return (
    <g className={`pz-link pz-link-${n}`}>
      <path className="pz-link-path" d={d} markerEnd={`url(#${head})`} />
      <text className="f-mono pz-link-label" x={lx} y={ly} textAnchor={anchor}>
        {label}
      </text>
    </g>
  );
}

const LABEL =
  "How a pause works. 1: the agent's script runs on computer A and stops right before a refund that needs your OK. 2: modebox keeps only the memory pages that changed, 14 of 156. 3: it compresses and encrypts them into a token of about 17 KB that works once. 4: hours later, on computer B, you approve the refund and the script carries on from line 4; lines 1 to 3 don't run again.";

export function FigPause() {
  return (
    <figure className="fig pz">
      {/* Two rows on wide screens. */}
      <svg className="pz-wide" viewBox="0 0 960 470" role="img" aria-label={LABEL}>
        <Head id="pz-head-w" />
        {PANELS.map((Panel, i) => (
          <g key={i} transform={`translate(${(i % 2) * 500} ${Math.floor(i / 2) * 260})`}>
            <Panel head="pz-head-w" />
          </g>
        ))}
        <Link n={1} d="M462 105 H494" label="pause" lx={478} ly={96} head="pz-head-w" />
        <Link n={2} d="M730 212 V236 H230 V254" label="seal" lx={480} ly={230} head="pz-head-w" />
        <Link n={3} d="M462 365 H494" label="later" lx={478} ly={356} head="pz-head-w" />
      </svg>

      {/* One column on narrow screens. */}
      <svg className="pz-narrow" viewBox="0 0 460 966" role="img" aria-label={LABEL}>
        <Head id="pz-head-n" />
        {PANELS.map((Panel, i) => (
          <g key={i} transform={`translate(0 ${i * 252})`}>
            <Panel head="pz-head-n" />
          </g>
        ))}
        <Link n={1} d="M230 212 V248" label="pause" lx={242} ly={234} anchor="start" head="pz-head-n" />
        <Link n={2} d="M230 464 V500" label="seal" lx={242} ly={486} anchor="start" head="pz-head-n" />
        <Link n={3} d="M230 716 V752" label="later" lx={242} ly={738} anchor="start" head="pz-head-n" />
      </svg>

      <figcaption className="fig-cap">
        modebox freezes the run where it stopped, keeps only what changed, and picks it back up
        after you say yes. Nothing runs twice.
      </figcaption>
    </figure>
  );
}
