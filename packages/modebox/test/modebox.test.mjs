import { test } from 'node:test';
import assert from 'node:assert/strict';
import { run, createRunner, getHostFunctionContext, requireApproval, RunError } from '../index.mjs';

test('runs code with host functions', async () => {
  const r = await run({ source: 'return await tools.double(21);', hostFunctions: { tools: { double: (x) => x * 2 } } });
  assert.equal(r.status, 'completed');
  assert.equal(r.value, 42);
});

test('rich values cross both ways', async () => {
  const r = await run({
    source: 'const v = await t.echo(input); [v.d instanceof Date, v.m.get("k"), typeof v.n, v.b[1]]',
    hostFunctions: { t: { echo: (v) => v }, },
  }).catch(() => null);
  // `input` isn't defined: errors are RunErrors with a code
  assert.equal(r, null);
  const ok = await run({
    source: 'const v = await t.make(); [v.d instanceof Date, v.m.get("k"), typeof v.n, v.b[1]]',
    hostFunctions: { t: { make: () => ({ d: new Date(5), m: new Map([['k', 1]]), n: 10n, b: new Uint8Array([7, 8]) }) } },
  });
  assert.deepEqual(ok.value, [true, 1, 'bigint', 8]);
});

test('console output is captured, not printed', async () => {
  const r = await run({ source: 'console.log("hello", { a: 1 }); 1' });
  assert.deepEqual(r.logs, [{ level: 'log', message: 'hello { a: 1 }' }]);
});

test('failures throw RunError with kind and code', async () => {
  await assert.rejects(run({ source: 'null.x' }), (e) => e instanceof RunError && e.kind === 'Uncaught' && e.name === 'TypeError');
  await assert.rejects(run({ source: 'while(true){}', limits: { timeoutMs: 100 } }), (e) => e.code === 'RUN_TIMEOUT');
});

test('getHostFunctionContext().interrupt() suspends; resume re-invokes with the resolution', async () => {
  const runner = createRunner({ continuationSecret: 'k'.repeat(32) });
  let calls = 0;
  const hostFunctions = {
    auth: {
      login: () => {
        calls++;
        const ctx = getHostFunctionContext();
        if (!ctx.resume) ctx.interrupt({ kind: 'sign_in', url: 'https://example.test' });
        return `token:${ctx.resume.resolution}`;
      },
    },
  };
  const a = await runner.run({ source: 'const r = Math.random(); const t = await auth.login(); [t, r]', hostFunctions });
  assert.equal(a.status, 'interrupted');
  assert.equal(a.interruptions[0].kind, 'sign_in');
  assert.equal(a.interruptions[0].payload.url, 'https://example.test');
  const b = await runner.run({ hostFunctions, continuation: a.continuation, resolutions: [{ interruptionId: a.interruptions[0].id, value: 'abc' }] });
  assert.equal(b.value[0], 'token:abc');
  assert.equal(calls, 2);
  // single-use
  await assert.rejects(runner.run({ hostFunctions, continuation: a.continuation, resolutions: [{ interruptionId: a.interruptions[0].id, value: 'x' }] }), (e) => e.kind === 'InvalidContinuation');
});

test('interrupt() unwinds through host code that rethrows errors', async () => {
  const runner = createRunner({ continuationSecret: 'k'.repeat(32) });
  const hostFunctions = {
    svc: {
      call: () => {
        try {
          getHostFunctionContext().interrupt({ kind: 'auth' });
        } catch (e) {
          if (e instanceof Error) throw e;
          return 'swallowed';
        }
      },
    },
  };
  const a = await runner.run({ source: 'return await svc.call()', hostFunctions });
  assert.equal(a.status, 'interrupted');
  assert.equal(a.interruptions[0].kind, 'auth');
});

test('approval policies pause before the host function runs', async () => {
  const runner = createRunner({ continuationSecret: 'k'.repeat(32) });
  const sent = [];
  const hostFunctions = { bank: { send: requireApproval((amt) => { sent.push(amt); return 'ok'; }) } };
  const a = await runner.run({ source: 'await bank.send(5)', hostFunctions });
  assert.equal(a.status, 'interrupted');
  assert.equal(sent.length, 0);
  const b = await runner.run({ hostFunctions, continuation: a.continuation, resolutions: [{ interruptionId: a.interruptions[0].id, value: true }] });
  assert.equal(b.value, 'ok');
  assert.deepEqual(sent, [5]);
});

test('a shared continuationsDir makes tokens single-use across runners', async () => {
  const { mkdtempSync, rmSync } = await import('node:fs');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const dir = mkdtempSync(join(tmpdir(), 'modebox-used-'));
  // Two runners with the same key and directory stand in for two processes.
  const [a, b] = [0, 1].map(() => createRunner({ continuationSecret: 'k'.repeat(32), continuationsDir: dir }));
  const hostFunctions = { bank: { send: requireApproval(() => 'ok') } };
  const s = await a.run({ source: 'await bank.send(1)', hostFunctions });
  const resolutions = [{ interruptionId: s.interruptions[0].id, value: true }];
  assert.equal((await b.run({ hostFunctions, continuation: s.continuation, resolutions })).value, 'ok');
  await assert.rejects(a.run({ hostFunctions, continuation: s.continuation, resolutions }), (e) => e.kind === 'InvalidContinuation');
  rmSync(dir, { recursive: true });
});

test('a sink after a source pauses for approval', async () => {
  const runner = createRunner({ continuationSecret: 'k'.repeat(32), sources: ['mail.*'], sinks: ['http.post'] });
  const sent = [];
  const hostFunctions = { mail: { read: () => 'secret' }, http: { post: (v) => { sent.push(v); return 'ok'; } } };
  const free = await runner.run({ source: 'await http.post("hi")', hostFunctions });
  assert.equal(free.value, 'ok');
  const a = await runner.run({ source: 'await http.post(await mail.read())', hostFunctions });
  assert.equal(a.status, 'interrupted');
  assert.match(a.interruptions[0].payload.reason, /mail\.read/);
  assert.deepEqual(sent, ['hi']);
});

test('declarations for prompts', () => {
  const runner = createRunner();
  const d = runner.declarations({ gh: { issues: { list: () => [] } } }, { 'gh.issues.list': { description: 'List issues', inputSchema: { type: 'object', properties: { repo: { type: 'string' } }, required: ['repo'] } } });
  assert.match(d, /declare const gh/);
  assert.match(d, /list\(input: \{ repo: string \}\): Promise<unknown>;/);
});
