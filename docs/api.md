# Using modebox from code

More of the library APIs than the README shows: plain runs with logs and stats, plain host functions, and sessions that keep state between runs.

## Rust

```rust
use modebox_core::*;

let engine = Engine::new()?;                     // compile/cache + pre-init, once per process
let mut tools = Toolbox::new();
tools.func("db.users", |_c: Call| async { Ok(Value::from(serde_json::json!([{"name": "ada"}]))) });
tools.add(Tool::new("bank.transfer", transfer).approval(Approval::Always));

let r = engine.run("const u = await db.users(); u.length", &tools, RunOptions::default()).await;
match r.outcome {
    Outcome::Completed(v) => println!("{v:?}"),
    Outcome::Failed(e) => eprintln!("{e}"),
    Outcome::Suspended(s) => {
        // ask a human, later, maybe in another process (with the same $MODEBOX_SECRET):
        let r = engine.resume(&s.continuation, [(s.interrupts[0].id, Resolution::Approve)], &tools, RunOptions::default()).await;
    }
}

// notebook-style session
let mut s = engine.session(tools, RunOptions::default());
s.run("const users = await db.users()").await;
s.run("users.filter(u => u.name.startsWith('a'))").await;
let token = s.save().await;                      // restore with engine.restore_session(token, ...)
```

## Node

```js
import { run, createRunner, getHostFunctionContext, requireApproval } from 'modebox';

const { value, logs, stats } = await run({
  source: 'const d = await tools.double(21); console.log(d); d',
  hostFunctions: { tools: { double: (x) => x * 2 } },
});

const runner = createRunner({ continuationSecret: process.env.SECRET });
const r = await runner.run({ source, hostFunctions: { bank: { send: requireApproval(send) } } });
if (r.status === 'interrupted') {
  await runner.run({ hostFunctions, continuation: r.continuation,
                     resolutions: [{ interruptionId: r.interruptions[0].id, value: true }] });
}
```

To run code written for Vercel's `run` package, import the same API from `modebox/run`; see [In place of Vercel's `run`](run-compat.md).

## Python

The same API in Python 3.10+, with `async def` or plain host functions. See [crates/modebox-python](../crates/modebox-python/README.md) for how values map.

```python
import asyncio
import os

import modebox

async def main():
    result = await modebox.run(
        "const d = await tools.double(21); console.log(d); d",
        host_functions={"tools": {"double": lambda x: x * 2}},
    )
    print(result.value, result.logs, result.stats)

    runner = modebox.Runner(continuation_secret=os.environ["SECRET"])
    r = await runner.run(source, host_functions={"bank": {"send": modebox.require_approval(send)}})
    if r.status == "interrupted":
        await runner.run(host_functions=host_functions, continuation=r.continuation,
                         resolutions=[modebox.approve(r.interruptions[0].id)])

asyncio.run(main())
```
