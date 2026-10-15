// A script that pauses for approval, then resumes in a new process.
//
//   node refund.mjs            run until the refund needs approval, save the token, exit
//   node refund.mjs approve    resume from the token and let the refund run
//   node refund.mjs deny       resume from the token and refuse it

import { createRunner, requireApproval } from 'modebox';
import { randomBytes } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';

// The code an agent might write. It only sees `orders` and `payments`.
const script = `
const order = await orders.get('A-1042');
console.log('found order', order.id, 'for $' + order.amount);
try {
  const r = await payments.refund({ orderId: order.id, amount: order.amount });
  console.log('refunded:', r.refundId);
} catch (e) {
  console.log('refund not allowed:', e.message);
}
'done'
`;

// Your functions. These run on the host, outside the sandbox.
const hostFunctions = {
  orders: {
    get(id) {
      console.log(`  host: orders.get(${id})`);
      return { id, amount: 42 };
    },
  },
  payments: {
    refund: requireApproval(({ orderId, amount }) => {
      console.log(`  host: payments.refund(${orderId}, $${amount}): money moved`);
      return { refundId: 're_' + randomBytes(4).toString('hex') };
    }),
  },
};

// Tokens are sealed with this key, so both processes need the same one.
if (!existsSync('.secret')) writeFileSync('.secret', randomBytes(32).toString('hex'), { mode: 0o600 });
const runner = createRunner({
  continuationSecret: readFileSync('.secret', 'utf8'),
  continuationsDir: '.used', // remembers spent tokens across processes
});
const print = (r) => r.logs.forEach((l) => console.log('  sandbox:', l.message));

const decision = process.argv[2];
let r;
if (!decision) {
  r = await runner.run({ source: script, hostFunctions });
} else {
  if (!existsSync('paused.json')) throw new Error('nothing to resume: run `node refund.mjs` first');
  const { continuation, interruptionId } = JSON.parse(readFileSync('paused.json', 'utf8'));
  console.log(`New process (pid ${process.pid}), resuming with "${decision}"`);
  r = await runner.run({
    hostFunctions,
    continuation,
    resolutions: [{ interruptionId, value: decision === 'approve' }],
    throwOnError: false,
  });
}
print(r);

if (r.status === 'interrupted') {
  const [i] = r.interruptions;
  writeFileSync('paused.json', JSON.stringify({ continuation: r.continuation, interruptionId: i.id }));
  console.log(`\nPaused before ${i.hostFunctionName}(${JSON.stringify(i.args[0])}).`);
  console.log(`The whole sandbox is sealed in paused.json (${Math.round(r.continuation.length / 1024)} KB). This process can exit now.`);
  console.log('Next: node refund.mjs approve   (or: deny)');
} else if (r.status === 'completed') {
  console.log(`\nFinished: ${JSON.stringify(r.value)}. orders.get didn't run again; the sandbox picked up where it paused.`);
  console.log('Try `node refund.mjs approve` again: a token works only once.');
} else {
  console.log(`\nRefused: ${r.error.message}`);
}
