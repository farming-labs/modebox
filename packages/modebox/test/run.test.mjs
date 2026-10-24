import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  RunAbortedError,
  RunBridgeLimitError,
  RunConcurrencyError,
  RunCpuTimeoutError,
  RunError,
  RunProtocolError,
  RunSourceTooLargeError,
  RunTimeoutError,
  createRunner,
  createSignedContinuationCodec,
  getHostFunctionContext,
  isRunInterruptedResult,
  run,
  setMaxWorkers,
} from '../run.mjs';

const codec = () => createSignedContinuationCodec({ secret: 's'.repeat(32), maxAgeMs: 60_000 });

test('runs a function body with host functions', async () => {
  const seen = [];
  const hostFunctions = {
    tools: {
      double: (x) => {
        const c = getHostFunctionContext();
        seen.push([c.hostFunctionName, c.requestIndex]);
        return x * 2;
      },
    },
  };
  const r = await run({ source: 'const a = await tools.double(1); return a + (await tools.double(20));', hostFunctions });
  assert.deepEqual(r, { status: 'completed', value: 42 });
  assert.deepEqual(seen, [['tools.double', 1], ['tools.double', 2]]);
});

test('interrupts resume with the resolution and the original payload', async () => {
  const runner = createRunner({ continuationCodec: codec() });
  const resumed = [];
  const hostFunctions = {
    auth: {
      token: (scope) => {
        const c = getHostFunctionContext();
        if (!c.resume) c.interrupt({ kind: 'sign_in', scope });
        resumed.push(c.resume);
        return `${c.resume.resolution}:${scope}`;
      },
    },
  };
  const source = 'return await auth.token("repo");';
  const a = await runner.run({ source, hostFunctions });
  assert.ok(isRunInterruptedResult(a));
  assert.deepEqual(a.interruptions, [
    { id: 'interrupt-1', hostFunctionName: 'auth.token', arguments: ['repo'], payload: { kind: 'sign_in', scope: 'repo' } },
  ]);
  const b = await runner.run({ source, hostFunctions, continuation: a.continuation, resolutions: [{ interruptionId: 'interrupt-1', value: 'abc' }] });
  assert.deepEqual(b, { status: 'completed', value: 'abc:repo' });
  assert.deepEqual(resumed, [{ interruptionId: 'interrupt-1', payload: { kind: 'sign_in', scope: 'repo' }, resolution: 'abc' }]);
  // single-use
  await assert.rejects(
    runner.run({ source, hostFunctions, continuation: a.continuation, resolutions: [{ interruptionId: 'interrupt-1', value: 'x' }] }),
    RunProtocolError,
  );
});

test('interruptions without a resolution stay pending', async () => {
  const runner = createRunner({ continuationCodec: codec() });
  let calls = 0;
  const hostFunctions = {
    ask: {
      q: (name) => {
        calls++;
        const c = getHostFunctionContext();
        if (!c.resume) c.interrupt({ name });
        return c.resume.resolution;
      },
    },
  };
  const source = 'return await Promise.all([ask.q("a"), ask.q("b")]);';
  const a = await runner.run({ source, hostFunctions });
  assert.deepEqual(a.interruptions.map((i) => i.id), ['interrupt-1', 'interrupt-2']);
  const b = await runner.run({ source, hostFunctions, continuation: a.continuation, resolutions: [{ interruptionId: 'interrupt-1', value: 1 }] });
  assert.equal(b.status, 'interrupted');
  assert.deepEqual(b.interruptions, [{ id: 'interrupt-2', hostFunctionName: 'ask.q', arguments: ['b'], payload: { name: 'b' } }]);
  const c = await runner.run({ source, hostFunctions, continuation: b.continuation, resolutions: [{ interruptionId: 'interrupt-2', value: 2 }] });
  assert.deepEqual(c, { status: 'completed', value: [1, 2] });
  assert.equal(calls, 4, 'the pending call is not re-invoked until it gets its resolution');
  await assert.rejects(
    runner.run({ source, hostFunctions, continuation: (await runner.run({ source, hostFunctions })).continuation, resolutions: [{ interruptionId: 'interrupt-9', value: 1 }] }),
    RunProtocolError,
  );
});

test('continuations are bound to the audience and context', async () => {
  const hostFunctions = { a: { ask: () => getHostFunctionContext().resume?.resolution ?? getHostFunctionContext().interrupt({}) } };
  const source = 'return await a.ask();';
  const first = createRunner({ continuationCodec: codec(), continuationAudience: 'app/v1' });
  const r = await first.run({ source, hostFunctions, continuationContext: { tenant: 't1', ids: [1, 2] } });
  const resolutions = [{ interruptionId: r.interruptions[0].id, value: 'ok' }];
  await assert.rejects(first.run({ source, hostFunctions, continuation: r.continuation, resolutions, continuationContext: { tenant: 't2' } }), RunProtocolError);
  const other = createRunner({ continuationCodec: codec(), continuationAudience: 'app/v2' });
  await assert.rejects(other.run({ source, hostFunctions, continuation: r.continuation, resolutions, continuationContext: { tenant: 't1', ids: [1, 2] } }), RunProtocolError);
  // Key order doesn't matter, and the token resumes in a fresh runner with the same key.
  const again = createRunner({ continuationCodec: codec(), continuationAudience: 'app/v1' });
  const done = await again.run({ source, hostFunctions, continuation: r.continuation, resolutions, continuationContext: { ids: [1, 2], tenant: 't1' } });
  assert.deepEqual(done, { status: 'completed', value: 'ok' });
});

test('limits use run defaults and are validated', async () => {
  for (const bad of [0, -1, 1.5, Number.NaN]) {
    await assert.rejects(run({ source: 'return 1', limits: { cpuTimeoutMs: bad } }), {
      name: 'TypeError',
      message: 'limits.cpuTimeoutMs must be a positive integer.',
    });
  }
  assert.throws(() => createRunner({ limits: { maxBridgeRequests: 2 ** 31 } }), /limits\.maxBridgeRequests must be at most 2147483647/);
  await assert.rejects(run({ source: `return "${'x'.repeat(300 * 1024)}"` }), (e) => e instanceof RunSourceTooLargeError && e.details.maxBytes === 256 * 1024);
});

test('limit failures map to run error classes', async () => {
  await assert.rejects(run({ source: 'while (true) {}', limits: { timeoutMs: 100 } }), (e) => e instanceof RunTimeoutError && e.details.timeoutMs === 100);
  await assert.rejects(run({ source: 'while (true) {}', limits: { cpuTimeoutMs: 100 } }), (e) => e instanceof RunCpuTimeoutError && e.code === 'RUN_CPU_TIMEOUT');
  await assert.rejects(run({ source: 'while (true) {}', abortSignal: AbortSignal.timeout(100) }), RunAbortedError);
  await assert.rejects(run({ source: 'for (let i = 0; i < 5; i++) await t.f(); return 1;', hostFunctions: { t: { f: () => 1 } }, limits: { maxBridgeRequests: 3 } }), RunBridgeLimitError);
  await assert.rejects(run({ source: 'return await t.big();', hostFunctions: { t: { big: () => 'x'.repeat(2000) } }, limits: { maxHostFunctionOutputBytes: 1000 } }), RunBridgeLimitError);
});

test('a RunError a host function throws reaches the caller with its details', async () => {
  const thrown = new RunError('quota exceeded', 'E_QUOTA', { limit: 5 });
  const hostFunctions = { billing: { charge: () => { throw thrown; } } };
  await assert.rejects(run({ source: 'await billing.charge();', hostFunctions }), (e) => e === thrown);
  // Guest errors keep their name and are RunErrors.
  await assert.rejects(run({ source: 'null.x' }), (e) => RunError.isInstance(e) && e.name === 'TypeError' && e.code === 'RUN_ERROR');
});

test('setMaxWorkers caps concurrent runs', async () => {
  assert.throws(() => setMaxWorkers(0), { name: 'TypeError', message: 'maxWorkers must be a positive integer.' });
  setMaxWorkers(1);
  try {
    const hold = Promise.withResolvers();
    const first = run({ source: 'return await t.wait();', hostFunctions: { t: { wait: () => hold.promise } } });
    await assert.rejects(run({ source: 'return 1' }), (e) => e instanceof RunConcurrencyError && e.details.maxWorkers === 1);
    hold.resolve('done');
    assert.equal((await first).value, 'done');
  } finally {
    setMaxWorkers(undefined);
  }
});

test('unsupported options fail clearly', async () => {
  await assert.rejects(run({ source: 'export const x = 1', sourceType: 'module' }), TypeError);
  await assert.rejects(run({ source: 'return 1', moduleLoader: () => '' }), TypeError);
  assert.throws(() => createRunner({ continuationCodec: { encode() {}, decode() {} } }), /custom codecs/);
  assert.throws(() => createSignedContinuationCodec({ secret: 'k', verificationSecrets: ['old'] }), /verificationSecrets/);
  assert.throws(() => createRunner({ syncHostFunctions: {} }), /syncHostFunctions/);
});
