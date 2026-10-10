//! Persistent sessions: one VM that keeps its state across runs, like a
//! notebook kernel. Top-level declarations from one cell are visible in the
//! next. A whole session can be sealed into a token and restored later, in
//! another process.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::engine::{Clock, Engine, Guest, now_epoch_us};
use crate::run::{ClockMode, ErrorKind, LogEntry, Outcome, Resolution, RunError, RunOptions, RunResult};
use crate::snapshot;
use crate::tools::Toolbox;

pub struct Session {
    engine: Engine,
    tools: Toolbox,
    opts: RunOptions,
    guest: Option<Guest>,
    cells: u32,
}

#[derive(Serialize, Deserialize)]
struct SessionHeader {
    v: u32,
    kind: String,
    image: String,
    scope: String,
    created_ms: u64,
    expires_ms: u64,
    cells: u32,
    #[serde(default)]
    ext: std::collections::BTreeMap<String, String>,
}

impl Engine {
    /// Start a session. The VM is created lazily on the first run.
    pub fn session(&self, tools: Toolbox, opts: RunOptions) -> Session {
        Session { engine: self.clone(), tools, opts, guest: None, cells: 0 }
    }

    /// Restore a session sealed with [`Session::save`].
    pub async fn restore_session(&self, token: &str, tools: Toolbox, opts: RunOptions) -> Result<Session, RunError> {
        let invalid = |m: String| RunError { kind: ErrorKind::InvalidContinuation, name: "SessionError".into(), message: m, stack: None };
        let inner = &self.inner;
        let blob = inner.sealer.open(token).map_err(invalid)?;
        let (h, diff): (SessionHeader, snapshot::MemDiff) = snapshot::unpack(&blob).map_err(invalid)?;
        if h.v != 1 || h.kind != "session" || h.image != inner.image_id {
            return Err(invalid("session was saved by a different sandbox image".into()));
        }
        if h.scope != tools.scope_hash() {
            return Err(invalid("session was saved with a different set of tools".into()));
        }
        if (now_epoch_us() / 1000.0) as u64 > h.expires_ms {
            return Err(invalid("session token has expired".into()));
        }
        tools.load_extensions(&h.ext).map_err(invalid)?;
        let cap = crate::run::guest_memory_cap(inner, &opts.limits);
        let guest = Guest::restore(inner, &diff, clock_of(&opts), cap)
            .await
            .map_err(|e| invalid(format!("{e:#}")))?;
        Ok(Session { engine: self.clone(), tools, opts, guest: Some(guest), cells: h.cells })
    }
}

fn clock_of(opts: &RunOptions) -> Clock {
    match opts.clock {
        ClockMode::Real => Clock::Real,
        ClockMode::Virtual { start_ms } => Clock::Virtual(start_ms * 1000.0),
    }
}

impl Session {
    /// Number of cells run so far.
    pub fn cells(&self) -> u32 {
        self.cells
    }

    pub fn tools(&self) -> &Toolbox {
        &self.tools
    }

    /// Run a cell in the session's VM.
    pub async fn run(&mut self, code: &str) -> RunResult {
        let t0 = Instant::now();
        let inner = &self.engine.inner;
        let mut fresh_note = None;
        let guest = match self.guest.take() {
            Some(g) => g,
            None => {
                if self.cells > 0 {
                    fresh_note = Some("[modebox] the previous cell was aborted, so this cell starts in a fresh session");
                }
                let cap = crate::run::guest_memory_cap(inner, &self.opts.limits);
                match Guest::new(inner, clock_of(&self.opts), cap).await {
                    Ok(g) => g,
                    Err(e) => return crate::run::fail(RunError {
                        kind: ErrorKind::Internal, name: "ModeboxError".into(), message: format!("{e:#}"), stack: None,
                    }),
                }
            }
        };
        self.cells += 1;
        let (mut r, g) = self.engine.run_on(guest, code, &self.tools, self.opts.clone(), t0, true).await;
        self.adopt(&r, g).await;
        if let Some(note) = fresh_note {
            r.logs.insert(0, LogEntry { level: "warn".into(), message: note.into() });
        }
        r
    }

    /// Continue a suspended cell (from this session or any token for the same
    /// tools); the resumed VM becomes the session's state.
    pub async fn resume(&mut self, continuation: &str, resolutions: impl IntoIterator<Item = (u32, Resolution)>) -> RunResult {
        let (r, g) = self.engine.resume_inner(continuation, resolutions, &self.tools, self.opts.clone()).await;
        if g.is_some() {
            self.guest = None;
            self.adopt(&r, g).await;
        }
        r
    }

    async fn adopt(&mut self, r: &RunResult, g: Option<Guest>) {
        let Some(mut g) = g else { return };
        match &r.outcome {
            // The token holds the suspended state; resume() brings it back.
            Outcome::Suspended(_) => self.guest = None,
            _ if r.stats.aborted => self.guest = None,
            Outcome::Failed(_) => {
                // The VM is intact but may still hold the dead run's promise.
                self.guest = if g.reset_run().await.is_ok() { Some(g) } else { None };
            }
            Outcome::Completed(_) => self.guest = Some(g),
        }
    }

    /// Forget all state; the next run starts a fresh VM.
    pub fn reset(&mut self) {
        self.guest = None;
        self.cells = 0;
    }

    /// Seal the session's VM into an encrypted token (`None` before the first
    /// cell, or while a cell is suspended).
    pub async fn save(&mut self) -> Option<String> {
        let inner = &self.engine.inner;
        let g = self.guest.as_mut()?;
        let diff = g.snapshot(&inner.base).await;
        let now = (now_epoch_us() / 1000.0) as u64;
        let header = SessionHeader {
            v: 1,
            kind: "session".into(),
            image: inner.image_id.clone(),
            scope: self.tools.scope_hash(),
            created_ms: now,
            expires_ms: now + inner.config.continuation_ttl.as_millis() as u64,
            cells: self.cells,
            ext: self.tools.save_extensions(),
        };
        Some(inner.sealer.seal(&snapshot::pack(&header, &diff)))
    }
}
