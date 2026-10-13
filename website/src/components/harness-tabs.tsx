"use client";

import { useState } from "react";
import { Rich } from "./code-block";
import { CopyButton } from "./copy-button";
import { ShellCode } from "./shell-code";
import { onTabKeys } from "./tab-keys";

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
  const ids = items.map((h) => h.id);

  return (
    <div className="tabs">
      <div
        className="tab-list"
        role="tablist"
        aria-label="Agent"
        onKeyDown={(e) => onTabKeys(e, ids, active, setActive)}
      >
        {items.map((h) => (
          <button
            key={h.id}
            type="button"
            role="tab"
            id={`harness-${h.id}-tab`}
            aria-selected={h.id === active}
            aria-controls={`harness-${h.id}-panel`}
            tabIndex={h.id === active ? 0 : -1}
            className="tab"
            onClick={() => setActive(h.id)}
          >
            {h.label}
          </button>
        ))}
      </div>
      {items.map((item) => (
        <div
          key={item.id}
          className="tab-panel"
          role="tabpanel"
          id={`harness-${item.id}-panel`}
          aria-labelledby={`harness-${item.id}-tab`}
          hidden={item.id !== active}
        >
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
      ))}
    </div>
  );
}
