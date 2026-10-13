import { CopyButton } from "./copy-button";

export function Command({ children }: { children: string }) {
  return (
    <div className="cmd">
      <code>
        <span className="prompt">$</span> {children}
      </code>
      <CopyButton text={children} />
    </div>
  );
}
