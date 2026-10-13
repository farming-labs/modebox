"use client";

import { useState } from "react";
import { Rich } from "./code-block";
import { CopyButton } from "./copy-button";
import { ShellCode } from "./shell-code";

export interface Harness {
  id: string;
  label: string;
  file?: string;
  code?: string;
  /** The code, already highlighted on the server. */
  html?: string;
  cmd?: string;
  notes: string[];
}

export function HarnessTabs({ items }: { items: Harness[] }) {
  const [active, setActive] = useState(items[0].id);
  const item = items.find((h) => h.id === active) ?? items[0];

  return (
    <div className="tabs">
      <div className="tab-list" role="tablist" aria-label="Agent">
        {items.map((h) => (
          <button
            key={h.id}
            type="button"
            role="tab"
            id={`harness-${h.id}-tab`}
            aria-selected={h.id === active}
            aria-controls="harness-panel"
            className="tab"
            onClick={() => setActive(h.id)}
          >
            {h.label}
          </button>
        ))}
      </div>
      <div className="tab-panel" role="tabpanel" id="harness-panel" aria-labelledby={`harness-${item.id}-tab`}>
        {item.file && item.code && item.html ? (
          <>
            <div className="code-head">
              <span>{item.file}</span>
              <CopyButton text={item.code} />
            </div>
            <div className="code-body" dangerouslySetInnerHTML={{ __html: item.html }} />
          </>
        ) : null}
        {item.cmd ? (
          <div className={item.file ? "code-install code-then" : "code-install"}>
            <ShellCode cmd={item.cmd} />
            <CopyButton text={item.cmd} />
          </div>
        ) : null}
        <ul className="notes">
          {item.notes.map((n, i) => (
            <li key={i}>
              <Rich text={n} />
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
