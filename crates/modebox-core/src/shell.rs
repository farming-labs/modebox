//! Tier 2: shell commands inside an OS sandbox, exposed to code as `sh()`.
//!
//! The JS sandbox can't touch the machine at all; this tool is the controlled
//! way to let it. Commands run under Seatbelt (macOS) or bubblewrap (Linux)
//! with: reads allowed except secret stores, writes only under the workspace
//! and temp dirs, no network unless enabled, a scrubbed environment, a
//! timeout that kills the whole process group, and capped output. On Linux a
//! seccomp filter also refuses syscalls commands never need (see `seccomp`).

#![allow(clippy::result_large_err)] // ToolError carries messages; boxing it buys nothing here.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
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
    sandbox_status().ok()
}

/// The sandbox in use, or why there is none. Probed once per process by
/// starting a trivial confined command, so a sandbox that is installed but
/// can't start (no user namespaces, already inside a sandbox) is reported
/// here rather than on every command.
pub fn sandbox_status() -> Result<&'static str, String> {
    sandbox().as_ref().map(|s| s.kind.name()).map_err(Clone::clone)
}

struct Sandbox {
    kind: Kind,
    bin: PathBuf,
}

#[derive(Clone, Copy)]
enum Kind {
    Seatbelt,
    Bubblewrap,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Seatbelt => "seatbelt",
            Kind::Bubblewrap => "bubblewrap",
        }
    }
}

fn sandbox() -> &'static Result<Sandbox, String> {
    static SANDBOX: OnceLock<Result<Sandbox, String>> = OnceLock::new();
    SANDBOX.get_or_init(probe)
}

fn probe() -> Result<Sandbox, String> {
    if cfg!(target_os = "macos") {
        let bin = PathBuf::from("/usr/bin/sandbox-exec");
        if !bin.exists() {
            return Err("macOS sandbox-exec was not found at /usr/bin/sandbox-exec".into());
        }
        // The real base profile: inside another sandbox only trivial profiles apply.
        let mut probe = std::process::Command::new(&bin);
        probe.args(["-p", SEATBELT_BASE, "/usr/bin/true"]);
        try_start(&mut probe)
            .map_err(|e| format!("sandbox-exec can't start a sandbox here ({e}); modebox may already be running inside one"))?;
        Ok(Sandbox { kind: Kind::Seatbelt, bin })
    } else if cfg!(target_os = "linux") {
        let bin = which("bwrap").ok_or("bubblewrap (bwrap) is not installed; install the bubblewrap package")?;
        let mut probe = std::process::Command::new(&bin);
        probe.args(["--ro-bind", "/", "/", "--unshare-pid", "--unshare-net", "--die-with-parent"]);
        #[cfg(target_os = "linux")]
        let _filter = seccomp::attach(&mut probe)?;
        probe.args(["--", "/bin/true"]);
        try_start(&mut probe)
            .map_err(|e| {
                format!(
                    "bubblewrap is installed but can't create a sandbox ({e}). It needs unprivileged user namespaces: \
                     Ubuntu 23.10 and later block them unless bwrap has an AppArmor profile that allows `userns` \
                     (see \"Linux\" in the modebox README), and some containers disable them entirely"
                )
            })?;
        Ok(Sandbox { kind: Kind::Bubblewrap, bin })
    } else {
        Err("no OS sandbox is supported on this platform (needs macOS Seatbelt or Linux bubblewrap)".into())
    }
}

/// Run a sandbox command once; Err holds its stderr (or the spawn error) when it fails.
fn try_start(cmd: &mut std::process::Command) -> Result<(), String> {
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    Err(if stderr.is_empty() { out.status.to_string() } else { stderr })
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

    // `_keep` holds anything the sandbox reads at startup (the seccomp filter).
    let (mut cmd, _keep) = confine(policy, &root, &cwd, command)?;
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

/// What a confined command needs kept open until it has started.
type Keep = Option<std::io::PipeReader>;

fn confine(policy: &ShellPolicy, root: &Path, cwd: &Path, command: &str) -> Result<(Command, Keep), ToolError> {
    let tmp = canon(&std::env::temp_dir());
    let mut writable = vec![root.to_path_buf(), tmp.clone(), canon(Path::new("/tmp"))];
    writable.extend(policy.writable.iter().map(|p| canon(p)));
    let deny: Vec<PathBuf> = policy.deny_read.iter().map(|p| canon(p)).collect();

    match sandbox() {
        Ok(Sandbox { kind: Kind::Seatbelt, bin }) => {
            let mut profile = String::from(SEATBELT_BASE);
            let mut cmd = Command::new(bin);
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
            Ok((cmd, None))
        }
        Ok(Sandbox { kind: Kind::Bubblewrap, bin }) => {
            let mut cmd = Command::new(bin);
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
            #[cfg(target_os = "linux")]
            let keep = seccomp::attach(cmd.as_std_mut()).map_err(|e| ToolError::new(format!("sh: {e}")))?;
            #[cfg(not(target_os = "linux"))]
            let keep = None;
            cmd.arg("--chdir").arg(cwd).args(["--", "/bin/sh", "-c", command]);
            Ok((cmd, keep))
        }
        _ if policy.allow_unsandboxed => {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg(command);
            Ok((cmd, None))
        }
        Err(why) => Err(ToolError::new(format!("sh: no OS sandbox is available: {why}")).with_code("ENOSYS")),
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

/// A seccomp filter for bubblewrap. bwrap already drops privileges and
/// isolates namespaces; this also refuses syscalls that sandboxed commands
/// never need and that widen the kernel attack surface: namespace and mount
/// administration, tracing other processes, BPF, io_uring, keyrings, kernel
/// modules. Modeled on Docker's default profile.
///
/// Refused calls fail with ENOSYS, as if the kernel didn't have them. That is
/// required for `clone3`, whose flags live in memory a filter can't read:
/// on ENOSYS, libc falls back to `clone`, where new namespaces are refused by
/// flag.
#[cfg(target_os = "linux")]
mod seccomp {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::sync::OnceLock;

    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter, SeccompRule,
        TargetArch,
    };

    const REFUSED: &[libc::c_long] = &[
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_open_tree,
        libc::SYS_move_mount,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fspick,
        libc::SYS_mount_setattr,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_clone3,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_userfaultfd,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_kexec_load,
        libc::SYS_kexec_file_load,
        libc::SYS_reboot,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_acct,
        libc::SYS_quotactl,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        libc::SYS_syslog,
    ];

    /// `clone` flags that create namespaces.
    const NEW_NAMESPACES: &[libc::c_int] = &[
        libc::CLONE_NEWUSER,
        libc::CLONE_NEWNS,
        libc::CLONE_NEWNET,
        libc::CLONE_NEWPID,
        libc::CLONE_NEWIPC,
        libc::CLONE_NEWUTS,
        libc::CLONE_NEWCGROUP,
    ];

    /// The compiled filter, as bwrap reads it: raw `sock_filter` structs.
    #[allow(clippy::unnecessary_cast)] // syscall numbers are c_long, which is i32 on 32-bit targets
    fn program() -> Result<&'static [u8], String> {
        static PROGRAM: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
        PROGRAM
            .get_or_init(|| {
                let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|e| format!("seccomp: {e:?}"))?;
                let mut rules: BTreeMap<i64, Vec<SeccompRule>> = REFUSED.iter().map(|&nr| (nr as i64, Vec::new())).collect();
                // clone's flags are its first argument on x86_64 and aarch64.
                let clone = NEW_NAMESPACES
                    .iter()
                    .map(|&flag| {
                        let flag = flag as u64;
                        let cond = SeccompCondition::new(0, SeccompCmpArgLen::Qword, SeccompCmpOp::MaskedEq(flag), flag)
                            .map_err(|e| format!("seccomp: {e}"))?;
                        SeccompRule::new(vec![cond]).map_err(|e| format!("seccomp: {e}"))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                rules.insert(libc::SYS_clone as i64, clone);
                let filter = SeccompFilter::new(rules, SeccompAction::Allow, SeccompAction::Errno(libc::ENOSYS as u32), arch)
                    .map_err(|e| format!("seccomp: {e}"))?;
                let program: BpfProgram = filter.try_into().map_err(|e| format!("seccomp: {e:?}"))?;
                Ok(program.iter().flat_map(|f| {
                    let mut b = Vec::with_capacity(8);
                    b.extend_from_slice(&f.code.to_ne_bytes());
                    b.push(f.jt);
                    b.push(f.jf);
                    b.extend_from_slice(&f.k.to_ne_bytes());
                    b
                }).collect())
            })
            .as_deref()
            .map_err(Clone::clone)
    }

    /// Add `--seccomp FD` to a bwrap command line. The returned pipe must stay
    /// open until the command has started.
    pub fn attach(cmd: &mut std::process::Command) -> Result<Option<std::io::PipeReader>, String> {
        let program = program()?;
        let (reader, mut writer) = std::io::pipe().map_err(|e| format!("seccomp: {e}"))?;
        // A few hundred bytes: well under a pipe's buffer, so this can't block.
        writer.write_all(program).map_err(|e| format!("seccomp: {e}"))?;
        drop(writer);
        let fd = reader.as_raw_fd();
        cmd.arg("--seccomp").arg(fd.to_string());
        // The pipe is close-on-exec; keep it open in bwrap only.
        unsafe {
            cmd.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(Some(reader))
    }
}
