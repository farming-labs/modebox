use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("resume-{name}-{}", std::process::id()));
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
fn resume_decides_each_call() {
    let dir = scratch("per-call");
    let notes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/notes-mcp.mjs");
    let config = serde_json::json!({
        "mcpServers": { "notes": { "command": "node", "args": [notes] } },
        "approve": ["notes.delete_*"],
        "shell": { "enabled": false }
    });
    std::fs::write(dir.join("modebox.json"), config.to_string()).unwrap();

    let code = "const rs = await Promise.allSettled([notes.delete_note({ id: 1 }), notes.delete_note({ id: 2 })]); rs.map(r => r.status)";
    let v = json(&modebox(&dir).args(["run", "--json", "-e", code]).output().unwrap());
    assert_eq!(v["status"], "suspended", "{v}");
    let token = v["continuation"].as_str().unwrap();
    let first = v["interrupts"][0]["id"].as_u64().unwrap().to_string();

    // Inspecting shows both calls and leaves the token usable.
    let info = json(&modebox(&dir).args(["inspect", token]).output().unwrap());
    let args: Vec<_> = info["interrupts"].as_array().unwrap().iter().map(|i| i["args"][0]["id"].clone()).collect();
    assert_eq!(args, vec![serde_json::json!(1), serde_json::json!(2)]);

    let bad = modebox(&dir).args(["resume", token, "--approve-only", "999"]).output().unwrap();
    assert!(!bad.status.success() && String::from_utf8_lossy(&bad.stderr).contains("isn't pending"));

    let r = json(&modebox(&dir).args(["resume", token, "--approve-only", &first]).output().unwrap());
    assert_eq!(r["value"], serde_json::json!(["fulfilled", "rejected"]), "{r}");
    std::fs::remove_dir_all(&dir).unwrap();
}
