//! Build the sandbox's toolbox from config: downstream MCP servers become
//! `server.tool(input)` functions, plus the OS-sandboxed `sh`.

#![allow(clippy::result_large_err)] // ToolError carries messages; boxing it buys nothing here.

use std::sync::Arc;
use std::time::Duration;

use modebox_core::shell::{ShellPolicy, command_of, shell_tool};
use modebox_core::{Approval, Call, Tool, ToolError, Toolbox, Value};
use anyhow::{Context, Result};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;

use crate::config::{Config, command_allowed, glob_match};

pub type Client = Arc<RunningService<RoleClient, ()>>;

pub struct Bridge {
    pub tools: Toolbox,
    /// Keep downstream connections alive for as long as the tools are used.
    pub clients: Vec<(String, Client)>,
}

/// JS-safe identifier for an MCP tool or server name.
pub fn ident(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '$' { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

pub async fn build(cfg: &Config) -> Result<Bridge> {
    let mut tools = Toolbox::new();
    let mut clients = Vec::new();

    for (server, sc) in &cfg.mcp_servers {
        let mut cmd = tokio::process::Command::new(&sc.command);
        cmd.args(&sc.args).envs(&sc.env).stderr(std::process::Stdio::null());
        if let Some(cwd) = &sc.cwd {
            cmd.current_dir(cwd);
        }
        let transport = TokioChildProcess::new(cmd).with_context(|| format!("starting MCP server {server}"))?;
        let client: Client = Arc::new(
            tokio::time::timeout(Duration::from_secs(30), ().serve(transport))
                .await
                .with_context(|| format!("MCP server {server} did not start in time"))?
                .with_context(|| format!("connecting to MCP server {server}"))?,
        );
        let listed = client.list_all_tools().await.with_context(|| format!("listing tools of {server}"))?;
        let ns = ident(server);
        for t in listed {
            let path = format!("{ns}.{}", ident(&t.name));
            let remote_name = t.name.to_string();
            let c = client.clone();
            let mut tool = Tool::new(path.clone(), move |call: Call| {
                let c = c.clone();
                let name = remote_name.clone();
                async move { call_remote(&c, &name, call).await }
            })
            .input_schema(serde_json::Value::Object((*t.input_schema).clone()));
            if let Some(d) = &t.description {
                tool = tool.description(d.to_string());
            }
            if let Some(o) = &t.output_schema {
                tool = tool.output_schema(serde_json::Value::Object((**o).clone()));
            }
            if cfg.approve.iter().any(|p| glob_match(p, &path)) {
                tool = tool.approval(Approval::Always);
            }
            tools.add(tool);
        }
        clients.push((server.clone(), client));
    }

    if cfg.fs.enabled {
        use modebox_core::vfs::{Mount, MountMode, Vfs, VfsConfig};
        let root_mode = if cfg.fs.read_only { MountMode::ReadOnly } else { MountMode::Staged };
        let mut mounts = vec![Mount::dir("", cfg.workspace(), root_mode).map_err(anyhow::Error::msg)?, Mount::memory("tmp/")];
        for (prefix, m) in &cfg.fs.mounts {
            let mode = match (m.mode.as_deref(), m.read_only) {
                (_, true) | (Some("read-only"), _) => MountMode::ReadOnly,
                (Some("direct"), _) => MountMode::Direct,
                (Some("staged"), _) => MountMode::Staged,
                (None, _) if m.memory => MountMode::Direct,
                (None, _) => MountMode::Staged,
                (Some(other), _) => anyhow::bail!("fs mount {prefix}: unknown mode {other:?}"),
            };
            let options: Vec<(String, String)> = m
                .options
                .iter()
                .map(|(k, v)| {
                    let v = match v.strip_prefix('$') {
                        Some(var) => std::env::var(var).with_context(|| format!("fs mount {prefix}: ${var} is not set"))?,
                        None => v.clone(),
                    };
                    Ok((k.clone(), v))
                })
                .collect::<Result<_>>()?;
            let mount = match (&m.dir, &m.url, m.memory) {
                (Some(dir), None, false) => Mount::dir(prefix.clone(), dir, mode),
                (None, Some(url), false) => Mount::url(prefix.clone(), url, options, mode),
                (None, None, true) => Ok(Mount::new(prefix.clone(), std::sync::Arc::new(modebox_core::store::memory::MemoryStore::new()), mode)),
                _ => anyhow::bail!("fs mount {prefix}: give exactly one of \"dir\", \"url\" or \"memory\": true"),
            }
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("fs mount {prefix}"))?;
            mounts.retain(|x| x.prefix != mount.prefix);
            mounts.push(mount);
        }
        let mut vc = VfsConfig::new(mounts);
        if let Some(deny) = &cfg.fs.deny {
            vc.deny = deny.clone();
        }
        vc.commit_approval = cfg.fs.commit_approval;
        let vfs = Vfs::new(vc).map_err(anyhow::Error::msg)?;
        vfs.install(&mut tools, "fs");
    }

    if cfg.shell.enabled {
        let mut policy = ShellPolicy::new(cfg.workspace());
        policy.network = cfg.shell.network;
        policy.writable = cfg.shell.writable.clone();
        policy.pass_env = cfg.shell.pass_env.clone();
        if let Some(ms) = cfg.shell.timeout_ms {
            policy.timeout = Duration::from_millis(ms);
        }
        let allow = cfg.shell.allow.clone();
        let forced = cfg.approve.iter().any(|p| glob_match(p, "sh"));
        let tool = shell_tool("sh", policy).approval(Approval::When(Arc::new(move |call: &Call| {
            let cmd = command_of(call).unwrap_or("");
            if !forced && command_allowed(&allow, cmd) {
                None
            } else {
                Some(format!("run shell command: {cmd}"))
            }
        })));
        tools.add(tool);
    }

    Ok(Bridge { tools, clients })
}

async fn call_remote(client: &Client, name: &str, call: Call) -> Result<Value, ToolError> {
    let mut params = CallToolRequestParams::default();
    params.name = name.to_owned().into();
    params.arguments = match call.input().to_json() {
        serde_json::Value::Object(m) => Some(m),
        serde_json::Value::Null => Some(Default::default()),
        other => {
            return Err(ToolError::new(format!("{}: expected an input object, got {other}", call.name)).with_name("TypeError"));
        }
    };
    let result = client.call_tool(params).await.map_err(|e| ToolError::new(format!("{}: {e}", call.name)))?;
    to_value(&call.name, result)
}

/// MCP result → sandbox value. Structured content wins; otherwise text
/// blocks are joined, and parsed as JSON when they are JSON.
fn to_value(tool: &str, r: CallToolResult) -> Result<Value, ToolError> {
    let text: Vec<String> = r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect();
    let joined = text.join("\n");
    if r.is_error == Some(true) {
        let msg = if joined.is_empty() { format!("{tool} failed") } else { joined };
        return Err(ToolError::new(msg).with_code("TOOL_ERROR"));
    }
    if let Some(s) = r.structured_content {
        return Ok(Value::from(s));
    }
    if text.len() == 1 {
        let t = text[0].trim();
        if (t.starts_with('{') || t.starts_with('[')) && let Ok(j) = serde_json::from_str::<serde_json::Value>(t) {
            return Ok(Value::from(j));
        }
    }
    if text.is_empty() && !r.content.is_empty() {
        return Ok(Value::from(serde_json::to_value(&r.content).unwrap_or_default()));
    }
    Ok(Value::from(joined))
}
