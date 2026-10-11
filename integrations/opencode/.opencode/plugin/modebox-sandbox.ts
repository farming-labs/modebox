// opencode plugin: run every `bash` tool call inside modebox's OS sandbox
// (Seatbelt on macOS, bubblewrap on Linux). Writes are confined to the
// project and temp dirs, secret stores (~/.ssh, ~/.aws, ...) are unreadable,
// and network is off unless MODEBOX_NETWORK=1, or limited to the hosts in
// MODEBOX_ALLOW_HOSTS (comma-separated, e.g. "registry.npmjs.org,*.github.com").
//
// opencode itself runs shell commands with no OS sandbox; this closes that gap
// without changing how the agent works.
import type { Plugin } from "@opencode-ai/plugin"

const quote = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`

export const ModeboxSandbox: Plugin = async ({ worktree, directory }) => {
  const root = worktree || directory
  const hosts = (process.env.MODEBOX_ALLOW_HOSTS ?? "").split(",").map((h) => h.trim()).filter(Boolean)
  const network =
    process.env.MODEBOX_NETWORK === "1" ? " --network" : hosts.map((h) => ` --allow-host ${quote(h)}`).join("")
  const bin = process.env.MODEBOX_BIN ?? "modebox"
  return {
    "tool.execute.before": async (input, output) => {
      if (input.tool !== "bash" || typeof output.args?.command !== "string") return
      if (output.args.command.startsWith(`${bin} sh `)) return
      output.args.command = `${bin} sh --root ${quote(root)}${network} -- ${quote(output.args.command)}`
    },
  }
}
