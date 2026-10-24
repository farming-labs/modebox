"""modebox for Python: run agent-written JavaScript/TypeScript in a WebAssembly sandbox.

API: ``run`` / ``Runner`` to execute code with host functions,
``get_host_function_context().interrupt()`` to pause a run, ``continuation`` +
``resolutions`` to resume it, plus approvals, captured console output, stats
and deterministic options. The same API as the ``modebox`` npm package.
"""

from __future__ import annotations

import array
import asyncio
import base64
import contextvars
import dataclasses
import datetime as _dt
import functools
import inspect
import json
import math
import os
import re
import time
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from typing import Any, Literal, NoReturn

from . import _native

__all__ = [
    "UNDEFINED",
    "HostFunctionContext",
    "Interruption",
    "JsError",
    "JsMap",
    "JsRegExp",
    "Limits",
    "LogEntry",
    "Resolution",
    "Resume",
    "RunError",
    "RunResult",
    "Runner",
    "Stats",
    "ToolSchema",
    "approve",
    "declarations",
    "deny",
    "get_host_function_context",
    "require_approval",
    "resume_with",
    "return_value",
    "run",
    "run_sync",
]

# ---- values -------------------------------------------------------------------


class _Undefined:
    """JavaScript ``undefined``, for values sent into the sandbox.

    Values coming out of the sandbox turn ``undefined`` into ``None``.
    """

    _instance: _Undefined | None = None

    def __new__(cls) -> _Undefined:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __repr__(self) -> str:
        return "UNDEFINED"

    def __bool__(self) -> bool:
        return False


UNDEFINED = _Undefined()


class JsMap(dict):  # type: ignore[type-arg]
    """A JavaScript ``Map``. It is a dict; sent back into the sandbox it stays a ``Map``, not an object."""


@dataclass(frozen=True)
class JsRegExp:
    """A JavaScript ``RegExp``. JS and Python regex syntax differ, so it isn't converted to ``re.Pattern``."""

    source: str
    flags: str = ""


class JsError(Exception):
    """A JavaScript ``Error`` crossing the boundary, as a value or a host function's exception."""

    def __init__(self, message: str = "", name: str = "Error", code: Any = None, stack: str | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.name = name
        self.code = code
        self.stack = stack

    def __str__(self) -> str:
        return f"{self.name}: {self.message}"


# Number.MAX_SAFE_INTEGER: larger ints cross as BigInt.
_MAX_SAFE = 2**53 - 1
# Typed array kinds ↔ array.array type codes (Uint8Array and ArrayBuffer are bytes).
_TYPECODES = {
    "Int8Array": "b",
    "Uint8ClampedArray": "B",
    "Int16Array": "h",
    "Uint16Array": "H",
    "Int32Array": "i",
    "Uint32Array": "I",
    "Float32Array": "f",
    "Float64Array": "d",
    "BigInt64Array": "q",
    "BigUint64Array": "Q",
}
_KINDS = {code: kind for kind, code in _TYPECODES.items() if kind != "Uint8ClampedArray"}
_KINDS["B"] = "Uint8Array"


def _encode(v: Any, seen: set[int] | None = None) -> Any:
    """Python value → tagged JSON (mirrors the guest's codec.rs)."""
    if v is None or isinstance(v, (bool, str)):
        return v
    if v is UNDEFINED:
        return {"$t": "u"}
    if isinstance(v, int):
        return v if -_MAX_SAFE <= v <= _MAX_SAFE else {"$t": "bi", "v": str(v)}
    if isinstance(v, float):
        if math.isnan(v):
            return {"$t": "n", "v": "NaN"}
        if math.isinf(v):
            return {"$t": "n", "v": "Infinity" if v > 0 else "-Infinity"}
        if v == 0 and math.copysign(1.0, v) < 0:
            return {"$t": "n", "v": "-0"}
        return v
    if isinstance(v, (bytes, bytearray, memoryview)):
        return {"$t": "b", "v": base64.b64encode(bytes(v)).decode("ascii")}
    if isinstance(v, array.array):
        kind = _KINDS.get(v.typecode)
        if kind is None:
            raise TypeError(f"an array.array of type {v.typecode!r} cannot cross the sandbox boundary")
        return {"$t": "b", "k": kind, "v": base64.b64encode(v.tobytes()).decode("ascii")}
    if isinstance(v, _dt.datetime):
        # Naive datetimes are taken as UTC: the sandbox has no local time zone.
        aware = v if v.tzinfo is not None else v.replace(tzinfo=_dt.timezone.utc)
        return {"$t": "d", "v": aware.timestamp() * 1000}
    if isinstance(v, JsRegExp):
        return {"$t": "re", "src": v.source, "flags": v.flags}
    if isinstance(v, BaseException):
        e: dict[str, Any] = {"$t": "e", "name": _error_name(v), "message": _error_message(v)}
        code = getattr(v, "code", None)
        if code is not None:
            e["code"] = _encode(code, seen)
        return e
    if seen is None:
        seen = set()
    if id(v) in seen:
        raise TypeError("a circular structure cannot cross the sandbox boundary")
    seen.add(id(v))
    try:
        if isinstance(v, JsMap) or (isinstance(v, Mapping) and not all(isinstance(k, str) for k in v)):
            return {"$t": "m", "v": [[_encode(k, seen), _encode(x, seen)] for k, x in v.items()]}
        if isinstance(v, Mapping):
            return _object({k: _encode(x, seen) for k, x in v.items()})
        if isinstance(v, (list, tuple)):
            return [_encode(x, seen) for x in v]
        if isinstance(v, (set, frozenset)):
            return {"$t": "s", "v": [_encode(x, seen) for x in v]}
        if dataclasses.is_dataclass(v) and not isinstance(v, type):
            return _object({f.name: _encode(getattr(v, f.name), seen) for f in dataclasses.fields(v)})
    finally:
        seen.discard(id(v))
    kind = "function" if callable(v) else type(v).__name__
    raise TypeError(f"a {kind} cannot cross the sandbox boundary")


def _object(out: dict[str, Any]) -> dict[str, Any]:
    # An object with its own "$t" key is wrapped so it isn't read as a tag.
    return {"$t": "o", "v": out} if "$t" in out else out


def _decode(j: Any) -> Any:
    """Tagged JSON → Python value."""
    if isinstance(j, list):
        return [_decode(x) for x in j]
    if isinstance(j, float):
        # JS has one number type; whole numbers come back as int.
        return int(j) if j.is_integer() and abs(j) <= _MAX_SAFE else j
    if not isinstance(j, dict):
        return j
    t = j.get("$t")
    if t == "u":
        return None
    if t == "n":
        return {"Infinity": math.inf, "-Infinity": -math.inf, "-0": -0.0}.get(j["v"], math.nan)
    if t == "bi":
        return int(j["v"])
    if t == "d":
        if j.get("v") is None:
            return None  # an invalid Date
        try:
            return _dt.datetime.fromtimestamp(j["v"] / 1000, tz=_dt.timezone.utc)
        except (OverflowError, OSError, ValueError):
            return j["v"]  # outside datetime's range: milliseconds since the epoch
    if t == "b":
        raw = base64.b64decode(j["v"])
        code = _TYPECODES.get(j.get("k") or "Uint8Array")
        if code is None:
            return raw
        out = array.array(code)
        out.frombytes(raw)
        return out
    if t == "m":
        pairs = [(_decode(k), _decode(x)) for k, x in j["v"]]
        try:
            return JsMap(pairs)
        except TypeError:
            return pairs  # keys Python can't hash, like objects or arrays
    if t == "s":
        items = [_decode(x) for x in j["v"]]
        try:
            return set(items)
        except TypeError:
            return items
    if t == "re":
        return JsRegExp(j["src"], j.get("flags", ""))
    if t == "e":
        code = _decode(j["code"]) if "code" in j else None
        return JsError(j.get("message", ""), name=j.get("name", "Error"), code=code, stack=j.get("stack"))
    if t == "o":
        j = j["v"]
    return {k: _decode(x) for k, x in j.items()}


def _error_name(e: BaseException) -> str:
    return e.name if isinstance(e, JsError) else type(e).__name__


def _error_message(e: BaseException) -> str:
    return e.message if isinstance(e, JsError) else str(e)


# ---- host function context -----------------------------------------------------


@dataclass(frozen=True)
class Resume:
    """How an interrupted call was resolved: the ``value`` passed with ``resume_with``,
    and the ``payload`` the call interrupted with."""

    resolution: Any
    payload: Any = None


class _Interrupt(BaseException):
    # A BaseException, so a host function's `except Exception:` can't swallow it.
    def __init__(self, payload: Any) -> None:
        super().__init__("modebox interrupt")
        self.payload = payload


@dataclass(frozen=True)
class HostFunctionContext:
    """Information about the current host call, from ``get_host_function_context()``."""

    call_id: int
    host_function_name: str
    resume: Resume | None = None

    def interrupt(self, payload: Any = None) -> NoReturn:
        """Pause the run here. A dict payload's ``"kind"`` names the interruption.

        The run resumes from a continuation token, and this host function runs
        again with ``resume`` set to the resolution.
        """
        raise _Interrupt(payload)


_current: contextvars.ContextVar[HostFunctionContext] = contextvars.ContextVar("modebox_host_function")


def get_host_function_context() -> HostFunctionContext:
    """Inside a host function: information about the call, and ``interrupt()`` to pause the run."""
    ctx = _current.get(None)
    if ctx is None:
        raise RuntimeError("get_host_function_context() can only be called inside a host function")
    return ctx


_APPROVAL = "__modebox_approval__"


def require_approval(fn: Callable[..., Any]) -> Callable[..., Any]:
    """Mark a host function as needing approval before every call. Works as a decorator."""
    try:
        setattr(fn, _APPROVAL, True)
        return fn
    except (AttributeError, TypeError):  # bound methods, builtins
        wrapper = functools.wraps(fn)(lambda *a, **k: fn(*a, **k))
        setattr(wrapper, _APPROVAL, True)
        return wrapper


# ---- options and results ---------------------------------------------------------


@dataclass(frozen=True)
class Limits:
    timeout_ms: int | None = None
    #: Deterministic CPU budget in wasm fuel units (cumulative across resumes).
    fuel: int | None = None
    memory_limit_bytes: int | None = None
    max_host_calls: int | None = None
    #: Host calls running at once; extra calls queue.
    max_concurrency: int | None = None
    max_value_bytes: int | None = None
    max_log_bytes: int | None = None


@dataclass(frozen=True)
class ToolSchema:
    description: str | None = None
    input_schema: Mapping[str, Any] | None = None
    output_schema: Mapping[str, Any] | None = None


@dataclass(frozen=True)
class Resolution:
    """How to resolve one interruption when resuming. See ``approve``, ``deny``, ``resume_with`` and ``return_value``."""

    interruption_id: int
    action: Literal["approve", "deny", "resume", "return"]
    value: Any = None
    message: str | None = None


def approve(interruption_id: int) -> Resolution:
    """Run the host call that was waiting for approval."""
    return Resolution(interruption_id, "approve")


def deny(interruption_id: int, message: str | None = None) -> Resolution:
    """Refuse the call: it throws in the sandbox."""
    return Resolution(interruption_id, "deny", message=message)


def resume_with(interruption_id: int, value: Any) -> Resolution:
    """Run an interrupted host function again, with ``get_host_function_context().resume.resolution`` set to ``value``."""
    return Resolution(interruption_id, "resume", value)


def return_value(interruption_id: int, value: Any) -> Resolution:
    """Skip the host function: the call returns ``value`` in the sandbox."""
    return Resolution(interruption_id, "return", value)


@dataclass(frozen=True)
class LogEntry:
    level: str
    message: str


@dataclass(frozen=True)
class Stats:
    wall_ms: float
    guest_ms: float
    instantiate_ms: float
    fuel_used: int | None
    host_calls: int
    memory_bytes: int
    continuation_bytes: int | None


@dataclass(frozen=True)
class Interruption:
    id: int
    host_function_name: str
    kind: str
    args: list[Any]
    payload: Any


class RunError(Exception):
    """The sandboxed code failed: an uncaught exception, a limit, or an invalid continuation."""

    def __init__(self, error: Mapping[str, Any], logs: list[LogEntry], stats: Stats) -> None:
        self.name: str = error.get("name") or "Error"
        self.message: str = error.get("message") or ""
        super().__init__(f"{self.name}: {self.message}")
        #: What failed, e.g. ``"Uncaught"``, ``"Timeout"``, ``"InvalidContinuation"``.
        self.kind: str = error["kind"]
        #: The uncaught error's own ``code`` when it had one (say, from a host function),
        #: else ``kind`` as a constant, e.g. ``"RUN_TIMEOUT"``.
        self.code = error.get("code") or "RUN_" + re.sub(r"([a-z])([A-Z])", r"\1_\2", self.kind).upper()
        self.sandbox_stack: str | None = error.get("stack")
        self.logs = logs
        self.stats = stats


@dataclass
class RunResult:
    """``status`` is ``"completed"`` (see ``value``), ``"interrupted"`` (see ``interruptions`` and
    ``continuation``) or ``"failed"`` (see ``error``; only with ``throw_on_error=False``)."""

    status: Literal["completed", "interrupted", "failed"]
    logs: list[LogEntry]
    stats: Stats
    value: Any = None
    interruptions: list[Interruption] = field(default_factory=list)
    continuation: str | None = None
    error: RunError | None = None


# ---- plumbing ----------------------------------------------------------------------

HostFunctions = Mapping[str, Any]  # name → callable, or a nested mapping of them


def _flatten(fns: HostFunctions, prefix: str = "", out: dict[str, Callable[..., Any]] | None = None) -> dict[str, Callable[..., Any]]:
    out = {} if out is None else out
    for k, v in fns.items():
        path = f"{prefix}.{k}" if prefix else str(k)
        if callable(v):
            out[path] = v
        elif isinstance(v, Mapping):
            _flatten(v, path, out)
    return out


@functools.lru_cache(maxsize=256)
def _glob(pattern: str) -> re.Pattern[str]:
    return re.compile(".*".join(re.escape(part) for part in pattern.split("*")))


def _matches(patterns: Iterable[str], name: str) -> bool:
    return any(_glob(p).fullmatch(name) for p in patterns)


def _schema(s: ToolSchema | Mapping[str, Any] | None) -> dict[str, Any]:
    if s is None:
        return {}
    if isinstance(s, ToolSchema):
        s = {"description": s.description, "inputSchema": s.input_schema, "outputSchema": s.output_schema}
    out = {
        "description": s.get("description"),
        "inputSchema": s.get("inputSchema", s.get("input_schema")),
        "outputSchema": s.get("outputSchema", s.get("output_schema")),
    }
    return {k: v for k, v in out.items() if v is not None}


def _tool_specs(
    fns: Mapping[str, Callable[..., Any]],
    approve: Iterable[str],
    schemas: Mapping[str, Any],
    sources: Iterable[str],
    sinks: Iterable[str],
) -> str:
    approve, sources, sinks = list(approve), list(sources), list(sinks)
    return json.dumps(
        [
            {
                "name": name,
                "approval": bool(getattr(fn, _APPROVAL, False)) or _matches(approve, name),
                "source": _matches(sources, name),
                "sink": _matches(sinks, name),
                **_schema(schemas.get(name)),
            }
            for name, fn in fns.items()
        ]
    )


def _limits(*layers: Limits | Mapping[str, Any] | None) -> dict[str, int]:
    out: dict[str, int] = {}
    for layer in layers:
        if layer is None:
            continue
        items = dataclasses.asdict(layer) if isinstance(layer, Limits) else dict(layer)
        out.update({k: int(v) for k, v in items.items() if v is not None})
    return out


def _native_options(limits: dict[str, int], seed: Any, virtual_time: Any, mode: Any, filename: Any, typescript: Any) -> str:
    if virtual_time is True:
        virtual_time = time.time() * 1000
    elif isinstance(virtual_time, _dt.datetime):
        virtual_time = _encode(virtual_time)["v"]
    elif virtual_time is False:
        virtual_time = None
    o = {
        "timeoutMs": limits.get("timeout_ms"),
        "fuel": limits.get("fuel"),
        "memoryLimitBytes": limits.get("memory_limit_bytes"),
        "maxHostCalls": limits.get("max_host_calls"),
        "maxConcurrency": limits.get("max_concurrency"),
        "maxValueBytes": limits.get("max_value_bytes"),
        "maxLogBytes": limits.get("max_log_bytes"),
        "seed": seed.hex() if isinstance(seed, (bytes, bytearray)) else seed,
        "virtualTimeMs": virtual_time,
        "mode": mode,
        "filename": filename,
        "typescript": typescript,
    }
    return json.dumps({k: v for k, v in o.items() if v is not None})


def _resolution(r: Resolution | Mapping[str, Any]) -> dict[str, Any]:
    if isinstance(r, Resolution):
        return {"id": r.interruption_id, "action": r.action, "value": _encode(r.value), "message": r.message}
    value = r.get("value")
    action = r.get("action") or ("deny" if value is False else "approve" if value is True else "resume")
    rid = r.get("interruption_id", r.get("interruptionId", r.get("id")))
    if rid is None:
        raise TypeError("a resolution needs an interruption_id")
    return {"id": int(rid), "action": action, "value": _encode(value), "message": r.get("message")}


def _make_dispatch(fns: Mapping[str, Callable[..., Any]], on_call: Callable[[str, int], Any] | None) -> Callable[[str], Any]:
    async def dispatch(msg: str) -> str:
        m = json.loads(msg)
        name, call_id = m["name"], m["id"]
        resume = m.get("resume")
        payload = _decode(m["payload"]) if "payload" in m else None
        ctx = HostFunctionContext(call_id, name, None if resume is None else Resume(_decode(resume), payload))
        token = _current.set(ctx)
        try:
            if on_call is not None:
                on_call(name, call_id)
            value = fns[name](*_decode(m["args"]))
            if inspect.isawaitable(value):
                value = await value
            return json.dumps({"ok": True, "v": _encode(value)})
        except _Interrupt as i:
            kind = i.payload.get("kind", "interrupt") if isinstance(i.payload, Mapping) else "interrupt"
            return json.dumps({"interrupt": {"kind": str(kind), "payload": _encode(i.payload)}})
        except Exception as e:
            code = getattr(e, "code", None)
            err = {"name": _error_name(e), "message": _error_message(e), "code": code if isinstance(code, str) else None}
            return json.dumps({"ok": False, "e": err})
        finally:
            _current.reset(token)

    return dispatch


def _shape(out: str) -> RunResult:
    r = json.loads(out)
    logs = [LogEntry(entry["level"], entry["message"]) for entry in r["logs"]]
    s = r["stats"]
    stats = Stats(
        wall_ms=s["wallMs"],
        guest_ms=s["guestMs"],
        instantiate_ms=s["instantiateMs"],
        fuel_used=s["fuelUsed"],
        host_calls=s["hostCalls"],
        memory_bytes=s["memoryBytes"],
        continuation_bytes=s["continuationBytes"],
    )
    if r["status"] == "completed":
        return RunResult("completed", logs, stats, value=_decode(r["value"]))
    if r["status"] == "suspended":
        interruptions = [
            Interruption(i["id"], i["tool"], i["kind"], _decode(i["args"]), _decode(i["payload"])) for i in r["interrupts"]
        ]
        return RunResult("interrupted", logs, stats, interruptions=interruptions, continuation=r["continuation"])
    return RunResult("failed", logs, stats, error=RunError(r["error"], logs, stats))


# ---- runner --------------------------------------------------------------------------


class Runner:
    """An engine plus defaults. Create one per process and reuse it; runs can overlap."""

    def __init__(
        self,
        *,
        continuation_secret: str | None = None,
        metering: bool | None = None,
        cache_dir: str | os.PathLike[str] | None = None,
        continuation_ttl_ms: int | None = None,
        continuations_dir: str | os.PathLike[str] | None = None,
        max_instances: int | None = None,
        limits: Limits | Mapping[str, Any] | None = None,
        approve: Iterable[str] = (),
        sources: Iterable[str] = (),
        sinks: Iterable[str] = (),
    ) -> None:
        """
        Args:
            continuation_secret: key for sealing continuations (defaults to ``$MODEBOX_SECRET`` /
                ``$RUN_CONTINUATION_SECRET``, else random per process).
            metering: deterministic fuel accounting (default on).
            continuations_dir: directory recording used continuations, shared by every process that
                resumes the same tokens (default: in memory, so single-use only within this process).
            approve: glob patterns of host function paths that need approval (``"bank.*"``).
            sources: host functions that return private data. Once a run calls one, calls to
                ``sinks`` need approval.
            sinks: host functions that can send data out (see ``sources``).
        """
        secret = continuation_secret or os.environ.get("MODEBOX_SECRET") or os.environ.get("RUN_CONTINUATION_SECRET")
        config = {
            "secret": secret,
            "metering": metering,
            "cacheDir": os.fspath(cache_dir) if cache_dir is not None else None,
            "continuationTtlMs": continuation_ttl_ms,
            "continuationsDir": os.fspath(continuations_dir) if continuations_dir is not None else None,
            "maxInstances": max_instances,
        }
        self._engine: int | None = _native.create_engine(json.dumps({k: v for k, v in config.items() if v is not None}))
        self._limits = limits
        self._approve = list(approve)
        self._sources = list(sources)
        self._sinks = list(sinks)

    async def run(
        self,
        source: str | None = None,
        *,
        host_functions: HostFunctions | None = None,
        limits: Limits | Mapping[str, Any] | None = None,
        approve: Iterable[str] | None = None,
        sources: Iterable[str] | None = None,
        sinks: Iterable[str] | None = None,
        schemas: Mapping[str, ToolSchema | Mapping[str, Any]] | None = None,
        continuation: str | None = None,
        resolutions: Iterable[Resolution | Mapping[str, Any]] = (),
        seed: str | bytes | None = None,
        virtual_time: float | bool | _dt.datetime | None = None,
        mode: Literal["auto", "script", "function"] | None = None,
        filename: str | None = None,
        typescript: bool | None = None,
        on_log: Callable[[LogEntry], Any] | None = None,
        on_call: Callable[[str, int], Any] | None = None,
        throw_on_error: bool = True,
    ) -> RunResult:
        """Run ``source``, or resume a suspended run when ``continuation`` is given.

        Returns a ``RunResult`` with ``status`` ``"completed"`` or ``"interrupted"``. Raises
        ``RunError`` when the code fails, unless ``throw_on_error=False``.

        Args:
            seed: 64 hex characters or 32 bytes; makes ``Math.random`` and ``crypto`` reproducible.
            virtual_time: timers fire instantly; a start time in ms since the epoch, a datetime,
                or ``True`` for now.
        """
        if self._engine is None:
            raise RuntimeError("this Runner is closed")
        fns = _flatten(host_functions or {})
        tools = _tool_specs(
            fns,
            self._approve if approve is None else approve,
            schemas or {},
            self._sources if sources is None else sources,
            self._sinks if sinks is None else sinks,
        )
        dispatch = _make_dispatch(fns, on_call)
        opts = _native_options(_limits(self._limits, limits), seed, virtual_time, mode, filename, typescript)
        if continuation is not None:
            res = json.dumps([_resolution(r) for r in resolutions])
            out = await _native.resume(self._engine, continuation, res, tools, opts, dispatch)
        elif source is None:
            raise TypeError("run() needs source code, or a continuation to resume")
        else:
            out = await _native.run(self._engine, source, tools, opts, dispatch)
        result = _shape(out)
        if on_log is not None:
            for entry in result.logs:
                on_log(entry)
        if result.error is not None and throw_on_error:
            raise result.error
        return result

    def run_sync(self, source: str | None = None, **options: Any) -> RunResult:
        """``run()`` for code without an event loop. Takes the same arguments."""
        return asyncio.run(self.run(source, **options))

    def declarations(
        self, host_functions: HostFunctions, schemas: Mapping[str, ToolSchema | Mapping[str, Any]] | None = None
    ) -> str:
        """TypeScript declarations for a set of host functions (for the model's prompt)."""
        return declarations(host_functions, schemas)

    def close(self) -> None:
        if self._engine is not None:
            _native.drop_engine(self._engine)
            self._engine = None

    def __enter__(self) -> Runner:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()


def declarations(host_functions: HostFunctions, schemas: Mapping[str, ToolSchema | Mapping[str, Any]] | None = None) -> str:
    """TypeScript declarations for a set of host functions (for the model's prompt)."""
    return _native.declarations(_tool_specs(_flatten(host_functions), (), schemas or {}, (), ()))


_shared: Runner | None = None


def _default_runner() -> Runner:
    global _shared
    if _shared is None:
        _shared = Runner()
    return _shared


async def run(source: str | None = None, **options: Any) -> RunResult:
    """Run with a process-wide default ``Runner``. Takes the same arguments as ``Runner.run``."""
    return await _default_runner().run(source, **options)


def run_sync(source: str | None = None, **options: Any) -> RunResult:
    """``run()`` for code without an event loop. Takes the same arguments as ``Runner.run``."""
    return _default_runner().run_sync(source, **options)
