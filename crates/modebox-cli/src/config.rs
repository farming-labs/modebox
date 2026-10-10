//! `modebox.json`: which tools the sandbox gets and what needs approval.
//!
//! `mcpServers` uses the same shape as `.mcp.json` / Claude Desktop configs,
//! so an existing file can be pointed at directly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use modebox_core::Limits;
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// Workspace root for `sh` (default: current directory).
    pub workspace: Option<PathBuf>,
    /// Downstream MCP servers whose tools become `server.tool()` functions.
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, ServerConfig>,
    #[serde(default)]
    pub shell: ShellConfig,
    #[serde(default)]
    pub fs: FsConfig,
    /// Glob patterns (`*` wildcard) of tool paths that need approval, e.g.
    /// `"github.create_*"`.
    #[serde(default)]
    pub approve: Vec<String>,
    #[serde(default)]
    pub limits: LimitsConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShellConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub writable: Vec<PathBuf>,
    pub timeout_ms: Option<u64>,
    /// Commands that run without approval, matched by prefix on whole words
    /// (`"git status"` allows `git status -s`, not `git statusx`). `["*"]`
    /// allows everything. Anything else suspends for approval.
    #[serde(default = "default_allow")]
    pub allow: Vec<String>,
    /// Extra environment variables passed into the sandbox.
    #[serde(default)]
    pub pass_env: Vec<String>,
}

fn yes() -> bool {
    true
}

fn default_allow() -> Vec<String> {
    [
        "ls", "cat", "head", "tail", "wc", "pwd", "echo", "grep", "rg", "find", "file", "stat", "du", "tree", "sort",
        "uniq", "cut", "jq", "diff", "git status", "git diff", "git log", "git show", "git branch",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Default for ShellConfig {
    fn default() -> Self {
        ShellConfig {
            enabled: true,
            network: false,
            writable: Vec::new(),
            timeout_ms: None,
            allow: default_allow(),
            pass_env: Vec::new(),
        }
    }
}

/// The `fs.*` object-store API over the workspace (and extra mounts).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Make the workspace itself read-only (no staging or commits).
    #[serde(default)]
    pub read_only: bool,
    /// Extra mounts by key prefix: `{"docs/": {"dir": "../docs", "readOnly": true}, "scratch/": {"memory": true}}`.
    #[serde(default)]
    pub mounts: BTreeMap<String, MountConfig>,
    /// Globs that can't be read or written; replaces the defaults (.env, keys, .git/).
    pub deny: Option<Vec<String>>,
    #[serde(default = "yes")]
    pub commit_approval: bool,
}

impl Default for FsConfig {
    fn default() -> Self {
        FsConfig { enabled: true, read_only: false, mounts: BTreeMap::new(), deny: None, commit_approval: true }
    }
}

/// One mount: exactly one of `dir`, `url` or `memory`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MountConfig {
    /// A local folder.
    pub dir: Option<PathBuf>,
    /// `s3://bucket/prefix`, `gs://…`, `az://…`, `https://…`, `file://…`, `memory://`.
    pub url: Option<String>,
    #[serde(default)]
    pub memory: bool,
    /// `"read-only"`, `"staged"` (default for dir/url) or `"direct"` (default for memory).
    pub mode: Option<String>,
    /// Shorthand for `mode: "read-only"`.
    #[serde(default)]
    pub read_only: bool,
    /// Driver options for `url` mounts (region, endpoint, …). A value like
    /// `"$R2_SECRET"` is read from the environment, so secrets stay out of the file.
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LimitsConfig {
    pub timeout_ms: Option<u64>,
    pub fuel: Option<u64>,
    pub memory_mb: Option<usize>,
    pub max_host_calls: Option<u32>,
    pub max_concurrency: Option<usize>,
}

impl LimitsConfig {
    pub fn to_limits(&self) -> Limits {
        let mut l = Limits { timeout: Duration::from_secs(60), ..Limits::default() };
        if let Some(ms) = self.timeout_ms {
            l.timeout = Duration::from_millis(ms);
        }
        l.fuel = self.fuel;
        if let Some(mb) = self.memory_mb {
            l.memory_bytes = mb << 20;
        }
        if let Some(n) = self.max_host_calls {
            l.max_host_calls = n;
        }
        if let Some(n) = self.max_concurrency {
            l.max_concurrency = n;
        }
        l
    }
}

impl Config {
    /// Load `path`, or `./modebox.json` if it exists, or defaults.
    pub fn load(path: Option<&Path>) -> Result<Config> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => {
                let p = PathBuf::from("modebox.json");
                if !p.exists() {
                    return Ok(Config::default());
                }
                p
            }
        };
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        // Relative paths are relative to the config file.
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let base = if base.as_os_str().is_empty() { PathBuf::from(".") } else { base };
        cfg.workspace = Some(base.join(cfg.workspace.clone().unwrap_or_else(|| PathBuf::from("."))));
        for s in cfg.mcp_servers.values_mut() {
            if let Some(cwd) = &s.cwd {
                s.cwd = Some(base.join(cwd));
            }
        }
        cfg.shell.writable = cfg.shell.writable.iter().map(|w| base.join(w)).collect();
        for m in cfg.fs.mounts.values_mut() {
            if let Some(d) = &m.dir {
                m.dir = Some(base.join(d));
            }
        }
        Ok(cfg)
    }

    pub fn workspace(&self) -> PathBuf {
        self.workspace.clone().unwrap_or_else(|| PathBuf::from("."))
    }
}

/// `*` matches any run of characters (including dots).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(pos) = rest.find(part) else { return false };
            rest = &rest[pos + part.len()..];
        }
    }
    true
}

/// Is `command` covered by an allow entry (whole-word prefix match)?
/// Commands chaining with `;`, `&&`, `|`, backticks or `$(` are never
/// auto-allowed: each piece could be anything.
pub fn command_allowed(allow: &[String], command: &str) -> bool {
    if allow.iter().any(|a| a == "*") {
        return true;
    }
    let cmd = command.trim();
    if ["&&", "||", ";", "|", "`", "$(", ">", "<", "\n"].iter().any(|op| cmd.contains(op)) {
        return false;
    }
    let words: Vec<&str> = cmd.split_whitespace().collect();
    allow.iter().any(|a| {
        let aw: Vec<&str> = a.split_whitespace().collect();
        !aw.is_empty() && words.len() >= aw.len() && words[..aw.len()] == aw[..]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("github.create_*", "github.create_issue"));
        assert!(glob_match("*.delete*", "fs.delete_file"));
        assert!(glob_match("sh", "sh"));
        assert!(!glob_match("github.create_*", "github.list_issues"));
    }

    #[test]
    fn command_allowlist() {
        let allow = default_allow();
        assert!(command_allowed(&allow, "git status -s"));
        assert!(command_allowed(&allow, "ls -la src"));
        assert!(!command_allowed(&allow, "git push"));
        assert!(!command_allowed(&allow, "ls; rm -rf ~"));
        assert!(!command_allowed(&allow, "cat $(echo /etc/passwd)"));
        assert!(!command_allowed(&allow, "echo hi > file"));
        assert!(command_allowed(&["*".into()], "anything && everything"));
    }
}
