"""Token-free tests of the official-library boundary and production RPC script."""
import asyncio
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("hermes.py")
spec = importlib.util.spec_from_file_location("managed_hermes", SCRIPT)
hermes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hermes)


class Registry:
    def __init__(self):
        self.tools = {}

    def get_schema(self, name):
        return self.tools.get(name, {}).get("schema")

    def register(self, **tool):
        self.tools[tool["name"]] = tool


class Host:
    def __init__(self):
        self.calls = []
        self.events = []
        self.finished = threading.Event()

    def request_sync(self, method, value):
        self.calls.append((method, value))
        return {"accepted": True, "echo": value["arguments"]}

    def notify(self, method, value):
        self.events.append((method, value))
        if method == "complete":
            self.finished.set()


class HermesTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.registry = Registry()
        self.host = Host()
        self.input = {"provider": "hermes", "cwd": self.tmp.name, "model": "fixture", "effort": "high", "tools": [
            {"name": "workflow_result", "description": "Submit", "inputSchema": {"type": "object", "properties": {"result": {"type": "string"}}, "required": ["result"]}},
            {"name": "workflow_spawn", "description": "Spawn", "inputSchema": {"type": "object"}},
        ], "options": {"stateDir": self.tmp.name, "source": self.tmp.name, "profileHome": self.tmp.name}}
        registry = self.registry

        class Agent:
            calls = 0

            def __init__(self, *, enabled_toolsets, skip_context_files, skip_memory, skip_background_review, session_db, checkpoints_enabled, load_soul_identity, max_iterations, run_budget_seconds, **kwargs):
                assert enabled_toolsets == [hermes.TOOLSET]
                assert skip_context_files and skip_memory and skip_background_review
                assert session_db is None and not checkpoints_enabled and not load_soul_identity
                assert max_iterations == 64 and run_budget_seconds == 3600
                assert kwargs["credential_pool"] is None and kwargs["fallback_model"] is None
                self.options = kwargs
                self.session_id = kwargs["session_id"]
                self.tools = [{"type": "function", "function": tool["schema"]} for tool in registry.tools.values()]
                self.valid_tool_names = set(registry.tools)
                self.stopped = threading.Event()
                self.closed = False

            def run_conversation(self, *, user_message, conversation_history):
                Agent.calls += 1
                if user_message == "wait":
                    self.stopped.wait(5)
                    return {"interrupted": True}
                for name in registry.tools:
                    registry.tools[name]["handler"]({"result": name, "attempt_id": "untrusted"})
                return {"final_response": "coordinated", "completed": True, "messages": [{"role": "assistant", "content": "coordinated"}]}

            def interrupt(self, **kwargs):
                assert kwargs["hard_cancel"] is True
                self.stopped.set()

            def close(self):
                self.closed = True

        self.Agent = Agent
        self.runtime = hermes.Runtime(self.host, lambda _: (Agent, registry, {"provider": "custom", "api_mode": "chat_completions", "api_key": "fake", "base_url": "http://127.0.0.1:1"}, "fixture"))

    async def finish(self):
        for _ in range(500):
            if self.host.finished.is_set():
                return
            await asyncio.sleep(0.01)
        self.fail("Managed turn did not complete")

    async def test_exact_catalog_and_captured_host_calls_coordinate_without_tokens(self):
        ready = await self.runtime.initialize(self.input)
        self.assertEqual(ready["tools"], ["workflow_result", "workflow_spawn"])
        self.assertEqual(ready["isolation"], {"nativeTools": [], "nativeFanout": False, "ambientConfig": False})
        self.assertTrue(self.runtime.agent.options["quiet_mode"])
        self.assertFalse(self.runtime.agent.options["save_trajectories"])
        await self.runtime.start({"turnId": "turn", "prompt": "coordinate"})
        await self.finish()
        self.assertEqual([value["name"] for _, value in self.host.calls], ready["tools"])
        self.assertTrue(all(set(value) == {"name", "arguments"} for _, value in self.host.calls))
        self.assertEqual(self.host.events[-1][1]["status"], "success")
        self.assertEqual(self.host.events[-2], ("text", {"turnId": "turn", "text": "coordinated"}))
        await self.runtime.dispose()
        self.assertTrue(self.runtime.agent.closed)

    async def test_unexpected_native_tool_blocks_inference_and_schema_changes_fence_tools(self):
        await self.runtime.initialize(self.input)
        self.runtime.agent.tools.append({"function": {"name": "terminal", "parameters": {"type": "object"}}})
        with self.assertRaisesRegex(ValueError, "exactly"):
            await self.runtime.start({"turnId": "turn", "prompt": "forbidden"})
        self.assertEqual(self.Agent.calls, 0)
        self.assertEqual(self.host.calls, [])

    async def test_concurrent_turn_rejected_and_cancel_closes_host_authority(self):
        await self.runtime.initialize(self.input)
        await self.runtime.start({"turnId": "turn", "prompt": "wait"})
        with self.assertRaisesRegex(ValueError, "concurrent"):
            await self.runtime.start({"turnId": "other", "prompt": "work"})
        await self.runtime.cancel()
        with self.assertRaisesRegex(ValueError, "not active"):
            self.registry.tools["workflow_result"]["handler"]({})
        await self.finish()
        self.assertEqual(self.host.events[-1][1]["status"], "cancelled")
        await self.runtime.dispose()

    async def test_missing_public_isolation_contract_fails_before_inference(self):
        self.runtime.api_loader = lambda _: (lambda **kwargs: None, self.registry, {}, "fixture")
        with self.assertRaisesRegex(ValueError, "isolation constructor"):
            await self.runtime.initialize(self.input)
        self.assertEqual(self.Agent.calls, 0)

    async def test_partial_or_budget_exhausted_turn_never_reports_success(self):
        await self.runtime.initialize(self.input)
        self.runtime.agent.run_conversation = lambda **kwargs: {"final_response": "partial", "completed": False, "partial": True}
        await self.runtime.start({"turnId": "partial", "prompt": "work"})
        await self.finish()
        self.assertEqual(self.host.events[-1][1]["status"], "error")
        self.assertFalse(any(method == "text" for method, _ in self.host.events))

    async def test_duplicate_catalog_and_tool_collision_fail_closed(self):
        self.input["tools"].append(self.input["tools"][0])
        with self.assertRaisesRegex(ValueError, "duplicate"):
            await self.runtime.initialize(self.input)
        self.input["tools"].pop()
        self.registry.tools["workflow_result"] = {"schema": {"name": "workflow_result"}}
        with self.assertRaisesRegex(ValueError, "collides"):
            await self.runtime.initialize(self.input)


class BoundaryTests(unittest.TestCase):
    def test_rpc_round_trip_and_unknown_response(self):
        output = io.StringIO()
        rpc = hermes.Rpc(output, None)
        result = []
        worker = threading.Thread(target=lambda: result.append(rpc.request_sync("managed/tool_call", {"name": "workflow_result", "arguments": {}})))
        worker.start()
        for _ in range(100):
            if output.getvalue():
                break
            threading.Event().wait(0.001)
        call = json.loads(output.getvalue())
        rpc.response({"id": call["id"], "result": {"accepted": True}})
        worker.join(1)
        self.assertEqual(result, [{"accepted": True}])
        with self.assertRaises(ValueError):
            rpc.response({"id": "forged", "result": {}})

    def test_native_token_counters_are_disjoint_deltas_and_estimates_are_not_billing(self):
        runtime = hermes.Runtime(Host())
        counters = {"input_tokens": 10, "output_tokens": 8, "cache_read_tokens": 20, "cache_write_tokens": 4, "estimated_cost_usd": 1.5, "api_calls": 3, "model": "fixture"}
        first = runtime.usage(counters, 15)
        self.assertEqual(first["tokens"], {"inputTokens": 10, "outputTokens": 8, "cacheReadTokens": 20, "cacheWriteTokens": 4, "totalTokens": 42})
        self.assertNotIn("totalCostUsd", first)
        second = runtime.usage({**counters, "input_tokens": 13, "output_tokens": 12}, 10)
        self.assertEqual(second["tokens"]["totalTokens"], 7)
        self.assertEqual(second["tokens"]["inputTokens"], 3)
        self.assertNotIn("tokens", runtime.usage({**counters, "input_tokens": -1}, 0))

    def test_production_stdio_rejects_unprepared_runtime_without_tokens(self):
        with tempfile.TemporaryDirectory() as tmp:
            value = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"provider": "hermes", "cwd": tmp, "model": "fixture", "effort": None, "tools": [{"name": "workflow_result", "description": "result", "inputSchema": {"type": "object"}}], "options": {"source": tmp, "profileHome": tmp, "stateDir": tmp}}}
            result = subprocess.run([sys.executable, "-I", str(SCRIPT)], input=json.dumps(value) + "\n", capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            response = json.loads(result.stdout)
            self.assertEqual(response["id"], 1)
            self.assertIn("setup_required", response["error"]["message"])

    def test_public_audit_hook_rejects_profile_writes_and_subprocesses(self):
        with tempfile.TemporaryDirectory() as tmp:
            code = "import importlib.util, pathlib, subprocess, sys; s=importlib.util.spec_from_file_location('h',sys.argv[1]); h=importlib.util.module_from_spec(s); s.loader.exec_module(h); h.guard_filesystem(sys.argv[2]); pathlib.Path(sys.argv[2], 'owned').write_text('ok'); failures=0\nfor action in [lambda: pathlib.Path(sys.argv[3]).write_text('bad'), lambda: subprocess.run([sys.executable,'-c','pass'])]:\n try: action()\n except PermissionError: failures+=1\nassert failures==2"
            external = str(Path(tmp).parent / (Path(tmp).name + "-forbidden"))
            result = subprocess.run([sys.executable, "-I", "-c", code, str(SCRIPT), tmp, external], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(Path(external).exists())

    def test_route_capture_rejects_oauth_named_routes_and_interpolated_keys(self):
        class Yaml:
            config = {"model": {"provider": "openai", "default": "gpt-fixture", "base_url": "http://127.0.0.1:1", "api_key": "fake"}, "plugins": {"enabled": ["untrusted"]}}

            @classmethod
            def safe_load(cls, _):
                return cls.config

        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "config.yaml").write_text("fixture")
            route, key = hermes.read_route(tmp, "openai:gpt-fixture", Yaml, lambda *args, **kwargs: {})
            self.assertEqual(route, {"provider": "openai", "default": "gpt-fixture", "base_url": "http://127.0.0.1:1"})
            self.assertEqual(key, "fake")
            self.assertNotIn("plugins", route)
            Yaml.config["model"]["provider"] = "openai-codex"
            with self.assertRaisesRegex(ValueError, "OAuth"):
                hermes.read_route(tmp, None, Yaml, lambda *args, **kwargs: {})
            Yaml.config["model"]["provider"] = "openai"
            Yaml.config["model"]["api_key"] = "${SECRET}"
            with self.assertRaisesRegex(ValueError, "interpolation"):
                hermes.read_route(tmp, None, Yaml, lambda *args, **kwargs: {})
            Yaml.config["model"].update(provider="anthropic", api_key="sk-ant-oat-fixture")
            with self.assertRaisesRegex(ValueError, "OAuth"):
                hermes.read_route(tmp, None, Yaml, lambda *args, **kwargs: {})
            Yaml.config["providers"] = {"named": {}}
            with self.assertRaisesRegex(ValueError, "named"):
                hermes.read_route(tmp, None, Yaml, lambda *args, **kwargs: {})

    def test_profile_endpoint_is_captured_before_isolating_shell_routing(self):
        class Yaml:
            config = {"model": {"provider": "custom", "default": "fixture"}}

            @classmethod
            def safe_load(cls, _):
                return cls.config

        with tempfile.TemporaryDirectory() as tmp, patch.dict(hermes.os.environ, {
            "CUSTOM_BASE_URL": "https://stale-shell.example/v1",
            "OPENAI_BASE_URL": "https://unrelated.example/v1", "HERMES_PLUGIN": "unsafe", "MCP_SERVERS": "unsafe",
        }):
            Path(tmp, "config.yaml").write_text("fixture")
            Path(tmp, ".env").write_text("fixture")
            environment = {"CUSTOM_BASE_URL": "http://127.0.0.1:1234/v1", "OPENAI_API_KEY": "fake"}
            route, key = hermes.read_route(tmp, None, Yaml, lambda *args, **kwargs: environment)
            self.assertEqual(route["base_url"], "http://127.0.0.1:1234/v1")
            self.assertEqual(key, "fake")
            hermes.isolate_environment(Path(tmp))
            self.assertNotIn("CUSTOM_BASE_URL", hermes.os.environ)
            self.assertNotIn("OPENAI_BASE_URL", hermes.os.environ)
            self.assertNotIn("HERMES_PLUGIN", hermes.os.environ)
            self.assertNotIn("MCP_SERVERS", hermes.os.environ)
            self.assertEqual(hermes.os.environ["HERMES_HOME"], tmp)
            self.assertEqual(route["base_url"], "http://127.0.0.1:1234/v1")

            Yaml.config["model"].update(provider="openai-api", base_url="https://configured.example/v1", api_key="fake")
            route, _ = hermes.read_route(tmp, "openai-api:gpt-fixture", Yaml, lambda *args, **kwargs: environment)
            self.assertEqual(route["default"], "gpt-fixture")
            self.assertEqual(route["base_url"], "https://configured.example/v1")

            Yaml.config["model"] = {"provider": "custom", "default": "fixture", "api_key": "fake"}
            environment["CUSTOM_BASE_URL"] = "${PRIVATE_ENDPOINT}"
            with self.assertRaisesRegex(ValueError, "interpolation"):
                hermes.read_route(tmp, None, Yaml, lambda *args, **kwargs: environment)
            environment["CUSTOM_BASE_URL"] = "https://secret@example.com/v1"
            with self.assertRaisesRegex(ValueError, "embedded credentials"):
                hermes.read_route(tmp, None, Yaml, lambda *args, **kwargs: environment)

    def test_custom_profile_needs_endpoint_instead_of_cloud_fallback(self):
        class Yaml:
            @staticmethod
            def safe_load(_):
                return {"model": {"provider": "custom", "default": "fixture", "api_key": "fake"}}

        with tempfile.TemporaryDirectory() as tmp, patch.dict(hermes.os.environ, {}, clear=True):
            Path(tmp, "config.yaml").write_text("fixture")
            with self.assertRaisesRegex(ValueError, "explicit base URL"):
                hermes.read_route(tmp, None, Yaml, lambda *args, **kwargs: {})


if __name__ == "__main__":
    unittest.main()
