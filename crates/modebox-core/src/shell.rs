//! Tier 2: shell commands inside an OS sandbox, exposed to code as `sh()`.
//!
//! The JS sandbox can't touch the machine at all; this tool is the controlled
//! way to let it. Commands run under Seatbelt (macOS) or bubblewrap (Linux)
//! with: reads allowed except secret stores, writes only under the workspace
//! and temp dirs, no network unless enabled, a scrubbed environment, a
//! timeout that kills the whole process group, and capped output.

#![allow(clippy::result_large_err)] // ToolError carries messages; boxing it buys nothing here.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::tools::{Call, Tool, ToolError};
use crate::value::Value;

#[derive(Clone, Debug)]
pub struct ShellPolicy {
    /// Working directory; relative `cwd` options resolve against it and must stay inside.
    pub root: PathBuf,
    /// Extra writable directories (the root and temp dirs are always writable).
    pub writable: Vec<PathBuf>,
    /// Directories that can't even be read (defaults cover common secret stores).
    pub deny_read: Vec<PathBuf>,
    pub network: bool,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    /// Environment variables passed through from the host, besides the basics.
    pub pass_env: Vec<String>,
    /// Run without an OS sandbox when none is available. Off by default.
    pub allow_unsandboxed: bool,
}

impl ShellPolicy {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let deny_read = home
            .map(|h| {
                [".ssh", ".aws", ".gnupg", ".config/gcloud", ".kube", ".docker", ".netrc", ".config/gh", ".azure", ".npmrc"]
                    .iter()
                    .map(|p| h.join(p))
                    .collect()
            })
            .unwrap_or_default();
        ShellPolicy {
            root: root.into(),
            writable: Vec::new(),
            deny_read,
            network: false,
            timeout: Duration::from_secs(60),
            max_output_bytes: 256 << 10,
            pass_env: Vec::new(),
            allow_unsandboxed: false,
        }
    }
}

/// Which OS mechanism will confine commands on this machine, if any.
pub fn sandbox_kind() -> Option<&'static str> {
    if cfg!(target_os = "macos") && Path::new("/usr/bin/sandbox-exec").exists() {
        Some("seatbelt")
    } else if cfg!(target_os = "linux") && which("bwrap").is_some() {
        Some("bubblewrap")
    } else {
        None
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?.to_str()?.split(':').map(|d| Path::new(d).join(bin)).find(|p| p.is_file())
}

/// `sh(command)` / `sh({ command, cwd?, timeoutMs?, stdin? })` →
/// `{ stdout, stderr, exitCode, timedOut, truncated }`.
pub fn shell_tool(name: &str, policy: ShellPolicy) -> Tool {
    let description = format!(
        "Run a shell command in an OS sandbox ({}). Writes are limited to the workspace and temp dirs; network is {}. Returns {{ stdout, stderr, exitCode, timedOut, truncated }}.",
        sandbox_kind().unwrap_or("none available"),
        if policy.network { "allowed" } else { "blocked" },
    );
    Tool::new(name, move |call: Call| {
        let policy = policy.clone();
        async move { run_command(&policy, &call).await }
    })
    .description(description)
    .input_schema(serde_json::json!({
        "anyOf": [
            { "type": "string", "description": "the command line" },
            { "type": "object", "properties": {
                "command": { "type": "string" },
                "cwd": { "type": "string", "description": "relative to the workspace root" },
                "timeoutMs": { "type": "number" },
                "stdin": { "type": "string" }
            }, "required": ["command"] }
        ]
    }))
    .output_schema(serde_json::json!({
        "type": "object",
        "properties": {
            "stdout": { "type": "string" }, "stderr": { "type": "string" }, "exitCode": { "type": ["number", "null"] },
            "timedOut": { "type": "boolean" }, "truncated": { "type": "boolean" }
        },
        "required": ["stdout", "stderr", "exitCode", "timedOut", "truncated"]
    }))
}

/// The command string of a `sh` call, for approval policies.
pub fn command_of(call: &Call) -> Option<&str> {
    match call.input() {
        Value::String(s) => Some(s),
        v => v.get("command").and_then(Value::as_str),
    }
}

async fn run_command(policy: &ShellPolicy, call: &Call) -> Result<Value, ToolError> {
    let input = call.input();
    let command = command_of(call).ok_or_else(|| ToolError::new("sh: expected a command string").with_name("TypeError"))?;
    let root = policy.root.canonicalize().map_err(|e| ToolError::new(format!("sh: workspace root: {e}")))?;
    let cwd = match input.get("cwd").and_then(Value::as_str) {
        Some(rel) => {
            let p = root.join(rel).canonicalize().map_err(|e| ToolError::new(format!("sh: cwd {rel}: {e}")))?;
            if !p.starts_with(&root) {
                return Err(ToolError::new(format!("sh: cwd {rel} is outside the workspace")).with_code("EACCES"));
            }
            p
        }
        None => root.clone(),
    };
    let timeout = input
        .get("timeoutMs")
        .and_then(Value::as_f64)
        .map(|ms| Duration::from_millis(ms.max(1.0) as u64).min(policy.timeout))
        .unwrap_or(policy.timeout);
    let stdin = input.get("stdin").and_then(Value::as_str).map(str::to_owned);

    let mut cmd = confine(policy, &root, &cwd, command)?;
    cmd.current_dir(&cwd)
        .env_clear()
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    for key in ["PATH", "HOME", "USER", "LANG", "LC_ALL", "SHELL"].iter().map(|s| s.to_string()).chain(policy.pass_env.iter().cloned()) {
        if let Ok(v) = std::env::var(&key) {
            cmd.env(&key, v);
        }
    }
    cmd.env("TERM", "dumb").env("TMPDIR", std::env::temp_dir()).env("MODEBOX_SANDBOX", sandbox_kind().unwrap_or("none"));

    let mut child = cmd.spawn().map_err(|e| ToolError::new(format!("sh: failed to start: {e}")))?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = pipe.write_all(data.as_bytes()).await;
        });
    }
    let max = policy.max_output_bytes;
    let out = tokio::spawn(read_capped(child.stdout.take().unwrap(), max));
    let err = tokio::spawn(read_capped(child.stderr.take().unwrap(), max));
    let pid = child.id();

    let (status, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(s) => (s.ok(), false),
        Err(_) => {
            kill_group(pid);
            let _ = child.kill().await;
            (None, true)
        }
    };
    let (stdout, t1) = out.await.unwrap_or_default();
    let (stderr, t2) = err.await.unwrap_or_default();
    Ok(Value::object([
        ("stdout", Value::from(stdout)),
        ("stderr", Value::from(stderr)),
        ("exitCode", status.and_then(|s| s.code()).map(Value::from).unwrap_or(Value::Null)),
        ("timedOut", Value::from(timed_out)),
        ("truncated", Value::from(t1 || t2)),
    ]))
}

async fn read_capped(mut r: impl AsyncRead + Unpin, max: usize) -> (String, bool) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = max.saturating_sub(buf.len());
                if n > room {
                    truncated = true;
                }
                buf.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
    (String::from_utf8_lossy(&buf).into_owned(), truncated)
}

#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // SAFETY: plain syscall; the group was created for this child.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn confine(policy: &ShellPolicy, root: &Path, cwd: &Path, command: &str) -> Result<Command, ToolError> {
    let tmp = canon(&std::env::temp_dir());
    let mut writable = vec![root.to_path_buf(), tmp.clone(), canon(Path::new("/tmp"))];
    writable.extend(policy.writable.iter().map(|p| canon(p)));
    let deny: Vec<PathBuf> = policy.deny_read.iter().map(|p| canon(p)).collect();

    match sandbox_kind() {
        Some("seatbelt") => {
            let mut profile = String::from(SEATBELT_BASE);
            let mut cmd = Command::new("/usr/bin/sandbox-exec");
            let mut params = Vec::new();
            profile.push_str("(allow file-write*\n");
            for (i, w) in writable.iter().enumerate() {
                profile.push_str(&format!("  (subpath (param \"W{i}\"))\n"));
                params.push((format!("W{i}"), w.clone()));
            }
            profile.push_str(")\n");
            if !deny.is_empty() {
                profile.push_str("(deny file-read* file-write*\n");
                for (i, d) in deny.iter().enumerate() {
                    profile.push_str(&format!("  (subpath (param \"D{i}\"))\n"));
                    params.push((format!("D{i}"), d.clone()));
                }
                profile.push_str(")\n");
            }
            if policy.network {
                profile.push_str(SEATBELT_NETWORK);
            }
            cmd.arg("-p").arg(profile);
            for (k, v) in params {
                cmd.arg("-D").arg(format!("{k}={}", v.display()));
            }
            cmd.arg("/bin/sh").arg("-c").arg(command);
            Ok(cmd)
        }
        Some("bubblewrap") => {
            let mut cmd = Command::new(which("bwrap").unwrap());
            cmd.args(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc"]);
            for w in &writable {
                if w.exists() {
                    cmd.arg("--bind").arg(w).arg(w);
                }
            }
            for d in &deny {
                if d.is_dir() {
                    cmd.arg("--tmpfs").arg(d);
                } else if d.exists() {
                    cmd.arg("--ro-bind").arg("/dev/null").arg(d);
                }
            }
            cmd.args(["--unshare-pid", "--unshare-ipc", "--unshare-uts", "--die-with-parent", "--new-session"]);
            if !policy.network {
                cmd.arg("--unshare-net");
            }
            cmd.arg("--chdir").arg(cwd).args(["--", "/bin/sh", "-c", command]);
            Ok(cmd)
        }
        _ if policy.allow_unsandboxed => {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg(command);
            Ok(cmd)
        }
        _ => Err(ToolError::new(
            "sh: no OS sandbox is available on this machine (needs macOS sandbox-exec or Linux bwrap)",
        )
        .with_code("ENOSYS")),
    }
}

/// Seatbelt base profile. Deny by default; allow process management,
/// reading the filesystem, and the handful of system services ordinary CLI
/// tools need. Modeled on the policies used by Chromium and OpenAI Codex.
const SEATBELT_BASE: &str = r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow file-read*)
(allow file-write* file-ioctl (literal "/dev/null") (literal "/dev/zero") (literal "/dev/dtracehelper") (literal "/dev/tty"))
(allow pseudo-tty)
(allow ipc-posix-sem)
(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo")
  (global-name "com.apple.system.logger")
  (global-name "com.apple.bsd.dirhelper"))
"#;

const SEATBELT_NETWORK: &str = r#"(allow network-outbound)
(allow network-inbound (local ip "localhost:*"))
(allow system-socket)
(allow mach-lookup
  (global-name "com.apple.SecurityServer")
  (global-name "com.apple.networkd")
  (global-name "com.apple.ocspd")
  (global-name "com.apple.trustd.agent")
  (global-name "com.apple.SystemConfiguration.DNSConfiguration")
  (global-name "com.apple.SystemConfiguration.configd")
  (global-name "com.apple.dnssd.service"))
"#;
