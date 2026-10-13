"use client";

import { useEffect, useRef, useState } from "react";

// The script the "agent" runs. Line 4 needs approval, so the run pauses there.
const SCRIPT = [
  "const overdue = await db.find({ overdue: true });",
  "for (const c of overdue) {",
  "  const amount = computeRefund(c);",
  "  await payments.refund(c.id, amount);",
  "}",
  "return overdue.length;",
];
const APPROVAL_LINE = 3;

type Phase = "running" | "awaiting" | "paused" | "done";

const CELL = 7;
const GAP = 3;
const STEP = CELL + GAP;

function randomToken() {
  const abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let s = "mb1.";
  for (let i = 0; i < 18; i++) s += abc[Math.floor(Math.random() * abc.length)];
  return s + "…";
}

export function MemoryField() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [phase, setPhase] = useState<Phase>("running");
  const [line, setLine] = useState(0);
  const [stats, setStats] = useState({ changed: 0, total: 0 });
  const [token, setToken] = useState("mb1.…");
  // With reduced motion the window holds still on the paused run, and only
  // moves when you press its button.
  const [reduced, setReduced] = useState(false);
  // Timers and drawing stop while the window is off screen or the tab is hidden.
  const [visible, setVisible] = useState(true);
  // The status line is only announced once you've used the button.
  const [touched, setTouched] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const redraw = useRef(() => {});
  const approved = useRef(false);

  // Mutable state the animation loop reads without re-rendering.
  const sim = useRef({
    cols: 0,
    rows: 0,
    heat: new Float32Array(0),
    dirty: new Uint8Array(0),
    last: 0,
    frozen: false,
    reduced: false,
  });

  // Advance the script one line at a time while running. The approval line
  // pauses the run until it's approved.
  useEffect(() => {
    if (phase !== "running") return;
    if (reduced) {
      setLine(SCRIPT.length - 1);
      setPhase("done");
      return;
    }
    if (!visible) return;
    const id = setTimeout(() => {
      if (line === APPROVAL_LINE && !approved.current) freeze("awaiting");
      else if (line === SCRIPT.length - 1) setPhase("done");
      else setLine(line + 1);
    }, 900);
    return () => clearTimeout(id);
  }, [phase, line, reduced, visible]);

  // After the script returns, start a fresh run.
  useEffect(() => {
    if (phase !== "done" || reduced || !visible) return;
    const id = setTimeout(restart, 2600);
    return () => clearTimeout(id);
  }, [phase, reduced, visible]);

  // While awaiting approval, approve on its own after a while so the loop keeps going.
  useEffect(() => {
    if (phase !== "awaiting" || reduced || !visible) return;
    const id = setTimeout(() => resume(), 7000);
    return () => clearTimeout(id);
  }, [phase, reduced, visible]);

  function countDirty() {
    const s = sim.current;
    let n = 0;
    for (let i = 0; i < s.dirty.length; i++) n += s.dirty[i];
    return n;
  }

  function freeze(next: Phase) {
    sim.current.frozen = true;
    setStats({ changed: countDirty(), total: sim.current.dirty.length });
    setToken(randomToken());
    setPhase(next);
  }

  function resume() {
    if (phase === "awaiting") approved.current = true;
    sim.current.frozen = false;
    setPhase("running");
  }

  function restart() {
    sim.current.dirty.fill(0);
    approved.current = false;
    setLine(0);
    setPhase("running");
  }

  // The still picture: paused at the call that needs approval.
  function holdAtApproval() {
    const s = sim.current;
    for (let i = 0; i < s.dirty.length; i += 1) s.dirty[i] = Math.random() < 0.06 ? 1 : 0;
    approved.current = false;
    setLine(APPROVAL_LINE);
    freeze("awaiting");
    redraw.current();
  }

  function onButton() {
    setTouched(true);
    if (phase === "running") freeze("paused");
    else if (phase === "paused" || phase === "awaiting") resume();
    else if (reduced) holdAtApproval();
  }

  // Canvas: size to the container, then draw every frame while it is on screen.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const s = sim.current;
    s.reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    let lastW = -1;
    let lastH = -1;
    const resize = () => {
      const rect = canvas.getBoundingClientRect();
      // Only react to real changes in the displayed size.
      if (Math.abs(rect.width - lastW) < 1 && Math.abs(rect.height - lastH) < 1) return;
      lastW = rect.width;
      lastH = rect.height;
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(rect.width * dpr);
      canvas.height = Math.round(rect.height * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const cols = Math.max(8, Math.floor((rect.width + GAP) / STEP));
      const rows = Math.max(6, Math.floor((rect.height + GAP) / STEP));
      if (cols !== s.cols || rows !== s.rows) {
        s.cols = cols;
        s.rows = rows;
        s.heat = new Float32Array(cols * rows);
        s.dirty = new Uint8Array(cols * rows);
        s.last = Math.floor(Math.random() * cols * rows);
        if (s.reduced) holdAtApproval();
      }
      if (!raf) draw();
    };

    let raf = 0;
    const draw = () => {
      const n = s.cols * s.rows;
      if (!s.frozen && !s.reduced) {
        // Writes cluster near recent ones, like a heap does.
        for (let k = 0; k < 1; k++) {
          const near = Math.random() < 0.7;
          let i = near
            ? s.last + Math.floor((Math.random() - 0.5) * s.cols * 2)
            : Math.floor(Math.random() * n);
          i = (i + n) % n;
          s.heat[i] = 1;
          s.dirty[i] = 1;
          s.last = i;
        }
      }
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      for (let i = 0; i < n; i++) {
        const x = (i % s.cols) * STEP;
        const y = Math.floor(i / s.cols) * STEP;
        if ((s.frozen || s.reduced) && s.dirty[i]) {
          ctx.fillStyle = "#ededed";
        } else {
          const h = s.heat[i];
          const v = Math.round(31 + h * 95);
          ctx.fillStyle = `rgb(${v},${v},${v})`;
          if (!s.frozen) s.heat[i] = h * 0.95;
        }
        ctx.fillRect(x, y, CELL, CELL);
      }
    };
    const loop = () => {
      draw();
      raf = requestAnimationFrame(loop);
    };

    redraw.current = draw;
    if (s.reduced) setReduced(true);
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    let onScreen = true;
    const update = () => {
      const show = onScreen && !document.hidden;
      setVisible(show);
      if (show && !s.reduced && !raf) raf = requestAnimationFrame(loop);
      if (!show && raf) {
        cancelAnimationFrame(raf);
        raf = 0;
      }
    };
    const io = new IntersectionObserver(([entry]) => {
      onScreen = entry.isIntersecting;
      update();
    });
    if (rootRef.current) io.observe(rootRef.current);
    document.addEventListener("visibilitychange", update);
    update();

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      io.disconnect();
      document.removeEventListener("visibilitychange", update);
    };
  }, []);

  const frozen = phase === "awaiting" || phase === "paused";
  const state =
    phase === "running" ? "running" : phase === "awaiting" ? "needs approval" : phase === "paused" ? "paused" : "returned";
  const status =
    phase === "awaiting"
      ? "payments.refund needs your approval"
      : phase === "paused"
        ? `paused at line ${line + 1}`
        : phase === "done"
          ? "returned · nothing ran twice"
          : "booted from snapshot · ~0.1 ms";

  return (
    <div ref={rootRef} className={`sbx${frozen ? " is-frozen" : ""}`}>
      <div className="sbx-bar">
        <span className="sbx-id">sandbox · mb_7f3a2c</span>
        <span className="sbx-state">{state}</span>
        <button
          type="button"
          className="sbx-button"
          onClick={onButton}
          disabled={phase === "done" && !reduced}
        >
          {phase === "running" ? (
            <>
              <span aria-hidden="true">❚❚</span>
              {"  Pause"}
            </>
          ) : phase === "done" ? (
            reduced ? "Run again" : "Done"
          ) : (
            <>
              {phase === "awaiting" ? "Approve  " : "Resume  "}
              <span aria-hidden="true">▶</span>
            </>
          )}
        </button>
      </div>
      <div className="sbx-line" aria-hidden="true">
        <span className="sbx-ln">{String(line + 1).padStart(2, "0")}</span>
        <code>{SCRIPT[line]}</code>
      </div>
      <canvas
        ref={canvasRef}
        className="sbx-canvas"
        role="img"
        aria-label="The sandbox's memory, one square per 4 KiB page. Pages flicker as the script writes to them; when the run pauses, the changed pages turn white."
      />
      <div className="sbx-foot">
        <span aria-live={touched ? "polite" : "off"}>{status}</span>
        {frozen ? (
          <span className="sbx-token">
            {stats.changed.toLocaleString("en-US")} pages → <code>{token}</code>
          </span>
        ) : (
          <span>4 KiB pages</span>
        )}
      </div>
    </div>
  );
}
