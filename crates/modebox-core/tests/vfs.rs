use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use std::sync::Arc;

use modebox_core::store::{Bytes, Key, PutOptions, Store};
use modebox_core::vfs::{Mount, MountMode, Vfs, VfsConfig};
use modebox_core::*;

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine::new().expect("engine"))
}

/// A small project on disk, plus a secret outside it and a symlink pointing there.
fn project(name: &str) -> (PathBuf, PathBuf) {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("vfs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("project");
    let outside = base.join("outside");
    for d in ["src", "data", "build"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::create_dir_all(&outside).unwrap();
    let w = |p: &str, s: &[u8]| std::fs::write(root.join(p), s).unwrap();
    w("src/main.ts", b"import { oldApi } from './util';\nexport const x = oldApi(1);\n");
    w("src/util.ts", b"export function oldApi(n: number) {\n  return n * 2;\n}\n");
    w("README.md", b"# demo\n");
    w("data/users.json", br#"[{"name":"ada","age":36},{"name":"alan","age":41}]"#);
    w(".gitignore", b"build/\n");
    w("build/out.js", b"compiled");
    w(".env", b"SECRET=hunter2");
    w("logo.png", &[0x89, b'P', b'N', b'G', 0xff, 0x00, 0xfe]);
    std::fs::write(outside.join("secret.txt"), "top secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    (root, outside)
}

fn tools(v: &Vfs) -> Toolbox {
    let mut t = Toolbox::new();
    v.install(&mut t, "fs");
    t
}

async fn run(t: &Toolbox, code: &str) -> RunResult {
    engine().run(code, t, RunOptions::default()).await
}

fn ok(r: &RunResult) -> serde_json::Value {
    match &r.outcome {
        Outcome::Completed(v) => v.to_json(),
        other => panic!("{other:?}"),
    }
}

fn read(root: &Path, p: &str) -> String {
    std::fs::read_to_string(root.join(p)).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_real_files_as_objects() {
    let (root, _) = project("list");
    let v = Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap();
    let t = tools(&v);
    let all = ok(&run(&t, "(await fs.list()).objects.map(o => o.key)").await);
    assert_eq!(all, serde_json::json!(["README.md", "data/users.json", "logo.png", "src/main.ts", "src/util.ts"]),
        "gitignored, hidden and denied files are not listed");
    let top = ok(&run(&t, "const r = await fs.list({ recursive: false }); [r.objects.map(o => o.key), r.prefixes]").await);
    // `link` (a symlink pointing outside the workspace) is not followed or listed.
    assert_eq!(top, serde_json::json!([["README.md", "logo.png"], ["data/", "src/", "tmp/"]]));
    let ts = ok(&run(&t, "const r = await fs.list({ glob: '**/*.ts' }); r.objects.map(o => [o.key, o.size, o.modified instanceof Date])").await);
    assert_eq!(ts[0][0], "src/main.ts");
    assert_eq!(ts[0][2], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_text_json_and_bytes() {
    let (root, _) = project("read");
    let t = tools(&Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap());
    let r = ok(&run(&t, r#"
        const users = await fs.get({ key: 'data/users.json', as: 'json' });
        const logo = await fs.get('logo.png');
        [users.filter(u => u.age > 40).map(u => u.name), logo instanceof Uint8Array, logo.length,
         await fs.get('nope.txt'), typeof (await fs.head('README.md')).etag]
    "#).await);
    assert_eq!(r, serde_json::json!([["alan"], true, 7, null, "string"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn cannot_escape_or_read_secrets() {
    let (root, _) = project("escape");
    let t = tools(&Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap());
    let code = r#"
        const tryGet = async (k) => { try { return await fs.get(k) } catch (e) { return e.code } };
        [await tryGet('../outside/secret.txt'), await tryGet('link/secret.txt'), await tryGet('.env'),
         await tryGet('/etc/passwd'), await tryGet('.git/config')]
    "#;
    let r = ok(&run(&t, code).await);
    // '/etc/passwd' is a key relative to the root, i.e. <root>/etc/passwd, which doesn't exist.
    assert_eq!(r, serde_json::json!(["EACCES", "EACCES", "EACCES", null, "EACCES"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_are_staged_then_committed_with_approval() {
    let (root, _) = project("commit");
    let v = Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap();
    let t = tools(&v);
    let code = r#"
        const hits = await fs.grep({ pattern: 'oldApi', glob: 'src/**' });
        for (const key of new Set(hits.map(h => h.key))) {
            const src = await fs.get(key);
            await fs.put({ key, body: src.replaceAll('oldApi', 'newApi') });
        }
        await fs.put({ key: 'CHANGELOG.md', body: '- renamed oldApi to newApi\n' });
        const pending = (await fs.changes()).map(c => `${c.status} ${c.key}`);
        const result = await fs.commit({ message: 'rename oldApi' });
        ({ hits: hits.length, pending, applied: result.applied.map(a => a.key), conflicts: result.conflicts })
    "#;
    let first = run(&t, code).await;
    let s = first.suspension().expect("commit asks for approval").clone();
    assert_eq!(s.interrupts[0].tool, "fs.commit");
    let reason = s.interrupts[0].payload.get("reason").and_then(Value::as_str).unwrap().to_owned();
    assert!(reason.contains("M src/main.ts") && reason.contains("A CHANGELOG.md"), "{reason}");
    assert!(read(&root, "src/util.ts").contains("oldApi"), "nothing on disk before approval");

    let done = engine().resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &t, RunOptions::default()).await;
    let r = ok(&done);
    assert_eq!(r["hits"], 3);
    assert_eq!(r["pending"], serde_json::json!(["added CHANGELOG.md", "modified src/main.ts", "modified src/util.ts"]));
    assert_eq!(r["applied"], serde_json::json!(["CHANGELOG.md", "src/main.ts", "src/util.ts"]));
    assert!(read(&root, "src/util.ts").contains("newApi"));
    assert_eq!(read(&root, "CHANGELOG.md"), "- renamed oldApi to newApi\n");
    assert!(v.changes().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_refuses_files_changed_on_disk_meanwhile() {
    let (root, _) = project("conflict");
    let v = Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap();
    let t = tools(&v);
    ok(&run(&t, "const s = await fs.get('README.md'); await fs.put({ key: 'README.md', body: s + 'agent line\\n' })").await);
    std::fs::write(root.join("README.md"), "# edited by a human\n").unwrap();
    let first = run(&t, "await fs.commit()").await;
    let s = first.suspension().unwrap().clone();
    let r = ok(&engine().resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &t, RunOptions::default()).await);
    assert_eq!(r["applied"], serde_json::json!([]));
    assert_eq!(r["conflicts"][0]["key"], "README.md");
    assert_eq!(read(&root, "README.md"), "# edited by a human\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_and_scratch_mounts() {
    let (root, _) = project("mounts");
    let cfg = VfsConfig::new(vec![Mount::dir("", &root, MountMode::ReadOnly).unwrap(), Mount::memory("tmp/")]);
    let v = Vfs::new(cfg).unwrap();
    let t = tools(&v);
    let r = ok(&run(&t, r#"
        let ro; try { await fs.put({ key: 'README.md', body: 'x' }) } catch (e) { ro = e.code }
        await fs.put({ key: 'tmp/notes.json', body: { step: 1 } });
        [ro, (await fs.get({ key: 'tmp/notes.json', as: 'json' })).step, (await fs.changes()).length]
    "#).await);
    assert_eq!(r, serde_json::json!(["EROFS", 1, 0]));
    assert!(!root.join("tmp").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn staged_view_is_consistent_for_list_move_delete() {
    let (root, _) = project("view");
    let v = Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap();
    let t = tools(&v);
    let r = ok(&run(&t, r#"
        await fs.move({ from: 'src/util.ts', to: 'src/lib/util.ts' });
        await fs.delete('README.md');
        await fs.put({ key: 'docs/intro.md', body: 'hi' });
        const keys = (await fs.list()).objects.map(o => o.key + (o.status ? ':' + o.status : ''));
        const hits = (await fs.grep({ pattern: 'function', literal: true })).map(h => h.key + ':' + h.line);
        [keys, hits]
    "#).await);
    assert_eq!(r[0], serde_json::json!(["data/users.json", "docs/intro.md:added", "logo.png", "src/lib/util.ts:added", "src/main.ts"]));
    assert_eq!(r[1], serde_json::json!(["src/lib/util.ts:1"]));
    assert!(root.join("README.md").exists(), "still only staged");
}

#[tokio::test(flavor = "multi_thread")]
async fn staged_writes_travel_with_the_continuation() {
    let (root, _) = project("travel");
    // Process A: stage a change, then pause on commit.
    let a = Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap();
    let first = run(&tools(&a), "await fs.put({ key: 'src/new.ts', body: 'export {}\\n' }); await fs.commit()").await;
    let s = first.suspension().unwrap().clone();
    drop(a);

    // Process B: a brand-new filesystem object with nothing staged.
    let b = Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap();
    assert!(b.changes().await.is_empty());
    let t = tools(&b);
    let r = ok(&engine().resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &t, RunOptions::default()).await);
    assert_eq!(r["applied"][0]["key"], "src/new.ts");
    assert_eq!(read(&root, "src/new.ts"), "export {}\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_sessions_keep_staged_writes() {
    let (root, _) = project("session");
    let a = Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap();
    let mut s = engine().session(tools(&a), RunOptions::default());
    ok(&s.run("await fs.put({ key: 'draft.md', body: 'v1' }); const n = 1").await);
    let token = s.save().await.unwrap();

    let b = Vfs::new(VfsConfig::workspace(&root).unwrap()).unwrap();
    let mut s2 = engine().restore_session(&token, tools(&b), RunOptions::default()).await.unwrap();
    let r = ok(&s2.run("[n, await fs.get('draft.md'), (await fs.changes()).map(c => c.key)]").await);
    assert_eq!(r, serde_json::json!([1, "v1", ["draft.md"]]));
    assert!(!root.join("draft.md").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn one_namespace_over_different_backends() {
    let (root, _) = project("mixed");
    // A bucket (object_store's in-memory driver standing in for s3://…), mounted read-only.
    let bucket = modebox_core::store::objstore::ObjectStoreAdapter::from_url("memory://", std::iter::empty::<(&str, &str)>(), false).unwrap();
    for (k, v) in [("2026/10/sales.json", r#"[{"sku":"a","qty":3},{"sku":"b","qty":5}]"#), ("2026/10/notes.txt", "q4 kickoff")] {
        bucket.put(&Key::new(k).unwrap(), Bytes::from(v), PutOptions::default()).await.unwrap();
    }
    let out = Arc::new(modebox_core::store::memory::MemoryStore::new());
    let cfg = VfsConfig::new(vec![
        Mount::dir("", &root, MountMode::Staged).unwrap(),
        Mount::new("datasets/", Arc::new(bucket), MountMode::ReadOnly),
        Mount::new("out/", out.clone(), MountMode::Direct),
    ]);
    let v = Vfs::new(cfg).unwrap();
    let t = tools(&v);
    let r = ok(&run(&t, r#"
        const months = (await fs.list({ prefix: 'datasets/', recursive: false })).prefixes;
        const sales = await fs.get({ key: 'datasets/2026/10/sales.json', as: 'json' });
        const total = sales.reduce((n, s) => n + s.qty, 0);
        await fs.put({ key: 'out/report.json', body: { total } });            // direct: written now
        await fs.put({ key: 'REPORT.md', body: `total: ${total}\n` });       // staged: waits for commit
        let ro; try { await fs.put({ key: 'datasets/x', body: '' }) } catch (e) { ro = e.code }
        const mounts = (await fs.mounts()).map(m => `${m.prefix || '/'} ${m.mode} ${m.store}`);
        ({ months, total, ro, staged: (await fs.changes()).map(c => c.key), mounts })
    "#).await);
    assert_eq!(r["months"], serde_json::json!(["datasets/2026/"]));
    assert_eq!(r["total"], 8);
    assert_eq!(r["ro"], "EROFS");
    assert_eq!(r["staged"], serde_json::json!(["REPORT.md"]));
    assert!(r["mounts"][1].as_str().unwrap().starts_with("datasets/ read-only memory://"), "{}", r["mounts"]);
    let report = out.get(&Key::new("report.json").unwrap(), Default::default()).await.unwrap().unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&report.body).unwrap(), serde_json::json!({ "total": 8 }));
    assert!(!root.join("REPORT.md").exists());
}
