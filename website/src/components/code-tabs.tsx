"use client";

import { useState, type ReactNode } from "react";
import { CopyButton } from "./copy-button";

export interface CodeTab {
  id: string;
  label: string;
  install: string;
  file: string;
  lang: "json" | "ts" | "py" | "rs";
  code: string;
  then?: string;
}

const KEYWORDS: Record<CodeTab["lang"], string[]> = {
  json: [],
  ts: ["import", "from", "const", "await", "if", "return", "new"],
  py: ["import", "await", "if", "def", "async", "return"],
  rs: ["use", "let", "mut", "if", "await", "fn"],
};

// A deliberately small highlighter: comments, strings and keywords, in greys.
function highlight(code: string, lang: CodeTab["lang"]): ReactNode[] {
  const comment = lang === "py" ? "#.*" : "//.*";
  const kw = KEYWORDS[lang];
  const parts = [comment, `"(?:[^"\\\\]|\\\\.)*"`, kw.length ? `\\b(?:${kw.join("|")})\\b` : null]
    .filter(Boolean)
    .join("|");
  const re = new RegExp(parts, "g");
  const out: ReactNode[] = [];
  let last = 0;
  let k = 0;
  for (const m of code.matchAll(re)) {
    const i = m.index ?? 0;
    if (i > last) out.push(code.slice(last, i));
    const t = m[0];
    const cls = t.startsWith("#") || t.startsWith("//") ? "tok-com" : t.startsWith('"') ? "tok-str" : "tok-kw";
    out.push(
      <span key={k++} className={cls}>
        {t}
      </span>,
    );
    last = i + t.length;
  }
  if (last < code.length) out.push(code.slice(last));
  return out;
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
          <code>
            <span className="prompt">$</span> {tab.install}
          </code>
          <CopyButton text={tab.install} />
        </div>
        <div className="code-head">
          <span>{tab.file}</span>
          <CopyButton text={tab.code} />
        </div>
        <pre className="code-body">
          <code>{highlight(tab.code, tab.lang)}</code>
        </pre>
        {tab.then ? (
          <div className="code-install code-then">
            <code>
              <span className="prompt">$</span> {tab.then}
            </code>
            <CopyButton text={tab.then} />
          </div>
        ) : null}
      </div>
    </div>
  );
}
