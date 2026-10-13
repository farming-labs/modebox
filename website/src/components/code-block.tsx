import type { ReactNode } from "react";
import { CopyButton } from "./copy-button";
import { ShellCode } from "./shell-code";

/** A file's code, highlighted on the server, with its name and a copy button. */
export function CodeBlock({ file, code, html }: { file: string; code: string; html: string }) {
  return (
    <div className="block">
      <div className="code-head">
        <span>{file}</span>
        <CopyButton text={code} />
      </div>
      <div className="code-body" dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}

/** One shell command with a copy button. */
export function Cmd({ cmd }: { cmd: string }) {
  return (
    <div className="block code-install">
      <ShellCode cmd={cmd} />
      <CopyButton text={cmd} />
    </div>
  );
}

/** Text with `backticks` turned into inline code. */
export function Rich({ text }: { text: string }): ReactNode {
  return text.split("`").map((part, i) => (i % 2 ? <code key={i}>{part}</code> : part));
}
