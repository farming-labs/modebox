// Minimal dependency-free MCP server over stdio (newline-delimited JSON-RPC).
// Used as a downstream server in modebox's MCP tests.
import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';

const log = (m) => process.env.NOTES_LOG && appendFileSync(process.env.NOTES_LOG, m + '\n');
const notes = new Map([
  [1, { id: 1, title: 'groceries', body: 'eggs, milk', tags: ['home'] }],
  [2, { id: 2, title: 'standup', body: 'ship modebox', tags: ['work'] }],
  [3, { id: 3, title: 'deploy checklist', body: 'tests, tag, release', tags: ['work', 'ops'] }],
]);
let nextId = 4;

const tools = [
  { name: 'list_notes', description: 'List notes, optionally filtered by tag.',
    inputSchema: { type: 'object', properties: { tag: { type: 'string' } } } },
  { name: 'get-note', description: 'Get one note by id.',
    inputSchema: { type: 'object', properties: { id: { type: 'integer' } }, required: ['id'] } },
  { name: 'add_note', description: 'Create a note.',
    inputSchema: { type: 'object', properties: { title: { type: 'string' }, body: { type: 'string' }, tags: { type: 'array', items: { type: 'string' } } }, required: ['title'] },
    outputSchema: { type: 'object', properties: { id: { type: 'integer' } }, required: ['id'] } },
  { name: 'delete_note', description: 'Delete a note permanently.',
    inputSchema: { type: 'object', properties: { id: { type: 'integer' } }, required: ['id'] } },
];

function call(name, args = {}) {
  switch (name) {
    case 'list_notes': return [...notes.values()].filter((n) => !args.tag || n.tags.includes(args.tag));
    case 'get-note': { const n = notes.get(args.id); if (!n) throw new Error(`note ${args.id} not found`); return n; }
    case 'add_note': { const n = { id: nextId++, title: args.title, body: args.body ?? '', tags: args.tags ?? [] }; notes.set(n.id, n); return { id: n.id }; }
    case 'delete_note': { if (!notes.delete(args.id)) throw new Error(`note ${args.id} not found`); return { deleted: args.id }; }
    default: throw new Error(`unknown tool ${name}`);
  }
}

const send = (msg) => process.stdout.write(JSON.stringify(msg) + '\n');
createInterface({ input: process.stdin }).on('line', (line) => {
  if (!line.trim()) return;
  const msg = JSON.parse(line);
  log('<- ' + line);
  if (msg.id === undefined) return; // notification
  const reply = (result) => send({ jsonrpc: '2.0', id: msg.id, result });
  const fail = (code, message) => send({ jsonrpc: '2.0', id: msg.id, error: { code, message } });
  switch (msg.method) {
    case 'initialize':
      return reply({ protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'notes', version: '1.0.0' } });
    case 'ping': return reply({});
    case 'tools/list': return reply({ tools });
    case 'tools/call':
      try {
        const out = call(msg.params.name, msg.params.arguments);
        return reply({ content: [{ type: 'text', text: JSON.stringify(out) }], ...(msg.params.name === 'add_note' ? { structuredContent: out } : {}) });
      } catch (e) {
        return reply({ content: [{ type: 'text', text: e.message }], isError: true });
      }
    default: return fail(-32601, `method not found: ${msg.method}`);
  }
});
