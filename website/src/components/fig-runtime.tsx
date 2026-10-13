// "What runs where": the agent's code sits in a sealed box with one door.
// Three requests leave the script in turn (rt-* keyframes in globals.css, 9 s):
//   0–36%  db.users()        → through the door → allowed → database → answer back
//   38–56% fetch("evil.com") → hits the wall → blocked
//   58–94% payments.refund() → through the door → stops: asks you

const LINES = [
  { code: "await db.users()", cls: "rt-line-1" },
  { code: 'await fetch("evil.com")', cls: "rt-line-2" },
  { code: "await payments.refund(c, 40)", cls: "rt-line-3" },
];

const TOOLS = [
  { name: "database", y: 105 },
  { name: "payments", y: 185 },
  { name: "GitHub", y: 265 },
];

const BLOCKED = [
  { name: "secrets", x: 96 },
  { name: "files", x: 206 },
  { name: "internet", x: 306 },
];

export function FigRuntime() {
  return (
    <figure className="fig rt">
      <svg
        viewBox="0 0 960 384"
        role="img"
        aria-label="The agent's code runs inside a sealed box in your app. Its only way out is one door, where modebox checks each request. A database read is allowed and the answer comes back. A request to the internet hits the wall and is blocked. A refund stops at the door and asks you first."
      >
        <defs>
          <marker id="rt-head" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto">
            <path d="M2 1L8 5L2 9" fill="none" stroke="#7a7a7a" strokeWidth="1.4" />
          </marker>
        </defs>

        {/* Your app */}
        <rect className="f-outer" x="0.5" y="24.5" width="959" height="358" />
        <text className="f-mono f-dim" x="0" y="14">
          your app · Node, Python, Rust or the CLI
        </text>

        {/* Out of reach: above the box */}
        {BLOCKED.map((b) => (
          <g key={b.name}>
            <rect className="rt-out" x={b.x + 0.5} y="36.5" width="92" height="22" />
            <text className="f-sans f-dim" x={b.x + 46} y="52" textAnchor="middle">
              {b.name}
            </text>
          </g>
        ))}
        <text className="f-mono f-faint" x="410" y="52">
          out of reach
        </text>

        {/* The sealed box, with a door in its right wall (y 180–232) */}
        <rect className="rt-box-fill" x="40" y="74" width="480" height="290" />
        <path className="rt-wall" d="M520 180 V74 H40 V364 H520 V232" />
        <path className="rt-lock" d="M62 92 h14 v11 h-14 z M65 92 v-4 a4 4 0 0 1 8 0 v4" />
        <text className="pz-title" x="86" y="103">
          Sealed sandbox
        </text>
        <text className="f-mono f-dim" x="500" y="103" textAnchor="end">
          WebAssembly
        </text>

        {/* The agent's script */}
        <rect className="pz-card" x="62.5" y="120.5" width="290" height="102" />
        <text className="f-sans f-muted" x="76" y="142">
          the agent's script
        </text>
        {LINES.map((l, i) => (
          <text key={i} className={`f-mono f-text ${l.cls}`} x="76" y={166 + i * 20}>
            {l.code}
          </text>
        ))}

        {/* The engine */}
        <rect className="f-box" x="62.5" y="240.5" width="436" height="40" />
        <text className="f-sans f-text" x="76" y="265">
          QuickJS engine, booted once · each run gets a fresh copy
        </text>

        {/* Its own memory */}
        <text className="f-mono f-dim" x="62" y="304">
          its own memory
        </text>
        {Array.from({ length: 72 }, (_, n) => (
          <rect
            key={n}
            className={[3, 4, 21, 22, 30, 41, 47, 58, 66].includes(n) ? "f-cell f-on" : "f-cell"}
            x={62 + (n % 24) * 18}
            y={314 + Math.floor(n / 24) * 14}
            width="12"
            height="9"
          />
        ))}

        {/* The door and the check */}
        <text className="f-mono f-hi" x="530" y="172">
          the only door
        </text>
        <path className="rt-wire" d="M520 206 H578" markerEnd="url(#rt-head)" />
        <rect className="rt-gate" x="586.5" y="178.5" width="150" height="56" />
        <text className="f-sans f-title" x="600" y="202">
          modebox checks
        </text>
        <text className="f-mono f-dim" x="600" y="222">
          allow · ask · deny
        </text>

        {/* Your tools */}
        <text className="f-mono f-dim" x="800" y="90">
          your tools
        </text>
        {TOOLS.map((t) => (
          <g key={t.name}>
            <rect className="f-box rt-tool" x="800.5" y={t.y + 0.5} width="140" height="40" />
            <text className="f-sans f-text" x="816" y={t.y + 25}>
              {t.name}
            </text>
          </g>
        ))}
        <path className="rt-wire" d="M737 206 H766 V125 H796" markerEnd="url(#rt-head)" />
        <path className="rt-wire" d="M737 206 H796" markerEnd="url(#rt-head)" />
        <path className="rt-wire" d="M737 206 H766 V285 H796" markerEnd="url(#rt-head)" />

        {/* Request 1: allowed, answered */}
        <circle className="rt-dot rt-dot-1" cx="352" cy="161" r="5" />
        <circle className="rt-dot rt-dot-1b" cx="800" cy="125" r="4" />
        <text className="f-mono f-hi rt-tag rt-tag-1" x="742" y="170">
          allowed
        </text>

        {/* Request 2: blocked at the wall */}
        <circle className="rt-dot rt-dot-2" cx="330" cy="120" r="5" />
        <g className="rt-tag rt-tag-2">
          <path className="rt-x" d="M324 68 l12 12 M336 68 l-12 12" />
          <text className="f-mono f-hi" x="344" y="71">
            blocked
          </text>
        </g>

        {/* Request 3: waits for you */}
        <circle className="rt-dot rt-dot-3" cx="352" cy="201" r="5" />
        <g className="rt-tag rt-tag-3">
          <rect className="pz-badge" x="596" y="244" width="130" height="24" rx="3" />
          <text className="pz-badge-text" x="661" y="260.5" textAnchor="middle">
            ❚❚ asks you first
          </text>
        </g>
      </svg>
      <figcaption className="fig-cap">
        The agent's code runs in a sealed box. Its only way out is one door, where modebox lets a
        request through, asks you, or says no. Everything else is out of reach.
      </figcaption>
    </figure>
  );
}
