import type { CSSProperties } from "react";

// "Behind the scenes": what gets compiled, and when, as three short lines.
// A highlight walks the steps in order (bts-* in globals.css).

const LANES: { when: string; steps: [string, string][] }[] = [
  {
    when: "At release",
    steps: [
      ["JS engine", "QuickJS-ng, in C"],
      ["to WebAssembly", "clang + rustc"],
    ],
  },
  {
    when: "First start",
    steps: [
      ["boot once", "Wizer"],
      ["machine code", "Cranelift"],
      ["cached", "on disk"],
    ],
  },
  {
    when: "Every run",
    steps: [
      ["fresh copy", "~0.1 ms"],
      ["strip types", "swc"],
      ["to bytecode", "QuickJS"],
      ["interpret", "QuickJS"],
    ],
  },
];

export function BehindScenes() {
  let n = 0;
  return (
    <figure className="fig">
      <div className="bts">
        {LANES.map((lane) => (
          <div className="lane" key={lane.when}>
            <span className="lane-when">{lane.when}</span>
            <ol className="lane-steps">
              {lane.steps.map(([label, tool], i) => (
                <li key={label} className="lane-item">
                  {i > 0 ? (
                    <span className="lane-arrow" aria-hidden="true">
                      →
                    </span>
                  ) : null}
                  <span className="step" style={{ "--n": n++ } as CSSProperties}>
                    <span className="step-label">{label}</span>
                    <span className="step-tool">{tool}</span>
                  </span>
                </li>
              ))}
            </ol>
          </div>
        ))}
      </div>
      <figcaption className="fig-cap">
        Only the engine becomes machine code, once. The agent's script stays bytecode, run inside the
        sandbox.
      </figcaption>
    </figure>
  );
}
