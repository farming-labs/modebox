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
    const id = setTimeout(() => {
      if (line === APPROVAL_LINE && !approved.current) freeze("awaiting");
      else if (line === SCRIPT.length - 1) setPhase("done");
      else setLine(line + 1);
    }, 900);
    return () => clearTimeout(id);
  }, [phase, line]);

  // After the script returns, start a fresh run.
  useEffect(() => {
    if (phase !== "done") return;
    const id = setTimeout(() => {
      const s = sim.current;
      s.dirty.fill(0);
      approved.current = false;
      setLine(0);
      setPhase("running");
    }, 2600);
    return () => clearTimeout(id);
  }, [phase]);

  // While awaiting approval, approve on its own after a while so the loop keeps going.
  useEffect(() => {
    if (phase !== "awaiting") return;
    const id = setTimeout(() => resume(), 7000);
    return () => clearTimeout(id);
  }, [phase]);

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

  function onButton() {
    if (phase === "running") freeze("paused");
    else if (phase === "paused" || phase === "awaiting") resume();
  }

  // Canvas: size to the container, then draw every frame.
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
        if (s.reduced) {
          // A still picture of a paused run.
          for (let i = 0; i < s.dirty.length; i += 1) if (Math.random() < 0.06) s.dirty[i] = 1;
        }
      }
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

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
          ctx.fillStyle = "#fafafa";
        } else {
          const h = s.heat[i];
          const v = Math.round(27 + h * 90);
          ctx.fillStyle = `rgb(${v},${v},${v + 2})`;
          if (!s.frozen) s.heat[i] = h * 0.95;
        }
        ctx.fillRect(x, y, CELL, CELL);
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, []);

  const frozen = phase === "awaiting" || phase === "paused";
  const status =
    phase === "running"
      ? `running · line ${line + 1}`
      : phase === "awaiting"
        ? `paused at line ${APPROVAL_LINE + 1} · payments.refund needs your approval`
        : phase === "paused"
          ? `paused by you at line ${line + 1}`
          : "returned · nothing ran twice";

  return (
    <div className={`field${frozen ? " is-frozen" : ""}`}>
      <div className="field-script" aria-hidden="true">
        <span className="field-ln">{String(line + 1).padStart(2, "0")}</span>
        <code>{SCRIPT[line]}</code>
      </div>
      <canvas
        ref={canvasRef}
        className="field-canvas"
        role="img"
        aria-label="The sandbox's memory, one square per 4 KiB page. Pages flicker as the script writes to them; when the run pauses, the changed pages turn white."
      />
      <div className="field-bar">
        <p className="field-status" aria-live="polite">
          {status}
        </p>
        <button
          type="button"
          className="field-button"
          onClick={onButton}
          disabled={phase === "done"}
        >
          {phase === "running" ? "❚❚  Pause" : phase === "awaiting" ? "Approve  ▶" : phase === "paused" ? "Resume  ▶" : "Done"}
        </button>
      </div>
      <div className="field-token" aria-hidden={!frozen}>
        <span>
          sealed · {stats.changed.toLocaleString("en-US")} of {stats.total.toLocaleString("en-US")} pages changed
        </span>
        <code>{token}</code>
      </div>
    </div>
  );
}
