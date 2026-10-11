//! Data-flow checks: once a run has called a source tool, every call to a
//! sink tool needs approval.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use modebox_core::*;

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine::new().expect("engine"))
}

/// `mail.read` is a source, `http.post` a sink (counting real sends), and
/// `ask` always pauses.
fn tools(sent: Arc<AtomicU32>) -> Toolbox {
    let mut t = Toolbox::new();
    t.add(Tool::new("mail.read", |_c: Call| async { Ok(Value::from("the secret")) }).source());
    t.add(
        Tool::new("http.post", move |c: Call| {
            let sent = sent.clone();
            async move {
                sent.fetch_add(1, Ordering::SeqCst);
                Ok(c.input().clone())
            }
        })
        .sink(),
    );
    t.add(Tool::new("ask", |_c: Call| async { Ok(Value::Null) }).approval(Approval::Always));
    t.func("math.add", |c: Call| async move { Ok(Value::from(c.args.iter().filter_map(Value::as_f64).sum::<f64>())) });
    t
}

fn suspension(r: &RunResult) -> &Suspension {
    match &r.outcome {
        Outcome::Suspended(s) => s,
        other => panic!("expected a pause, got {other:?}"),
    }
}

fn approve_all(s: &Suspension) -> Vec<(u32, Resolution)> {
    s.interrupts.iter().map(|i| (i.id, Resolution::Approve)).collect()
}

#[tokio::test]
async fn sinks_run_freely_until_a_source_is_read() {
    let sent = Arc::new(AtomicU32::new(0));
    let r = engine().run("await http.post('hello'); await math.add(1, 2)", &tools(sent.clone()), RunOptions::default()).await;
    assert_eq!(r.value(), Some(&Value::from(3)));
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_sink_after_a_source_needs_approval_each_time() {
    let sent = Arc::new(AtomicU32::new(0));
    let t = tools(sent.clone());
    let code = "const m = await mail.read(); await http.post(m); await http.post(m.length); 'done'";
    let r = engine().run(code, &t, RunOptions::default()).await;
    let s = suspension(&r).clone();
    assert_eq!(s.interrupts[0].tool, "http.post");
    let reason = s.interrupts[0].payload.get("reason").and_then(Value::as_str).unwrap_or_default().to_owned();
    assert!(reason.contains("mail.read"), "{reason}");
    assert_eq!(sent.load(Ordering::SeqCst), 0, "nothing was sent before approval");

    // Approving one send doesn't approve the next.
    let r = engine().resume(&s.continuation, approve_all(&s), &t, RunOptions::default()).await;
    let s = suspension(&r).clone();
    assert_eq!(sent.load(Ordering::SeqCst), 1);
    let r = engine().resume(&s.continuation, approve_all(&s), &t, RunOptions::default()).await;
    assert_eq!(r.value(), Some(&Value::from("done")));
    assert_eq!(sent.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_denied_sink_is_catchable() {
    let t = tools(Arc::new(AtomicU32::new(0)));
    let code = "await mail.read(); try { await http.post('x') } catch (e) { e.name }";
    let r = engine().run(code, &t, RunOptions::default()).await;
    let s = suspension(&r).clone();
    let deny = s.interrupts.iter().map(|i| (i.id, Resolution::Deny("no".into())));
    let r = engine().resume(&s.continuation, deny, &t, RunOptions::default()).await;
    assert_eq!(r.value(), Some(&Value::from("PermissionDenied")));
}

#[tokio::test]
async fn what_was_read_survives_a_pause() {
    let sent = Arc::new(AtomicU32::new(0));
    let t = tools(sent.clone());
    // Read, then pause on something unrelated; the send after resuming still counts the read.
    let r = engine().run("await mail.read(); await ask(); await http.post('x'); 'sent'", &t, RunOptions::default()).await;
    let s = suspension(&r).clone();
    assert_eq!(s.interrupts[0].tool, "ask");
    let r = engine().resume(&s.continuation, approve_all(&s), &t, RunOptions::default()).await;
    let s = suspension(&r).clone();
    assert_eq!(s.interrupts[0].tool, "http.post");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn what_was_read_carries_across_session_cells_and_saves() {
    let sent = Arc::new(AtomicU32::new(0));
    let t = tools(sent.clone());
    let mut session = engine().session(t.clone(), RunOptions::default());
    session.run("var secret = await mail.read()").await;
    let r = session.run("await http.post(secret)").await;
    assert_eq!(suspension(&r).interrupts[0].tool, "http.post");

    // A fresh session restored from a save remembers it too.
    let mut session = engine().session(t.clone(), RunOptions::default());
    session.run("var secret = await mail.read()").await;
    let saved = session.save().await.unwrap();
    let mut restored = engine().restore_session(&saved, t, RunOptions::default()).await.unwrap();
    let r = restored.run("await http.post(secret)").await;
    assert_eq!(suspension(&r).interrupts[0].tool, "http.post");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}
