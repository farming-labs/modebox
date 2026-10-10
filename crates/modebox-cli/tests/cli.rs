use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `modebox` with its key, state and config all inside `dir`.
fn modebox(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_modebox"));
    c.current_dir(dir)
        .env_remove("MODEBOX_SECRET")
        .env("MODEBOX_SECRET_FILE", dir.join("secret"))
        .env("MODEBOX_STATE_DIR", dir.join("state"))
        .arg("-c")
        .arg(dir.join("modebox.json"));
    c
}

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn a_token_resumes_once_across_processes() {
    let dir = scratch("single-use");
    // `touch` isn't on the default allow list, so it suspends for approval.
    std::fs::write(dir.join("modebox.json"), r#"{ "shell": { "enabled": true } }"#).unwrap();

    let run = modebox(&dir).args(["run", "--json", "-e", "try { await sh('touch x') } catch (e) { e.name }"]).output().unwrap();
    let v = json(&run);
    assert_eq!(v["status"], "suspended", "{v}");
    let token = v["continuation"].as_str().unwrap();

    // Each resume is a new process; the deny still completes the run.
    let first = json(&modebox(&dir).args(["resume", token]).output().unwrap());
    assert_eq!(first["status"], "completed", "{first}");
    assert_eq!(first["value"], "PermissionDenied");

    let again = modebox(&dir).args(["resume", token]).output().unwrap();
    assert!(!again.status.success());
    let again = json(&again);
    assert_eq!(again["status"], "failed", "{again}");
    assert!(again["message"].as_str().unwrap().contains("already used"), "{again}");
    std::fs::remove_dir_all(&dir).unwrap();
}
