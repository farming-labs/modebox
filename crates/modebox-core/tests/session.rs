use std::sync::OnceLock;
use std::time::Duration;

use modebox_core::*;

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine::new().expect("engine"))
}

fn tools() -> Toolbox {
    let mut t = Toolbox::new();
    t.func("db.users", |_c: Call| async move {
        Ok(Value::from(serde_json::json!([{"name": "ada", "age": 36}, {"name": "alan", "age": 41}, {"name": "grace", "age": 85}])))
    });
    t.add(Tool::new("db.drop", |_c: Call| async move { Ok(Value::from("dropped")) }).approval(Approval::Always));
    t
}

fn ok(r: &RunResult) -> Value {
    match &r.outcome {
        Outcome::Completed(v) => v.clone(),
        other => panic!("{other:?} logs={:?}", r.logs),
    }
}

#[tokio::test]
async fn state_persists_across_cells() {
    let mut s = engine().session(tools(), RunOptions::default());
    assert_eq!(ok(&s.run("const users = await db.users(); users.length").await), Value::from(3));
    assert_eq!(ok(&s.run("function names(min: number) { return users.filter(u => u.age > min).map(u => u.name) }").await), Value::Undefined);
    assert_eq!(ok(&s.run("names(40)").await).to_json(), serde_json::json!(["alan", "grace"]));
    assert_eq!(s.cells(), 3);
}

#[tokio::test]
async fn errors_keep_state_but_aborts_reset_it() {
    let mut s = engine().session(tools(), RunOptions { limits: Limits { timeout: Duration::from_millis(200), ..Limits::default() }, ..RunOptions::default() });
    ok(&s.run("let counter = 1").await);
    let r = s.run("counter++; throw new Error('boom')").await;
    assert!(matches!(r.outcome, Outcome::Failed(_)));
    assert_eq!(ok(&s.run("counter").await), Value::from(2));

    let r = s.run("while (true) {}").await;
    assert_eq!(r.error().unwrap().kind, ErrorKind::Timeout);
    let r = s.run("typeof counter").await;
    assert_eq!(ok(&r), Value::from("undefined"));
    assert!(r.logs[0].message.contains("fresh session"));
}

#[tokio::test]
async fn sessions_save_and_restore() {
    let mut s = engine().session(tools(), RunOptions::default());
    ok(&s.run("const memo = new Map([['answer', 42]]); class Point { constructor(x) { this.x = x } }").await);
    let token = s.save().await.expect("token");
    drop(s);

    let mut restored = engine().restore_session(&token, tools(), RunOptions::default()).await.expect("restore");
    assert_eq!(ok(&restored.run("[memo.get('answer'), new Point(7).x]").await).to_json(), serde_json::json!([42, 7]));
}

#[tokio::test]
async fn suspended_cells_resume_into_the_session() {
    let mut s = engine().session(tools(), RunOptions::default());
    ok(&s.run("const before = 'kept'").await);
    let r = s.run("const res = await db.drop(); res").await;
    let susp = r.suspension().expect("suspended").clone();
    let r = s.resume(&susp.continuation, [(susp.interrupts[0].id, Resolution::Approve)]).await;
    assert_eq!(ok(&r), Value::from("dropped"));
    assert_eq!(ok(&s.run("[before, res]").await).to_json(), serde_json::json!(["kept", "dropped"]));
}
