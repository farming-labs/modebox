"use client";

import { useState } from "react";
import { CopyButton } from "./copy-button";
import { ShellCode } from "./shell-code";
import { onTabKeys } from "./tab-keys";

export interface CodeTab {
  id: string;
  label: string;
  install: string;
  file: string;
  code: string;
  /** The code, already highlighted on the server. */
  html: string;
  then?: string;
}

export function CodeTabs({ tabs }: { tabs: CodeTab[] }) {
  const [active, setActive] = useState(tabs[0].id);
  const ids = tabs.map((t) => t.id);

  return (
    <div className="tabs">
      <div
        className="tab-list"
        role="tablist"
        aria-label="Runtime"
        onKeyDown={(e) => onTabKeys(e, ids, active, setActive)}
      >
        {tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            id={`code-${t.id}-tab`}
            aria-selected={t.id === active}
            aria-controls={`code-${t.id}-panel`}
            tabIndex={t.id === active ? 0 : -1}
            className="tab"
            onClick={() => setActive(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>

      {/* Every panel is in the HTML, so all four samples are there without JavaScript. */}
      {tabs.map((tab) => (
        <div
          key={tab.id}
          className="tab-panel"
          role="tabpanel"
          id={`code-${tab.id}-panel`}
          aria-labelledby={`code-${tab.id}-tab`}
          hidden={tab.id !== active}
        >
          <div className="code-install">
            <ShellCode cmd={tab.install} />
            <CopyButton text={tab.install} />
          </div>
          <div className="code-head">
            <span>{tab.file}</span>
            <CopyButton text={tab.code} />
          </div>
          <div className="code-body" dangerouslySetInnerHTML={{ __html: tab.html }} />
          {tab.then ? (
            <div className="code-install code-then">
              <ShellCode cmd={tab.then} />
              <CopyButton text={tab.then} />
            </div>
          ) : null}
        </div>
      ))}
    </div>
  );
}
