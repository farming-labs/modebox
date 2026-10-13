// "What runs where": the agent's code sits in a sealed box with one door.
// The wires themselves animate: a route's dashes brighten and march in the
// direction of travel (rt-* keyframes in globals.css, 10 s):
//   0–41%  db.users()        → door → allowed → database, and the answer runs back
//   44–58% fetch("evil.com") → hits the wall → blocked
//   60–94% payments.refund() → door → stops at the check: asks you
// Two drawings share those classes: a wide one, and a tall one for phones
// with the door in the floor of the box and the tools in a row below.

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

const ON = [3, 4, 21, 22, 30, 41, 47, 58, 66];

const LABEL =
  "The agent's code runs inside a sealed box in your app. Its only way out is one door, where modebox checks each request. A database read is allowed and the answer comes back. A request to the internet hits the wall and is blocked. A refund stops at the door and asks you first.";

function Head({ id }: { id: string }) {
  return (
    <defs>
      <marker id={id} viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" markerUnits="userSpaceOnUse" orient="auto">
        <path d="M2 1L8 5L2 9" fill="none" stroke="#7a7a7a" strokeWidth="1.4" />
      </marker>
    </defs>
  );
}

export function FigRuntime() {
  return (
    <figure className="fig rt">
      <svg className="rt-wide" viewBox="0 0 960 384" role="img" aria-label={LABEL}>
        <Head id="rt-head" />

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
        <text className="f-mono f-dim" x="410" y="52">
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

        {/* Routes out of the script: through the door to the check, and up into the wall */}
        <path className="rt-wire rt-w1" d="M352 181 H430 V206 H584" />
        {/* Two heads per animated wire: forward, and back for the answer */}
        <path className="rt-head rt-head-1f" d="M577 201.5 L584 206 L577 210.5" />
        <path className="rt-head rt-head-1b" d="M359 176.5 L352 181 L359 185.5" />
        <path className="rt-wire rt-w5" d="M330 120 V78" />

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
            className={ON.includes(n) ? "f-cell f-on" : "f-cell"}
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
            <rect className={t.name === "database" ? "f-box rt-tool rt-db" : "f-box rt-tool"} x="800.5" y={t.y + 0.5} width="140" height="40" />
            <text className="f-sans f-text" x="816" y={t.y + 25}>
              {t.name}
            </text>
          </g>
        ))}
        <path className="rt-wire rt-w2" d="M737 206 H766 V125 H798" />
        <path className="rt-head rt-head-2f" d="M791 120.5 L798 125 L791 129.5" />
        <path className="rt-head rt-head-2b" d="M744 201.5 L737 206 L744 210.5" />
        <path className="rt-wire" d="M737 206 H796" markerEnd="url(#rt-head)" />
        <path className="rt-wire" d="M737 206 H766 V285 H796" markerEnd="url(#rt-head)" />

        {/* Request 3's badge sits under the check */}
        <g className="rt-tag rt-tag-3">
          <rect className="pz-badge" x="596" y="244" width="130" height="24" rx="3" />
          <text className="pz-badge-text" x="661" y="260.5" textAnchor="middle">
            ❚❚ asks you first
          </text>
        </g>

        {/* Request 1: allowed */}
        <text className="f-mono f-hi rt-tag rt-tag-1" x="742" y="170">
          allowed
        </text>

        {/* Request 2: blocked at the wall */}
        <g className="rt-tag rt-tag-2">
          <path className="rt-x" d="M324 68 l12 12 M336 68 l-12 12" />
          <text className="f-mono f-hi" x="344" y="71">
            blocked
          </text>
        </g>
      </svg>
      <Narrow />
      <figcaption className="fig-cap">
        The agent's code runs in a sealed box. Its only way out is one door, where modebox lets a
        request through, asks you, or says no. Everything else is out of reach.
      </figcaption>
    </figure>
  );
}

function Narrow() {
  return (
    <svg className="rt-narrow" viewBox="0 0 460 616" role="img" aria-label={LABEL}>
      <Head id="rt-head-n" />

      {/* Your app */}
      <rect className="f-outer" x="0.5" y="24.5" width="459" height="590" />
      <text className="f-mono f-dim" x="0" y="14">
        your app · Node, Python, Rust or the CLI
      </text>

      {/* Out of reach: above the box */}
      {BLOCKED.map((b, i) => (
        <g key={b.name}>
          <rect className="rt-out" x={24.5 + i * 108} y="36.5" width="92" height="22" />
          <text className="f-sans f-dim" x={70 + i * 108} y="52" textAnchor="middle">
            {b.name}
          </text>
        </g>
      ))}
      <text className="f-mono f-dim" x="344" y="52">
        out of reach
      </text>

      {/* The sealed box, with a door in its floor (x 420–444) */}
      <rect className="rt-box-fill" x="16" y="74" width="428" height="306" />
      <path className="rt-wall" d="M444 380 V74 H16 V380 H420" />
      <path className="rt-lock" d="M34 92 h14 v11 h-14 z M37 92 v-4 a4 4 0 0 1 8 0 v4" />
      <text className="pz-title" x="58" y="103">
        Sealed sandbox
      </text>
      <text className="f-mono f-dim" x="426" y="103" textAnchor="end">
        WebAssembly
      </text>

      {/* Routes out of the script: down through the door to the check, and up into the wall */}
      <path className="rt-wire rt-w1" d="M415 167 H432 V398 H365 V420" />
      <path className="rt-head rt-head-1f" d="M360.5 413 L365 420 L369.5 413" />
      <path className="rt-head rt-head-1b" d="M422 162.5 L415 167 L422 171.5" />
      <path className="rt-wire rt-w5" d="M286 116 V78" />

      {/* The agent's script */}
      <rect className="pz-card" x="34.5" y="116.5" width="380" height="102" />
      <text className="f-sans f-muted" x="48" y="138">
        the agent's script
      </text>
      {LINES.map((l, i) => (
        <text key={i} className={`f-mono f-text ${l.cls}`} x="48" y={162 + i * 20}>
          {l.code}
        </text>
      ))}

      {/* The engine */}
      <rect className="f-box" x="34.5" y="236.5" width="380" height="40" />
      <text className="f-sans f-text" x="48" y="261">
        QuickJS engine, booted once · a fresh copy per run
      </text>

      {/* Its own memory */}
      <text className="f-mono f-dim" x="34" y="300">
        its own memory
      </text>
      {Array.from({ length: 60 }, (_, n) => (
        <rect
          key={n}
          className={ON.includes(n) ? "f-cell f-on" : "f-cell"}
          x={34 + (n % 20) * 18}
          y={310 + Math.floor(n / 20) * 14}
          width="12"
          height="9"
        />
      ))}
      <text className="f-mono f-hi" x="414" y="370" textAnchor="end">
        the only door
      </text>

      {/* The check */}
      <rect className="rt-gate" x="290.5" y="420.5" width="150" height="56" />
      <text className="f-sans f-title" x="304" y="444">
        modebox checks
      </text>
      <text className="f-mono f-dim" x="304" y="464">
        allow · ask · deny
      </text>

      {/* Request 3's badge sits beside the check */}
      <g className="rt-tag rt-tag-3">
        <rect className="pz-badge" x="146" y="436" width="130" height="24" rx="3" />
        <text className="pz-badge-text" x="211" y="452.5" textAnchor="middle">
          ❚❚ asks you first
        </text>
      </g>

      {/* Your tools */}
      <path className="rt-wire rt-w2" d="M365 477 V504 H89 V538" />
      <path className="rt-head rt-head-2f" d="M84.5 531 L89 538 L93.5 531" />
      <path className="rt-head rt-head-2b" d="M360.5 484 L365 477 L369.5 484" />
      <path className="rt-wire" d="M227 504 V536" markerEnd="url(#rt-head-n)" />
      <path className="rt-wire" d="M365 504 V536" markerEnd="url(#rt-head-n)" />
      {TOOLS.map((t, i) => (
        <g key={t.name}>
          <rect
            className={t.name === "database" ? "f-box rt-tool rt-db" : "f-box rt-tool"}
            x={24.5 + i * 138}
            y="540.5"
            width="130"
            height="40"
          />
          <text className="f-sans f-text" x={40 + i * 138} y="565">
            {t.name}
          </text>
        </g>
      ))}
      <text className="f-mono f-dim" x="24" y="602">
        your tools
      </text>

      {/* Request 1: allowed */}
      <text className="f-mono f-hi rt-tag rt-tag-1" x="375" y="498">
        allowed
      </text>

      {/* Request 2: blocked at the wall */}
      <g className="rt-tag rt-tag-2">
        <path className="rt-x" d="M280 68 l12 12 M292 68 l-12 12" />
        <text className="f-mono f-hi" x="300" y="71">
          blocked
        </text>
      </g>
    </svg>
  );
}
