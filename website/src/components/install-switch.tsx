"use client";

import { useState } from "react";
import { CopyButton } from "./copy-button";

const OPTIONS = [
  { id: "npm", cmd: "npm install modebox" },
  { id: "pip", cmd: "pip install modebox" },
  { id: "cargo", cmd: "cargo install modebox" },
];

export function InstallSwitch() {
  const [active, setActive] = useState(OPTIONS[0].id);
  const cmd = OPTIONS.find((o) => o.id === active)?.cmd ?? OPTIONS[0].cmd;

  return (
    <div className="switch">
      <div className="switch-tabs" role="tablist" aria-label="Package manager">
        {OPTIONS.map((o) => (
          <button
            key={o.id}
            type="button"
            role="tab"
            aria-selected={o.id === active}
            className="switch-tab"
            onClick={() => setActive(o.id)}
          >
            {o.id}
          </button>
        ))}
      </div>
      <div className="switch-cmd">
        <code>
          <span className="prompt">$</span> {cmd}
        </code>
        <CopyButton text={cmd} />
      </div>
    </div>
  );
}
