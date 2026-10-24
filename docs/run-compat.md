# Using modebox in place of Vercel's `run`

`modebox/run` has the API of Vercel's [`run`](https://www.npmjs.com/package/run) package: `run`, `createRunner`, `getHostFunctionContext`, `createSignedContinuationCodec`, `setMaxWorkers` and the `RunError` classes, with the same inputs and result shapes. Code written against `run` can use modebox without changes. The difference is how a paused run resumes: from a VM snapshot, so the script doesn't run again and the token stays small.

```js
import { createRunner, createSignedContinuationCodec, getHostFunctionContext } from 'modebox/run';

const runner = createRunner({
  continuationCodec: createSignedContinuationCodec({ secret: process.env.RUN_SECRET }),
});
const hostFunctions = {
  payments: {
    refund: async (input) => {
      const { resume, interrupt } = getHostFunctionContext();
      if (!resume) interrupt({ kind: 'approval', input });
      return resume.resolution === true ? refund(input) : 'declined';
    },
  },
};

const source = 'return await payments.refund({ id: "ch_1" });';
const paused = await runner.run({ source, hostFunctions });
// later, in any process with the same secret:
const done = await runner.run({
  source,
  hostFunctions,
  continuation: paused.continuation,
  resolutions: [{ interruptionId: paused.interruptions[0].id, value: true }],
});
```

## With a library that imports `run`

Libraries such as `@ai-sdk/code-mode` import `run` by package name. To run them on modebox, point `run` at a two-line local package:

```json
// run-modebox/package.json
{ "name": "run", "version": "0.0.0", "type": "module", "main": "index.mjs" }
```

```js
// run-modebox/index.mjs
export * from 'modebox/run';
```

Then depend on it as `"run": "file:./run-modebox"`, or use your package manager's override for `run`. `@ai-sdk/code-mode` 1.0.109's own test suite passes (65 of 65) this way.

## Differences from `run`

- Source is a function body with top-level `await` and `return`. `sourceType: 'module'`, `moduleLoader` and `syncHostFunctions` throw.
- Continuations come from `continuationSecret` or `createSignedContinuationCodec`. Custom codecs and `verificationSecrets` (key rotation) throw.
- `maxInFlightBridgeRequests` queues extra host calls instead of failing them.
- `maxStackSizeBytes` and `maxContinuationBytes` are validated but not enforced separately. QuickJS still reports a stack overflow as a `RangeError`.
- `setMaxWorkers()` caps how many runs execute at once. Without it there is no cap, since runs don't use worker threads.
- Error messages differ. Codes, classes and `details` match.
- Like the rest of modebox, it needs a native binding: macOS and Linux (glibc) only.
