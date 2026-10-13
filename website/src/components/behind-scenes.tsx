import type { CSSProperties } from "react";

// "Behind the scenes": what gets compiled, by what, and when. A highlight
// walks the ten steps in order, one per second (bts-* in globals.css).

interface Step {
  tool: string;
  what: string;
  out: string;
}

const LANES: { when: string; where: string; steps: Step[] }[] = [
  {
    when: "Once, at release",
    where: "on GitHub's build machines",
    steps: [
      { tool: "QuickJS-ng", what: "A small JavaScript engine, written in C.", out: "C source" },
      { tool: "clang + rustc", what: "Compile the engine and modebox's Rust glue to WebAssembly.", out: "wasm32" },
      { tool: "modebox_guest.wasm", what: "One file, built into every modebox package.", out: "1.7 MB" },
    ],
  },
  {
    when: "Once per machine",
    where: "the first time modebox starts",
    steps: [
      { tool: "Wizer", what: "Runs the engine once to boot it, and saves the booted memory.", out: "booted image" },
      { tool: "Cranelift", what: "Compiles the WebAssembly into machine code for your CPU.", out: "arm64 · x86-64" },
      { tool: "~/.cache/modebox", what: "Keeps that machine code, so later starts skip both steps.", out: ".cwasm" },
    ],
  },
  {
    when: "Every run",
    where: "each time an agent calls execute",
    steps: [
      { tool: "Fresh sandbox", what: "A copy-on-write copy of the booted engine, in wasmtime.", out: "~0.1 ms" },
      { tool: "swc", what: "Strips TypeScript types, outside the sandbox.", out: "JavaScript" },
      { tool: "QuickJS compiler", what: "Turns the script into QuickJS bytecode, inside the sandbox.", out: "bytecode" },
      { tool: "QuickJS interpreter", what: "Runs that bytecode, one instruction at a time.", out: "result" },
    ],
  },
];

export function BehindScenes() {
  let n = 0;
  return (
    <figure className="fig bts-fig">
      <div className="bts">
        {LANES.map((lane) => (
          <div className="lane" key={lane.when}>
            <div className="lane-head">
              <span className="lane-when">{lane.when}</span>
              <span className="lane-where">{lane.where}</span>
            </div>
            <ol className="lane-steps">
              {lane.steps.map((step) => {
                const i = n++;
                return (
                  <li className="step" key={step.tool} style={{ "--n": i } as CSSProperties}>
                    <span className="step-tool">{step.tool}</span>
                    <span className="step-what">{step.what}</span>
                    <span className="step-out">{step.out}</span>
                  </li>
                );
              })}
            </ol>
          </div>
        ))}
      </div>
      <figcaption className="fig-cap">
        Only the engine is ever compiled to machine code, and only once per machine. The agent's
        script becomes QuickJS bytecode and is interpreted inside the sandbox, so it never runs as
        native code.
      </figcaption>
    </figure>
  );
}
