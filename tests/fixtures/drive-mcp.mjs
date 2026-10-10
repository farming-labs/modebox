// Drive `modebox mcp` like an agent harness would, through each approval path:
//   node tests/fixtures/drive-mcp.mjs legacy|elicit|mrtr
import { spawn } from 'node:child_process';

const mode = process.argv[2] ?? 'legacy';
const bin = process.env.MODEBOX_BIN ?? './target/release/modebox';
const proc = spawn(bin, ['-c', 'modebox.example.json', 'mcp'], { stdio: ['pipe', 'pipe', 'inherit'] });
const waiters = new Map();
let buf = '', nextId = 0;
const send = (m) => proc.stdin.write(JSON.stringify(m) + '\n');
proc.stdout.on('data', (d) => {
  buf += d;
  for (let i; (i = buf.indexOf('\n')) >= 0; ) {
    const line = buf.slice(0, i); buf = buf.slice(i + 1);
    if (!line.trim()) continue;
    const msg = JSON.parse(line);
    if (msg.method && msg.id !== undefined) { // server -> client request
      if (msg.method === 'elicitation/create') {
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
console.log('tools:', list.result.tools.map((t) => t.name).join(', '));

console.log('\n> execute (read-only)');
console.log(text(await call('execute', { code: SUMMARIZE })));

console.log('\n> execute (needs approval)');
let res = await call('execute', { code: DELETE });
if (mode === 'legacy') {
  console.log(text(res));
  const id = text(res).match(/"id": "([^"]+)"/)[1];
  console.log('\n> approve', id);
  res = await call('approve', { id, approve: true });
}
if (mode === 'mrtr') {
  const r = res.result;
  console.log('  resultType:', r.resultType, '| requests:', Object.keys(r.inputRequests ?? {}).join(','), '| requestState bytes:', r.requestState?.length);
  for (const [k, req] of Object.entries(r.inputRequests ?? {})) console.log(`  [user prompt] ${req.params.message}\n  [user] approve`);
  const inputResponses = Object.fromEntries(Object.keys(r.inputRequests).map((k) => [k, { action: 'accept', content: { approve: true } }]));
  console.log('\n> retry with inputResponses + requestState (server kept no state)');
  res = await call('execute', { code: DELETE }, { inputResponses, requestState: r.requestState });
}
console.log(text(res));

console.log('\n> execute (uses state from earlier calls)');
console.log(text(await call('execute', { code: `({ earlier: work.length, deleted: typeof target === 'object' ? target.title : null })` })));
proc.kill();
process.exit(0);
