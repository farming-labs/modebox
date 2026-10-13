"use client";

import { useState } from "react";
import { CopyButton } from "./copy-button";
import { ShellCode } from "./shell-code";
import { onTabKeys } from "./tab-keys";

const OPTIONS = [
  { id: "npm", cmd: "npm install modebox" },
  { id: "pip", cmd: "pip install modebox" },
  { id: "cargo", cmd: "cargo install modebox" },
];
const IDS = OPTIONS.map((o) => o.id);

export function InstallSwitch() {
  const [active, setActive] = useState(OPTIONS[0].id);

  return (
    <div className="switch">
      <div
        className="switch-tabs"
        role="tablist"
        aria-label="Package manager"
        onKeyDown={(e) => onTabKeys(e, IDS, active, setActive)}
      >
        {OPTIONS.map((o) => (
          <button
            key={o.id}
            type="button"
            role="tab"
            id={`install-${o.id}-tab`}
            aria-selected={o.id === active}
            aria-controls={`install-${o.id}-panel`}
            tabIndex={o.id === active ? 0 : -1}
            className="switch-tab"
            onClick={() => setActive(o.id)}
          >
            {o.id}
          </button>
        ))}
      </div>
      {OPTIONS.map((o) => (
        <div
          key={o.id}
          className="switch-cmd"
          role="tabpanel"
          id={`install-${o.id}-panel`}
          aria-labelledby={`install-${o.id}-tab`}
          hidden={o.id !== active}
        >
          <ShellCode cmd={o.cmd} />
          <CopyButton text={o.cmd} />
        </div>
      ))}
    </div>
  );
}
