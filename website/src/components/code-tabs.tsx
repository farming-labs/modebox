"use client";

import { useState } from "react";
import { CopyButton } from "./copy-button";
import { ShellCode } from "./shell-code";

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
  const tab = tabs.find((t) => t.id === active) ?? tabs[0];

  return (
    <div className="tabs">
      <div className="tab-list" role="tablist" aria-label="Runtime">
        {tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            id={`code-${t.id}-tab`}
            aria-selected={t.id === active}
            aria-controls="code-panel"
            className="tab"
            onClick={() => setActive(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>

      <div className="tab-panel" role="tabpanel" id="code-panel" aria-labelledby={`code-${tab.id}-tab`}>
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
    </div>
  );
}
