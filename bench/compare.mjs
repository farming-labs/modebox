// Identical workloads against Vercel's `run` and modebox's Node binding
// (JS host functions on both sides):  node bench/compare.mjs run|modebox
import { performance } from 'node:perf_hooks';
const which = process.argv[2] ?? 'modebox';
const { run, createRunner, getHostFunctionContext } = await import(which === 'run' ? 'run' : '../packages/modebox/index.mjs');

const out = {};
const time = async (fn) => { const t = performance.now(); await fn(); return performance.now() - t; };
const echo = { tools: { echo: (x) => x } };

await run({ source: 'return 1;' }); // warm the worker pool

out.trivial_run_ms = (await time(async () => { for (let i = 0; i < 1000; i++) await run({ source: 'return 1;' }); })) / 1000;
out.one_host_call_ms = (await time(async () => { for (let i = 0; i < 200; i++) await run({ source: 'return await tools.echo(1);', hostFunctions: echo }); })) / 200;
out.host_calls_200_in_one_run_ms = await time(() => run({ source: 'let s = 0; for (let i = 0; i < 200; i++) s += await tools.echo(i); return s;', hostFunctions: echo, limits: { maxBridgeRequests: 1000 } }));
out.parallel8_per_run_ms = (await time(async () => {
  for (let b = 0; b < 125; b++) await Promise.all(Array.from({ length: 8 }, () => run({ source: 'return 1;' })));
})) / 1000;

let best = Infinity;
for (let k = 0; k < 3; k++) best = Math.min(best, await time(() => run({ source: 'let s = 0; for (let i = 1; i <= 10_000_000; i++) s += i; return s;' })));
out.cpu_sum_10m_ms = best;

out.return_1mb_string_ms = await time(() => run({ source: "return 'x'.repeat(1 << 20);", limits: { maxResultBytes: 4 << 20 } }));
const rows = Array.from({ length: 5000 }, (_, i) => ({ id: i, name: 'user' + i, email: `u${i}@example.com`, active: i % 2 === 0 }));
out.host_returns_5000_rows_ms = await time(() => run({ source: 'const r = await db.rows(); return r.length;', hostFunctions: { db: { rows: () => rows } }, limits: { maxHostFunctionOutputBytes: 16 << 20 } }));

const runner = createRunner({ continuationSecret: 'x'.repeat(32) });
const approve = { ask: () => { const c = getHostFunctionContext(); if (!c.resume) c.interrupt({ kind: 'approval' }); return true; } };
out.interrupt_resume_pair_ms = (await time(async () => {
  for (let i = 0; i < 50; i++) {
    const a = await runner.run({ source: 'return await approve.ask();', hostFunctions: { approve } });
    await runner.run({ source: 'return await approve.ask();', hostFunctions: { approve }, continuation: a.continuation, resolutions: [{ interruptionId: a.interruptions[0].id, value: true }] });
  }
})) / 50;

const hist = { db: { get: (i) => 'z'.repeat(4096) }, approve };
const src = 'for (let i = 0; i < 200; i++) await db.get(i); return await approve.ask();';
const first = await runner.run({ source: src, hostFunctions: hist, limits: { maxBridgeRequests: 1000 } });
out.token_bytes_after_200_calls = first.continuation.length;
out.resume_after_200_calls_ms = await time(() => runner.run({ source: src, hostFunctions: hist, limits: { maxBridgeRequests: 1000 }, continuation: first.continuation, resolutions: [{ interruptionId: first.interruptions[0].id, value: true }] }));

console.log(JSON.stringify(out, null, 2));
process.exit(0);
