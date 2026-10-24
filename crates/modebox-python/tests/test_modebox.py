import asyncio
import datetime as dt
import math
import shutil
import tempfile
import unittest

import modebox
from modebox import RunError, Runner, get_host_function_context, require_approval

SECRET = "k" * 32


class ModeboxTest(unittest.IsolatedAsyncioTestCase):
    async def test_runs_code_with_host_functions(self):
        r = await modebox.run("return await tools.double(21);", host_functions={"tools": {"double": lambda x: x * 2}})
        self.assertEqual(r.status, "completed")
        self.assertEqual(r.value, 42)

    async def test_async_host_functions_run_on_the_callers_loop(self):
        loop = asyncio.get_running_loop()

        async def get(uid):
            self.assertIs(asyncio.get_running_loop(), loop)
            await asyncio.sleep(0)
            return {"id": uid, "name": "Ada"}

        r = await modebox.run("const [a, b] = await Promise.all([users.get(1), users.get(2)]); a.name + b.id", host_functions={"users": {"get": get}})
        self.assertEqual(r.value, "Ada2")

    async def test_rich_values_cross_both_ways(self):
        # `input` isn't defined: errors are RunErrors.
        with self.assertRaises(RunError):
            await modebox.run("const v = await t.echo(input); v", host_functions={"t": {"echo": lambda v: v}})
        made = {
            "d": dt.datetime.fromtimestamp(0.005, tz=dt.timezone.utc),
            "m": modebox.JsMap({"k": 1}),
            "n": 10**20,
            "b": bytes([7, 8]),
        }
        ok = await modebox.run(
            'const v = await t.make(); [v.d instanceof Date, v.d.getTime(), v.m.get("k"), typeof v.n, v.b[1]]',
            host_functions={"t": {"make": lambda: made}},
        )
        self.assertEqual(ok.value, [True, 5, 1, "bigint", 8])
        back = await modebox.run('[new Date(5), new Map([[1, "a"]]), 10n ** 20n, new Uint8Array([1, 2]), -0, NaN, undefined, null, new Set([1]), /a+/gi, 2.5]')
        d, m, big, b, neg_zero, nan, undef, null, s, rx, half = back.value
        self.assertEqual(d, dt.datetime(1970, 1, 1, 0, 0, 0, 5000, tzinfo=dt.timezone.utc))
        self.assertIsInstance(m, modebox.JsMap)
        self.assertEqual(m, {1: "a"})
        self.assertEqual(big, 10**20)
        self.assertEqual(b, b"\x01\x02")
        self.assertTrue(neg_zero == 0 and math.copysign(1, neg_zero) < 0)
        self.assertTrue(math.isnan(nan))
        self.assertIsNone(undef)
        self.assertIsNone(null)
        self.assertEqual(s, {1})
        self.assertEqual(rx, modebox.JsRegExp("a+", "gi"))
        self.assertEqual(half, 2.5)

    async def test_values_that_cannot_cross_are_errors_in_the_sandbox(self):
        r = await modebox.run("try { await t.bad() } catch (e) { e.message }", host_functions={"t": {"bad": lambda: object()}})
        self.assertIn("cannot cross the sandbox boundary", r.value)

    async def test_console_output_is_captured_not_printed(self):
        r = await modebox.run('console.log("hello", { a: 1 }); 1')
        self.assertEqual(r.logs, [modebox.LogEntry("log", "hello { a: 1 }")])

    async def test_failures_raise_run_error_with_kind_and_code(self):
        with self.assertRaises(RunError) as c:
            await modebox.run("null.x")
        self.assertEqual(c.exception.kind, "Uncaught")
        self.assertEqual(c.exception.name, "TypeError")
        with self.assertRaises(RunError) as c:
            await modebox.run("while(true){}", limits=modebox.Limits(timeout_ms=100))
        self.assertEqual(c.exception.code, "RUN_TIMEOUT")
        r = await modebox.run("null.x", throw_on_error=False)
        self.assertEqual(r.status, "failed")
        self.assertIsInstance(r.error, RunError)

    async def test_cpu_timeout_stops_busy_code(self):
        with self.assertRaises(RunError) as c:
            await modebox.run("while (true) {}", limits=modebox.Limits(cpu_timeout_ms=100, timeout_ms=10_000))
        self.assertEqual(c.exception.kind, "CpuTimeout")
        self.assertEqual(c.exception.code, "RUN_CPU_TIMEOUT")

    async def test_an_uncaught_error_keeps_its_own_code(self):
        class QuotaError(Exception):
            code = "E_QUOTA"

        def charge():
            raise QuotaError("quota exceeded")

        with self.assertRaises(RunError) as c:
            await modebox.run("await billing.charge()", host_functions={"billing": {"charge": charge}})
        self.assertEqual(c.exception.kind, "Uncaught")
        self.assertEqual(c.exception.code, "E_QUOTA")
        with self.assertRaises(RunError) as c:
            await modebox.run("null.x")
        self.assertEqual(c.exception.code, "RUN_UNCAUGHT")

    async def test_host_function_exceptions_are_catchable_in_the_sandbox(self):
        def boom():
            raise ValueError("bad input")

        r = await modebox.run("try { await t.boom() } catch (e) { [e.name, e.message] }", host_functions={"t": {"boom": boom}})
        self.assertEqual(r.value, ["ValueError", "bad input"])

    async def test_interrupt_suspends_and_resume_reinvokes_with_the_resolution(self):
        runner = Runner(continuation_secret=SECRET)
        calls = 0

        def login():
            nonlocal calls
            calls += 1
            ctx = get_host_function_context()
            if ctx.resume is None:
                ctx.interrupt({"kind": "sign_in", "url": "https://example.test"})
            return f"token:{ctx.resume.resolution}:{ctx.resume.payload['url']}"

        host_functions = {"auth": {"login": login}}
        a = await runner.run("const r = Math.random(); const t = await auth.login(); [t, r]", host_functions=host_functions)
        self.assertEqual(a.status, "interrupted")
        self.assertEqual(a.interruptions[0].kind, "sign_in")
        self.assertEqual(a.interruptions[0].payload["url"], "https://example.test")
        resolutions = [modebox.resume_with(a.interruptions[0].id, "abc")]
        b = await runner.run(host_functions=host_functions, continuation=a.continuation, resolutions=resolutions)
        self.assertEqual(b.value[0], "token:abc:https://example.test")
        self.assertEqual(calls, 2)
        # single-use
        with self.assertRaises(RunError) as c:
            await runner.run(host_functions=host_functions, continuation=a.continuation, resolutions=resolutions)
        self.assertEqual(c.exception.kind, "InvalidContinuation")

    async def test_except_exception_does_not_swallow_an_interrupt(self):
        def careful():
            try:
                get_host_function_context().interrupt({"kind": "ask"})
            except Exception:
                return "swallowed"

        r = await Runner(continuation_secret=SECRET).run("await t.careful()", host_functions={"t": {"careful": careful}})
        self.assertEqual(r.status, "interrupted")

    async def test_approval_policies_pause_before_the_host_function_runs(self):
        runner = Runner(continuation_secret=SECRET)
        sent = []

        @require_approval
        def send(amount):
            sent.append(amount)
            return "ok"

        host_functions = {"bank": {"send": send}}
        a = await runner.run("await bank.send(5)", host_functions=host_functions)
        self.assertEqual(a.status, "interrupted")
        self.assertEqual(sent, [])
        b = await runner.run(host_functions=host_functions, continuation=a.continuation, resolutions=[modebox.approve(a.interruptions[0].id)])
        self.assertEqual(b.value, "ok")
        self.assertEqual(sent, [5])

    async def test_approve_patterns_and_deny(self):
        runner = Runner(continuation_secret=SECRET, approve=["bank.*"])
        host_functions = {"bank": {"send": lambda amount: "sent"}}
        a = await runner.run("try { await bank.send(1) } catch (e) { 'refused: ' + e.message }", host_functions=host_functions)
        self.assertEqual(a.status, "interrupted")
        b = await runner.run(host_functions=host_functions, continuation=a.continuation, resolutions=[modebox.deny(a.interruptions[0].id, "over budget")])
        self.assertTrue(b.value.startswith("refused: "), b.value)
        self.assertIn("over budget", b.value)

    async def test_a_shared_continuations_dir_makes_tokens_single_use_across_runners(self):
        d = tempfile.mkdtemp(prefix="modebox-used-")
        try:
            # Two runners with the same key and directory stand in for two processes.
            a, b = (Runner(continuation_secret=SECRET, continuations_dir=d) for _ in range(2))
            host_functions = {"bank": {"send": require_approval(lambda: "ok")}}
            s = await a.run("await bank.send()", host_functions=host_functions)
            resolutions = [modebox.approve(s.interruptions[0].id)]
            self.assertEqual((await b.run(host_functions=host_functions, continuation=s.continuation, resolutions=resolutions)).value, "ok")
            with self.assertRaises(RunError) as c:
                await a.run(host_functions=host_functions, continuation=s.continuation, resolutions=resolutions)
            self.assertEqual(c.exception.kind, "InvalidContinuation")
        finally:
            shutil.rmtree(d)

    async def test_a_sink_after_a_source_pauses_for_approval(self):
        runner = Runner(continuation_secret=SECRET, sources=["mail.*"], sinks=["http.post"])
        sent = []

        def post(v):
            sent.append(v)
            return "ok"

        host_functions = {"mail": {"read": lambda: "secret"}, "http": {"post": post}}
        free = await runner.run('await http.post("hi")', host_functions=host_functions)
        self.assertEqual(free.value, "ok")
        a = await runner.run("await http.post(await mail.read())", host_functions=host_functions)
        self.assertEqual(a.status, "interrupted")
        self.assertRegex(a.interruptions[0].payload["reason"], r"mail\.read")
        self.assertEqual(sent, ["hi"])

    async def test_seed_and_virtual_time_are_deterministic(self):
        code = "[Math.random(), Date.now()]"
        a = await modebox.run(code, seed="ab" * 32, virtual_time=1000)
        b = await modebox.run(code, seed=bytes.fromhex("ab" * 32), virtual_time=dt.datetime.fromtimestamp(1, tz=dt.timezone.utc))
        self.assertEqual(a.value, b.value)
        self.assertEqual(a.value[1], 1000)

    async def test_typescript(self):
        r = await modebox.run("const n: number = 2; n * 3", typescript=True)
        self.assertEqual(r.value, 6)


class SyncTest(unittest.TestCase):
    def test_run_sync(self):
        self.assertEqual(modebox.run_sync("1 + 1").value, 2)

    def test_declarations_for_prompts(self):
        with Runner() as runner:
            d = runner.declarations(
                {"gh": {"issues": {"list": list}}},
                {"gh.issues.list": modebox.ToolSchema(description="List issues", input_schema={"type": "object", "properties": {"repo": {"type": "string"}}, "required": ["repo"]})},
            )
        self.assertRegex(d, r"declare const gh")
        self.assertRegex(d, r"list\(input: \{ repo: string \}\): Promise<unknown>;")

    def test_get_host_function_context_outside_a_host_function(self):
        with self.assertRaises(RuntimeError):
            get_host_function_context()


if __name__ == "__main__":
    unittest.main()
