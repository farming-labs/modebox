// Drive `modebox mcp` like an agent harness would, through each approval path:
//   node tests/fixtures/drive-mcp.mjs legacy|elicit|mrtr|deny
// legacy: no elicitation, `mcp.approveTool` on, the `approve` tool resumes.
// deny:   no elicitation, `approveTool` off (the default): the call is denied.
// Exits non-zero if any step gives the wrong answer, so CI can run it.
import { spawn } from 'node:child_process';

const mode = process.argv[2] ?? 'legacy';
const bin = process.env.MODEBOX_BIN ?? './target/release/modebox';
const config = mode === 'legacy' ? 'tests/fixtures/approve-tool.json' : 'modebox.example.json';
const proc = spawn(bin, ['-c', config, 'mcp'], { stdio: ['pipe', 'pipe', 'inherit'] });
const waiters = new Map();
let buf = '', nextId = 0, prompts = 0, done = false;
const failures = [];
const expect = (ok, what) => { if (!ok) { failures.push(what); console.log(`  FAIL: ${what}`); } };
const bail = (why) => { console.error(`drive-mcp: ${why}`); proc.kill(); process.exit(1); };
proc.on('error', (e) => bail(`could not start ${bin}: ${e.message}`));
proc.on('exit', (code) => { if (!done) bail(`modebox exited early (${code})`); });
setTimeout(() => bail('timed out'), 120_000).unref();
const send = (m) => proc.stdin.write(JSON.stringify(m) + '\n');
proc.stdout.on('data', (d) => {
  buf += d;
  for (let i; (i = buf.indexOf('\n')) >= 0; ) {
    const line = buf.slice(0, i); buf = buf.slice(i + 1);
    if (!line.trim()) continue;
    const msg = JSON.parse(line);
    if (msg.method && msg.id !== undefined) { // server -> client request
      if (msg.method === 'elicitation/create') {
        prompts++;
        console.log(`  [user prompt] ${msg.params.message}\n  [user] approve`);
        send({ jsonrpc: '2.0', id: msg.id, result: { action: 'accept', content: { approve: true } } });
      } else send({ jsonrpc: '2.0', id: msg.id, result: {} });
    } else if (msg.id !== undefined && waiters.has(msg.id)) { waiters.get(msg.id)(msg); waiters.delete(msg.id); }
  }
});
const request = (method, params) => new Promise((r) => { const id = ++nextId; waiters.set(id, r); send({ jsonrpc: '2.0', id, method, params }); });
const meta2026 = { 'io.modelcontextprotocol/protocolVersion': '2026-07-28', 'io.modelcontextprotocol/clientCapabilities': { elicitation: { form: {} } }, 'io.modelcontextprotocol/clientInfo': { name: 'drive', version: '1' } };
const text = (res) => res.result?.content?.map((c) => c.text).join('\n') ?? JSON.stringify(res.error ?? res.result);

const SUMMARIZE = `
const work = await notes.list_notes({ tag: 'work' });
console.log('found', work.length, 'work notes');
work.map(n => n.title.toUpperCase())`;
const DELETE = `
const all = await notes.list_notes();
const target = all.find(n => n.title === 'standup');
const r = await notes.delete_note({ id: target.id });
({ deleted: r.deleted, remaining: (await notes.list_notes()).map(n => n.title) })`;

const call = (name, args, extra = {}) =>
  request('tools/call', { name, arguments: args, ...(mode === 'mrtr' ? { _meta: meta2026 } : {}), ...extra });

if (mode !== 'mrtr') {
  const init = await request('initialize', { protocolVersion: '2025-06-18', clientInfo: { name: 'drive', version: '1' },
    capabilities: mode === 'elicit' ? { elicitation: {} } : {} });
  console.log('initialized, protocol', init.result.protocolVersion);
  send({ jsonrpc: '2.0', method: 'notifications/initialized' });
}
const list = await request('tools/list', mode === 'mrtr' ? { _meta: meta2026 } : {});
const names = list.result.tools.map((t) => t.name);
console.log('tools:', names.join(', '));
expect(['execute', 'types'].every((n) => names.includes(n)), 'tools/list has execute and types');
expect(names.includes('approve') === (mode === 'legacy'), 'approve is listed only when approveTool is on');
const annotations = Object.fromEntries(list.result.tools.map((t) => [t.name, t.annotations ?? {}]));
expect(annotations.types.readOnlyHint === true && annotations.execute.destructiveHint === true, 'tools carry annotations');

console.log('\n> execute (read-only)');
const summary = text(await call('execute', { code: SUMMARIZE }));
console.log(summary);
expect(summary.includes('"STANDUP"') && summary.includes('"DEPLOY CHECKLIST"'), 'read-only run returns both work notes');
expect(summary.includes('found 2 work notes'), 'read-only run returns console output');

console.log('\n> execute (needs approval)');
let res = await call('execute', { code: DELETE });
if (mode === 'legacy') {
  console.log(text(res));
  const id = text(res).match(/"id": "([^"]+)"/)?.[1];
  if (!id) bail('no approval id in the paused result');
  console.log('\n> approve', id);
  res = await call('approve', { id, approve: true });
  console.log('\n> approve again (a retry after a lost response)');
  const again = await call('approve', { id, approve: true });
  expect(text(again) === text(res), 'a retried approve returns the same result');
}
if (mode === 'mrtr') {
  const r = res.result;
  expect(r?.resultType === 'input_required' && !!r.requestState, 'MRTR run asks for input with requestState');
  if (!r?.inputRequests) bail('no inputRequests in the MRTR result');
  console.log('  resultType:', r.resultType, '| requests:', Object.keys(r.inputRequests ?? {}).join(','), '| requestState bytes:', r.requestState?.length);
  for (const req of Object.values(r.inputRequests)) prompts++, console.log(`  [user prompt] ${req.params.message}\n  [user] approve`);
  const inputResponses = Object.fromEntries(Object.keys(r.inputRequests).map((k) => [k, { action: 'accept', content: { approve: true } }]));
  console.log('\n> retry with inputResponses + requestState (server kept no state)');
  res = await call('execute', { code: DELETE }, { inputResponses, requestState: r.requestState });
  console.log('\n> retry the same request (as if the response was lost)');
  const again = await call('execute', { code: DELETE }, { inputResponses, requestState: r.requestState });
  expect(text(again) === text(res), 'a retried resume returns the same result, not "already used"');
}
const deleted = text(res);
console.log(deleted);
if (mode === 'deny') {
  expect(prompts === 0, 'nobody was prompted');
  expect(deleted.includes('PermissionDenied') && deleted.includes('approveTool'), 'the call is denied and the result says how to allow it');
  const left = text(await call('execute', { code: `(await notes.list_notes()).map(n => n.title)` }));
  expect(left.includes('"standup"'), 'nothing was deleted');
} else {
  expect(prompts === (mode === 'legacy' ? 0 : 1), `the user was asked ${mode === 'legacy' ? 'through the approve tool' : 'once'}`);
  expect(deleted.includes('"deleted": 2') && !deleted.includes('"standup"'), 'approved delete removes the standup note');
}

console.log('\n> execute (uses state from earlier calls)');
const state = text(await call('execute', { code: `({ earlier: work.length, deleted: typeof target === 'object' ? target.title : null })` }));
console.log(state);
expect(state.includes('"earlier": 2') && state.includes('"deleted": "standup"'), 'later calls see earlier variables');

done = true;
proc.kill();
if (failures.length) { console.error(`\ndrive-mcp ${mode}: ${failures.length} check(s) failed`); process.exit(1); }
console.log(`\ndrive-mcp ${mode}: ok`);
process.exit(0);
