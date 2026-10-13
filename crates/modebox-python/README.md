# modebox for Python

Run untrusted, agent-written JavaScript and TypeScript in a WebAssembly sandbox whose only authority is the functions you give it. When a call needs approval, the whole VM is sealed into an encrypted token; the run resumes later, even in another process, and nothing runs twice.

This is the Python API for [modebox](https://github.com/farming-labs/modebox). It matches the `modebox` npm package. Python 3.10 or later on macOS (arm64, x64) or Linux (x64, arm64, glibc).

```bash
pip install modebox
```

Each of those platforms gets a prebuilt wheel. Anywhere else, pip builds from the source distribution, which needs Rust 1.96 or later.

## Usage

```python
import modebox

async def get_weather(city: str) -> dict:
    return {"city": city, "temp": 18}

@modebox.require_approval
async def send_email(to: str, body: str) -> str:
    ...
    return "sent"

host_functions = {"weather": {"get": get_weather}, "email": {"send": send_email}}

runner = modebox.Runner(limits=modebox.Limits(timeout_ms=5_000))
result = await runner.run(
    "const w = await weather.get('Paris'); await email.send('me@example.com', `${w.temp}°C`); w.temp",
    host_functions=host_functions,
)
if result.status == "interrupted":  # paused before email.send; nothing has been sent
    result = await runner.run(
        host_functions=host_functions,
        continuation=result.continuation,  # a token you can store and resume in another process
        resolutions=[modebox.approve(result.interruptions[0].id)],
    )
print(result.value, result.logs, result.stats)
```

- `modebox.run(...)` uses a shared default `Runner`; `run_sync(...)` runs without an event loop.
- Host functions can be plain functions or `async def`. They run on your event loop.
- Inside a host function, `modebox.get_host_function_context()` gives the call's details, and `.interrupt(payload)` pauses the run. Resume with `modebox.resume_with(id, value)`, and the host function runs again with `ctx.resume.resolution == value`.
- `approve=["bank.*"]` needs approval for matching calls. `sources` and `sinks` make every sink call after a source call wait for approval.
- Failures raise `modebox.RunError` with `kind` and `code`, or return `status="failed"` with `throw_on_error=False`.
- `runner.declarations(host_functions, schemas)` gives TypeScript declarations for the model's prompt.

## Values

| JavaScript | Python |
|---|---|
| `null`, `undefined` | `None` (send `modebox.UNDEFINED` for `undefined`) |
| number | `int` when whole, else `float` (`NaN`, `±Infinity` and `-0` kept) |
| `BigInt` | `int` (ints beyond ±2⁵³ cross as `BigInt`) |
| string, boolean, array, object | `str`, `bool`, `list` (or `tuple`), `dict` |
| `Map`, `Set` | `modebox.JsMap` (a dict), `set` |
| `Date` | `datetime` in UTC (naive datetimes are taken as UTC) |
| `Uint8Array`, `ArrayBuffer` | `bytes` (other typed arrays: `array.array`) |
| `RegExp`, `Error` | `modebox.JsRegExp`, `modebox.JsError` |

Dataclasses cross as objects. Functions and other objects can't cross; returning one from a host function is an error in the sandbox.

See the [main README](https://github.com/farming-labs/modebox#readme) for the security model and limitations.
