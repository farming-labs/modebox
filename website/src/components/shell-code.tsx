// A shell command colored like jevia.dev's: programs bright, options and
// operators dim, everything else soft.
const OPERATORS = new Set(["|", "||", "&&", ";", ">", ">>"]);

export function ShellCode({ cmd }: { cmd: string }) {
  const parts = cmd.split(/(\s+)/);
  let expectCommand = true;
  return (
    <code className="sh">
      <span className="sh-prompt">$ </span>
      {parts.map((p, i) => {
        if (/^\s+$/.test(p) || p === "") return p;
        let cls = "sh-plain";
        if (OPERATORS.has(p)) {
          cls = "sh-op";
          expectCommand = true;
        } else if (expectCommand) {
          cls = "sh-cmd";
          expectCommand = p === "sudo";
        } else if (p.startsWith("-")) {
          cls = "sh-opt";
        }
        return (
          <span key={i} className={cls}>
            {p}
          </span>
        );
      })}
    </code>
  );
}
