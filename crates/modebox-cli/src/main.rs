//! `modebox`: run agent code in a sandbox, run shell commands in an OS
//! sandbox, or serve both to any agent harness over MCP.

mod bridge;
mod config;
mod mcp;
mod secret;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use modebox_core::shell::{ShellPolicy, sandbox_status, shell_tool};
use modebox_core::{
    Call, ClockMode, Engine, EngineConfig, Event, Interrupt, Outcome, Resolution, RunOptions, RunResult,
    StoreContinuations, Value, declarations,
};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::config::Config;

#[derive(Parser)]
#[command(name = "modebox", version, about = "Sandboxed code execution for AI agents")]
struct Cli {
    /// Config file (default: ./modebox.json if present)
    #[arg(long, short, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run JavaScript/TypeScript in the sandbox with the configured tools
    Run {
        /// Source file (`-` for stdin)
        file: Option<PathBuf>,
        /// Inline code instead of a file
        #[arg(short, long)]
        eval: Option<String>,
        /// Print the full result as JSON (no prompts; suspensions print a continuation)
        #[arg(long)]
        json: bool,
        #[arg(long)]
        timeout_ms: Option<u64>,
        /// Deterministic CPU budget (wasm fuel units)
        #[arg(long)]
        fuel: Option<u64>,
        /// Use virtual time starting at this epoch-ms (timers are instant)
        #[arg(long)]
        virtual_time: Option<f64>,
        /// Approve everything without asking
        #[arg(long, short)]
        yes: bool,
    },
    /// Resume a suspended run from its continuation token. Asks about each
    /// pending call when stdin is a terminal; otherwise denies them unless
    /// told what to approve.
    Resume {
        token: String,
        /// Approve every pending call
        #[arg(long, conflicts_with = "approve_only")]
        approve: bool,
        /// Approve only these call ids (comma-separated) and deny the rest
        #[arg(long, value_delimiter = ',')]
        approve_only: Vec<u32>,
    },
    /// Show the calls a continuation token is waiting on, without using it
    Inspect { token: String },
    /// Serve code mode to an agent harness over MCP (stdio)
    Mcp,
    /// Print TypeScript declarations for the configured tools
    Types,
    /// Run one shell command inside the OS sandbox
    Sh {
        /// Workspace root: writable, and the working directory (default: config workspace or `.`)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Allow network access
        #[arg(long)]
        network: bool,
        /// Allow only this host (repeatable; `*.example.com` for subdomains)
        #[arg(long = "allow-host", value_name = "HOST")]
        allow_hosts: Vec<String>,
        /// Extra writable directory (repeatable)
        #[arg(long)]
        writable: Vec<PathBuf>,
        #[arg(long, default_value_t = 120_000)]
        timeout_ms: u64,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
}

#[tokio::main]
async fn main() {
    // The sandbox runs this binary as its network bridge (see `ShellPolicy::net_bridge`).
    #[cfg(unix)]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("__net-bridge") {
            modebox_core::net::net_bridge_main(&args[2..]);
        }
    }
    if let Err(e) = real_main().await {
        eprintln!("modebox: {e:#}");
        std::process::exit(1);
    }
}

async fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    match cli.cmd {
        Cmd::Run { file, eval, json, timeout_ms, fuel, virtual_time, yes } => {
            let code = match (eval, file) {
                (Some(c), _) => c,
                (None, Some(p)) if p.as_os_str() == "-" => {
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s)?;
                    s
                }
                (None, Some(p)) => std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?,
                (None, None) => bail!("give a file, `-` for stdin, or -e CODE"),
            };
            let engine = engine()?;
            let bridge = bridge::build(&cfg).await?;
            let mut opts = RunOptions { limits: cfg.limits.to_limits(), ..RunOptions::default() };
            if let Some(ms) = timeout_ms {
                opts.limits.timeout = Duration::from_millis(ms);
            }
            if fuel.is_some() {
                opts.limits.fuel = fuel;
            }
            if let Some(t) = virtual_time {
                opts.clock = ClockMode::Virtual { start_ms: t };
            }
            if !json {
                opts.on_event = Some(Arc::new(|e: &Event| {
                    if let Event::Log(l) = e {
                        let tag = if l.level == "log" { String::new() } else { format!("[{}] ", l.level) };
                        eprintln!("{tag}{}", l.message);
                    }
                }));
            }
            let mut r = engine.run(&code, &bridge.tools, opts.clone()).await;
            let interactive = !json && std::io::stdin().is_terminal();
            while let Outcome::Suspended(s) = &r.outcome {
                if !(yes || interactive) {
                    break;
                }
                let mut decisions = Vec::new();
                for i in &s.interrupts {
                    let ok = yes || ask(&format!("{} wants {}({})", i.kind, i.tool, Value::Array(i.args.clone()).to_json()))?;
                    decisions.push((i.id, if ok { Resolution::Approve } else { Resolution::Deny("denied at the prompt".into()) }));
                }
                r = engine.resume(&s.continuation, decisions, &bridge.tools, opts.clone()).await;
            }
            print_result(&r, json)
        }
        Cmd::Resume { token, approve, approve_only } => {
            let engine = engine()?;
            let info = engine.inspect_continuation(&token).map_err(|e| anyhow::anyhow!("{}", e.message))?;
            let pending: Vec<u32> = info.interrupts.iter().map(|i| i.id).collect();
            if let Some(id) = approve_only.iter().find(|id| !pending.contains(id)) {
                bail!("call {id} isn't pending in this continuation (pending: {pending:?})");
            }
            let interactive = !approve && approve_only.is_empty() && std::io::stdin().is_terminal();
            let mut decisions = Vec::new();
            for i in &info.interrupts {
                let ok = if interactive {
                    ask(&format!("[call {}] {} wants {}({})", i.id, i.kind, i.tool, Value::Array(i.args.clone()).to_json()))?
                } else {
                    approve || approve_only.contains(&i.id)
                };
                decisions.push((i.id, if ok { Resolution::Approve } else { Resolution::Deny("denied".into()) }));
            }
            let bridge = bridge::build(&cfg).await?;
            let opts = RunOptions { limits: cfg.limits.to_limits(), ..RunOptions::default() };
            let r = engine.resume(&token, decisions, &bridge.tools, opts).await;
            print_result(&r, true)
        }
        Cmd::Inspect { token } => {
            let info = engine()?.inspect_continuation(&token).map_err(|e| anyhow::anyhow!("{}", e.message))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "runId": info.run_id,
                    "expiresMs": info.expires_ms,
                    "interrupts": interrupts_json(&info.interrupts),
                }))?
            );
            Ok(())
        }
        Cmd::Mcp => {
            let engine = engine()?;
            let bridge = bridge::build(&cfg).await?;
            let has_approvals = !cfg.approve.is_empty() || cfg.shell.enabled;
            let approve_tool = has_approvals && cfg.mcp.approve_tool;
            let server = mcp::ModeboxServer::new(engine, bridge.tools, cfg.limits.to_limits(), approve_tool);
            let _keep = bridge.clients;
            server.serve_stdio().await
        }
        Cmd::Types => {
            let bridge = bridge::build(&cfg).await?;
            print!("{}", declarations(&bridge.tools));
            Ok(())
        }
        Cmd::Sh { root, network, allow_hosts, writable, timeout_ms, command } => {
            if let Err(why) = sandbox_status() {
                bail!("no OS sandbox is available: {why}");
            }
            let mut policy = ShellPolicy::new(root.unwrap_or_else(|| cfg.workspace()));
            policy.network = network || cfg.shell.network.all();
            policy.allow_hosts = allow_hosts.into_iter().chain(cfg.shell.network.hosts().iter().cloned()).collect();
            policy.net_bridge = std::env::current_exe().ok();
            policy.writable = writable;
            policy.timeout = Duration::from_millis(timeout_ms);
            policy.max_output_bytes = 16 << 20;
            let tool = shell_tool("sh", policy);
            let call = Call { name: "sh".into(), args: vec![Value::from(command.join(" "))], id: 0, run_id: "cli".into(), resume: None };
            let out = tool.invoke(call).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            print!("{}", out.get("stdout").and_then(Value::as_str).unwrap_or(""));
            eprint!("{}", out.get("stderr").and_then(Value::as_str).unwrap_or(""));
            if out.get("timedOut") == Some(&Value::Bool(true)) {
                eprintln!("modebox: command timed out");
            }
            let code = out.get("exitCode").and_then(Value::as_f64).unwrap_or(124.0) as i32;
            std::process::exit(code);
        }
    }
}

/// The CLI's engine. Unless `MODEBOX_SECRET` is set, continuation tokens are
/// sealed with a key saved on disk, so they still open after a restart.
fn engine() -> Result<Engine> {
    let mut cfg = EngineConfig::default();
    if cfg.secret.is_none()
        && let Some(path) = secret::default_path()
    {
        cfg.secret = Some(secret::load_or_create(&path)?);
    }
    // Tokens are resumed by other processes (`modebox resume`, MCP retries),
    // so record used ones on disk rather than in this process's memory.
    if let Some(dir) = continuations_dir() {
        let used = StoreContinuations::local(&dir)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("opening {}", dir.display()))?;
        cfg.continuations = Arc::new(used);
    }
    Engine::with_config(cfg)
}

/// `$MODEBOX_STATE_DIR/continuations`, else under `$XDG_STATE_HOME/modebox`,
/// else under `~/.local/state/modebox`.
fn continuations_dir() -> Option<PathBuf> {
    let base = std::env::var_os("MODEBOX_STATE_DIR").map(PathBuf::from).or_else(|| {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
            .map(|p| p.join("modebox"))
    })?;
    Some(base.join("continuations"))
}

fn interrupts_json(interrupts: &[Interrupt]) -> Vec<serde_json::Value> {
    interrupts
        .iter()
        .map(|i| serde_json::json!({ "id": i.id, "tool": i.tool, "kind": i.kind, "args": Value::Array(i.args.clone()).to_json(), "payload": i.payload.to_json() }))
        .collect()
}

fn ask(question: &str) -> Result<bool> {
    eprint!("\x1b[33m?\x1b[0m {question} — allow? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

fn print_result(r: &RunResult, json: bool) -> Result<()> {
    let stats = serde_json::json!({
        "wallMs": r.stats.wall.as_secs_f64() * 1000.0,
        "guestMs": r.stats.guest.as_secs_f64() * 1000.0,
        "fuelUsed": r.stats.fuel_used,
        "hostCalls": r.stats.host_calls,
        "memoryBytes": r.stats.memory_bytes,
    });
    match &r.outcome {
        Outcome::Completed(v) => {
            if json {
                let logs: Vec<_> = r.logs.iter().map(|l| serde_json::json!({"level": l.level, "message": l.message})).collect();
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "status": "completed", "value": v.to_json(), "logs": logs, "stats": stats }))?);
            } else {
                match v.to_json() {
                    serde_json::Value::String(s) => println!("{s}"),
                    serde_json::Value::Null if v.is_undefined() => {}
                    j => println!("{}", serde_json::to_string_pretty(&j)?),
                }
            }
            Ok(())
        }
        Outcome::Failed(e) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "status": "failed", "kind": format!("{:?}", e.kind), "name": e.name, "message": e.message, "stack": e.stack, "stats": stats,
                }))?);
            } else {
                eprintln!("\x1b[31m{}\x1b[0m: {}", e.name, e.message);
                if let Some(st) = &e.stack {
                    eprintln!("{}", st.trim_end());
                }
            }
            std::process::exit(1);
        }
        Outcome::Suspended(s) => {
            let interrupts = interrupts_json(&s.interrupts);
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "status": "suspended", "interrupts": interrupts, "continuation": s.continuation, "stats": stats,
            }))?);
            std::process::exit(2);
        }
    }
}
