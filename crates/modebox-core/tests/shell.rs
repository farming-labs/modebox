#![cfg(unix)]
use std::path::PathBuf;
use std::time::{Duration, Instant};

use modebox_core::shell::{ShellPolicy, sandbox_status, shell_tool};
use modebox_core::*;

/// Whether the OS sandbox can run here. Tests skip without one, unless
/// MODEBOX_REQUIRE_SANDBOX is set (CI sets it so a broken sandbox fails).
fn sandboxed() -> bool {
    match sandbox_status() {
        Ok(_) => true,
        Err(why) if std::env::var_os("MODEBOX_REQUIRE_SANDBOX").is_some() => panic!("no OS sandbox: {why}"),
        Err(why) => {
            eprintln!("skipping: no OS sandbox: {why}");
            false
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("modebox-shell-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p.canonicalize().unwrap()
}

async fn sh(policy: ShellPolicy, code: &str) -> Value {
    let engine = Engine::new().unwrap();
    let tools = Toolbox::new().with(shell_tool("sh", policy));
    let r = engine.run(code, &tools, RunOptions::default()).await;
    match r.outcome {
        Outcome::Completed(v) => v,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn runs_commands_and_captures_output() {
    if !sandboxed() { return; }
    let root = scratch("basic");
    let v = sh(ShellPolicy::new(&root), "const r = await sh('echo hello; echo oops 1>&2; exit 3'); r").await;
    assert_eq!(v.get("stdout"), Some(&Value::from("hello\n")));
    assert_eq!(v.get("stderr"), Some(&Value::from("oops\n")));
    assert_eq!(v.get("exitCode"), Some(&Value::from(3)));
}

#[tokio::test]
async fn writes_are_confined_to_the_workspace() {
    if !sandboxed() { return; }
    let root = scratch("writes");
    // Not under the system temp dir, which the policy leaves writable.
    let outside = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("outside-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    let outside = outside.canonicalize().unwrap();
    let code = format!(
        "const a = await sh('echo ok > inside.txt && cat inside.txt');\n\
         const b = await sh('echo pwned > {}/escaped.txt');\n\
         [a.stdout, a.exitCode, b.exitCode !== 0]",
        outside.display()
    );
    let v = sh(ShellPolicy::new(&root), &code).await;
    assert_eq!(v.to_json(), serde_json::json!(["ok\n", 0, true]));
    assert!(root.join("inside.txt").exists());
    assert!(!outside.join("escaped.txt").exists(), "write outside the workspace must fail");
}

#[tokio::test]
async fn secret_dirs_are_unreadable() {
    if !sandboxed() { return; }
    let root = scratch("secrets-root");
    let secrets = scratch("secrets");
    std::fs::write(secrets.join("id_rsa"), "PRIVATE KEY").unwrap();
    let mut policy = ShellPolicy::new(&root);
    policy.deny_read = vec![secrets.clone()];
    let v = sh(policy, &format!("(await sh('cat {}/id_rsa')).stdout", secrets.display())).await;
    assert_eq!(v, Value::from(""), "secret leaked: {v:?}");
}

#[tokio::test]
async fn cwd_cannot_escape_the_workspace() {
    if !sandboxed() { return; }
    let root = scratch("cwd");
    let v = sh(ShellPolicy::new(&root), "try { await sh({ command: 'pwd', cwd: '../..' }) } catch (e) { e.code }").await;
    assert_eq!(v, Value::from("EACCES"));
}

#[tokio::test]
async fn network_is_blocked_by_default() {
    if !sandboxed() { return; }
    let root = scratch("net");
    let v = sh(ShellPolicy::new(&root), "(await sh('curl -sS -m 3 -o /dev/null https://example.com; echo $?')).stdout.trim()").await;
    assert_ne!(v, Value::from("0"), "network request should fail inside the sandbox");
}

#[tokio::test]
async fn timeouts_kill_the_process_group() {
    if !sandboxed() { return; }
    let root = scratch("timeout");
    let mut policy = ShellPolicy::new(&root);
    policy.timeout = Duration::from_millis(300);
    let t = Instant::now();
    let v = sh(policy, "(await sh('sleep 30 & sleep 30; echo never')).timedOut").await;
    assert_eq!(v, Value::Bool(true));
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn seccomp_refuses_new_namespaces() {
    if !sandboxed() { return; }
    let root = scratch("seccomp");
    let v = sh(
        ShellPolicy::new(&root),
        "const a = await sh('unshare --user true'); const b = await sh('echo ok; (sleep 0 &); wait'); [a.exitCode, a.stderr, b.stdout]",
    )
    .await;
    let v = v.to_json();
    assert_ne!(v[0], serde_json::json!(0), "{v}");
    assert!(v[1].as_str().unwrap_or("").contains("not implemented"), "{v}");
    assert_eq!(v[2], serde_json::json!("ok\n"), "ordinary commands still work: {v}");
}
