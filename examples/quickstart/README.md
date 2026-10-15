# Quickstart: pause for approval, resume in a new process

A script looks up an order and refunds it. The refund needs approval, so the run pauses before it, and the paused sandbox is saved to a file. A second process approves or denies it and the script carries on from where it stopped. Needs Node 18+ on macOS or Linux.

```bash
git clone https://github.com/farming-labs/modebox && cd modebox/examples/quickstart
npm install
node refund.mjs            # pauses before payments.refund and exits
node refund.mjs approve    # a new process resumes; the refund runs once
node refund.mjs approve    # refused: a token works only once
```

```
  host: orders.get(A-1042)
  sandbox: found order A-1042 for $42

Paused before payments.refund({"orderId":"A-1042","amount":42}).
The whole sandbox is sealed in paused.json (15 KB). This process can exit now.
Next: node refund.mjs approve   (or: deny)

New process (pid 63786), resuming with "approve"
  host: payments.refund(A-1042, $42): money moved
  sandbox: refunded: re_4af593ab
```

`orders.get` runs once, in the first process. The second process doesn't replay the script; it restores the sandbox from the token. Run `node refund.mjs deny` instead to see the script handle a refusal.

[refund.mjs](refund.mjs) is the whole example. The files it writes next to itself:

- `.secret`: the key that seals tokens. Processes that resume each other's tokens need the same key (in your app, set `continuationSecret` or `$MODEBOX_SECRET`).
- `paused.json`: the token and the id of the call waiting for approval.
- `.used/`: spent tokens, so a token can't be resumed twice, even from another process.

[demo.tape](demo.tape) re-records the GIF at the top of the main README with [vhs](https://github.com/charmbracelet/vhs).

Next: [use modebox with your agent](../../docs/integrations.md), or [the full Node, Python and Rust APIs](../../docs/api.md).
