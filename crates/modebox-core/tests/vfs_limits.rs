//! Staged changes and scratch space can't grow host memory without bound.
use std::path::PathBuf;
use std::sync::OnceLock;

use modebox_core::vfs::{Vfs, VfsConfig};
use modebox_core::*;

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine::new().expect("engine"))
}

fn workspace(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("vfs-limits-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

async fn run(v: &Vfs, code: &str) -> serde_json::Value {
    let mut t = Toolbox::new();
    v.install(&mut t, "fs");
    match engine().run(code, &t, RunOptions::default()).await.outcome {
        Outcome::Completed(v) => v.to_json(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn staged_changes_are_capped_and_old_versions_freed() {
    let root = workspace("staged");
    let mut cfg = VfsConfig::workspace(&root).unwrap();
    cfg.max_staged_bytes = 1000;
    let v = Vfs::new(cfg).unwrap();
    let r = run(&v, r#"
        const code = async f => { try { await f(); return 'ok' } catch (e) { return e.code } };
        [
            await code(() => fs.put({ key: 'a.txt', body: 'x'.repeat(600) })),
            await code(() => fs.put({ key: 'b.txt', body: 'y'.repeat(600) })),   // 1200 > 1000
            await code(() => fs.put({ key: 'a.txt', body: 'z'.repeat(900) })),   // replaces a.txt: 900
            await code(() => fs.put({ key: 'a.txt', body: 'w'.repeat(500) })),   // 500
            await code(() => fs.put({ key: 'b.txt', body: 'y'.repeat(400) })),   // 900
            await fs.discard(),
            await code(() => fs.put({ key: 'b.txt', body: 'y'.repeat(1000) })),  // fits after discard
        ]
    "#).await;
    assert_eq!(r, serde_json::json!(["ok", "ENOSPC", "ok", "ok", "ok", 2, "ok"]));
    assert_eq!(v.staged_bytes(), 1000);
    // Overwritten and discarded versions don't linger in memory.
    assert_eq!(v.held_bytes(), 1000);
    assert!(!root.join("a.txt").exists() && !root.join("b.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn scratch_space_is_capped() {
    let v = Vfs::new(VfsConfig::workspace(workspace("scratch")).unwrap()).unwrap();
    let r = run(&v, r#"
        const big = 'x'.repeat(16 << 20);
        let written = 0, error;
        try { for (let i = 0; i < 8; i++) { await fs.put({ key: `tmp/chunk${i}`, body: big }); written++ } }
        catch (e) { error = e.code }
        [written, error, (await fs.get({ key: 'tmp/chunk0', range: { start: 0, end: 3 } }))]
    "#).await;
    // 64 MiB of scratch: four 16 MiB chunks fit, the fifth doesn't.
    assert_eq!(r, serde_json::json!([4, "ENOSPC", "xxx"]));
}
