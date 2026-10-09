"""Owned Hermes library adapter. stdout is JSON-RPC; no personal profile writes.

The official Python library is loaded only after a clean home and the host's
captured tool catalog exist. Missing prepared dependencies fail closed.
"""
from __future__ import annotations

import asyncio
import concurrent.futures
import inspect
from functools import partial
import json
import os
from pathlib import Path
import re
import sys
import threading
import time
from urllib.parse import urlsplit
import uuid

ADAPTER_VERSION = "hermes-library-v1"
TOOLSET = "codemux_managed_workflow"


class ManagedError(ValueError):
    """Only static host-owned diagnostic strings may cross the RPC boundary."""


MAX_LINE = 2 * 1024 * 1024
THREADS = concurrent.futures.ThreadPoolExecutor(max_workers=4, thread_name_prefix="managed-hermes")


async def run_thread(function, *args, **kwargs):
    return await asyncio.get_running_loop().run_in_executor(THREADS, partial(function, *args, **kwargs))


API_MODES = {"chat_completions", "anthropic_messages", "codex_responses"}
KEY_ENV = {
    "anthropic": "ANTHROPIC_API_KEY", "openrouter": "OPENROUTER_API_KEY",
    "openai": "OPENAI_API_KEY", "openai-api": "OPENAI_API_KEY", "custom": "OPENAI_API_KEY",
    "deepseek": "DEEPSEEK_API_KEY", "groq": "GROQ_API_KEY",
    "mistral": "MISTRAL_API_KEY", "together": "TOGETHER_API_KEY",
    "fireworks": "FIREWORKS_API_KEY", "cerebras": "CEREBRAS_API_KEY",
    "xai": "XAI_API_KEY", "zai": "ZAI_API_KEY", "kimi-coding": "KIMI_API_KEY",
    "ollama": "OPENAI_API_KEY", "vllm": "OPENAI_API_KEY",
}
ENDPOINT_ENV = {
    "anthropic": ("ANTHROPIC_BASE_URL",),
    "openai": ("OPENAI_BASE_URL",), "openai-api": ("OPENAI_BASE_URL",),
    "openrouter": ("CUSTOM_BASE_URL", "OPENROUTER_BASE_URL"),
    "custom": ("CUSTOM_BASE_URL",), "ollama": ("CUSTOM_BASE_URL",), "vllm": ("CUSTOM_BASE_URL",),
    "deepseek": ("DEEPSEEK_BASE_URL",), "xai": ("XAI_BASE_URL",),
    "zai": ("GLM_BASE_URL",), "kimi-coding": ("KIMI_BASE_URL",),
}


def parse_initialize(value):
    if not isinstance(value, dict) or value.get("provider") != "hermes":
        raise ManagedError("Invalid Hermes initialization")
    if not isinstance(value.get("cwd"), str) or not Path(value["cwd"]).is_dir():
        raise ManagedError("Managed workspace is unavailable")
    for key in ("model", "effort"):
        if value.get(key) is not None and not isinstance(value[key], str):
            raise ManagedError("Invalid managed model or effort")
    tools = value.get("tools")
    if not isinstance(tools, list) or not 1 <= len(tools) <= 64:
        raise ManagedError("Invalid managed tool catalog")
    names = set()
    for tool in tools:
        if not isinstance(tool, dict) or not isinstance(tool.get("name"), str) or not re.fullmatch(r"workflow_[a-z0-9_]+", tool["name"]):
            raise ManagedError("Invalid managed tool name")
        schema = tool.get("inputSchema")
        if tool["name"] in names or not isinstance(tool.get("description"), str) or not isinstance(schema, dict) or schema.get("type") != "object" or len(json.dumps(schema)) > 65536:
            raise ManagedError("Invalid or duplicate managed tool schema")
        names.add(tool["name"])
    options = value.get("options")
    if not isinstance(options, dict) or not all(isinstance(options.get(k), str) for k in ("source", "profileHome", "stateDir")):
        raise ManagedError("Prepared Hermes source, profile and owned home are required")
    return value


def guard_filesystem(home):
    """Public Python audit boundary prevents SDK startup from repairing user state."""
    root = Path(home).resolve()

    def owned(path):
        if isinstance(path, int):
            return False
        try:
            return Path(os.fsdecode(path)).resolve().is_relative_to(root)
        except (TypeError, ValueError, OSError):
            return False

    mutations = {"os.remove", "os.rmdir", "os.mkdir", "os.chmod", "os.chown", "os.utime", "os.truncate", "os.link", "os.symlink", "os.rename"}

    def audit(event, args):
        if event == "open":
            mode, flags = args[1], args[2]
            write = (isinstance(mode, str) and any(c in mode for c in "wax+")) or (isinstance(flags, int) and flags & (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC | os.O_APPEND))
            if write and not owned(args[0]):
                raise PermissionError("Managed Hermes cannot write outside its owned home")
        elif event in mutations:
            paths = args[:2] if event in {"os.link", "os.symlink", "os.rename"} else args[:1]
            if not all(owned(path) for path in paths):
                raise PermissionError("Managed Hermes cannot mutate personal profile or source files")
        elif event in {"subprocess.Popen", "os.system", "os.exec", "os.posix_spawn", "os.fork", "os.forkpty"}:
            raise PermissionError("Managed Hermes cannot launch native tools or installers")

    sys.addaudithook(audit)


def read_route(profile, selected_model, yaml, dotenv_values):
    """Read only routing and API keys; never load a user's plugins or OAuth store."""
    home = Path(profile).resolve()
    config_path = home / "config.yaml"
    if config_path.stat().st_size > 1024 * 1024:
        raise ManagedError("Hermes routing configuration is too large")
    config = yaml.safe_load(config_path.read_text(encoding="utf-8-sig"))
    if not isinstance(config, dict):
        raise ManagedError("Hermes routing configuration must be a mapping")
    if any(config.get(k) for k in ("providers", "custom_providers", "secret_sources", "credential_pool")):
        raise ManagedError("Managed Hermes requires a direct API-key or local route; named routes, pools and external secrets need explicit adapter support")
    model = config.get("model")
    if isinstance(model, str):
        model = {"default": model}
    if not isinstance(model, dict):
        raise ManagedError("Hermes profile has no model routing configuration")
    provider = model.get("provider", "auto")
    if provider not in KEY_ENV or model.get("openai_runtime") == "codex_app_server" or model.get("auth_mode") not in (None, "api_key"):
        raise ManagedError("Managed Hermes requires an explicit supported API-key or local provider; OAuth, plugin and external-process runtimes are unavailable")
    default = model.get("default", model.get("model", model.get("name")))
    if not isinstance(default, str) or not default:
        raise ManagedError("Hermes profile must configure a model explicitly")
    chosen = default if selected_model in (None, "profile_default") else selected_model
    prefix = provider + ":"
    if chosen.startswith(prefix):
        chosen = chosen[len(prefix):]
    if not chosen or re.match(r"^[a-z0-9_-]+:", chosen):
        raise ManagedError("Managed Hermes cannot infer a different or named provider from this model ID")
    route = {k: model[k] for k in ("base_url", "api_mode", "api_key") if k in model}
    configured_key = route.pop("api_key", None)
    if any(not isinstance(v, str) or any(s in v for s in ("${", "$(", "`")) for v in route.values()) or route.get("api_mode", "chat_completions") not in API_MODES:
        raise ManagedError("Unsupported Hermes wire protocol or routing fields")
    env = {}
    env_path = home / ".env"
    if env_path.exists():
        if env_path.stat().st_size > 1024 * 1024:
            raise ManagedError("Hermes environment file is too large")
        env = dotenv_values(env_path, interpolate=False)
    if not route.get("base_url"):
        for name in ENDPOINT_ENV.get(provider, ()):
            endpoint = env.get(name) or os.environ.get(name)
            if endpoint:
                route["base_url"] = endpoint
                break
    if route.get("base_url"):
        endpoint = route["base_url"]
        if not isinstance(endpoint, str) or any(s in endpoint for s in ("${", "$(", "`")):
            raise ManagedError("Managed Hermes endpoint cannot use interpolation or executable sources")
        try:
            parsed = urlsplit(endpoint)
            valid = parsed.scheme in {"http", "https"} and parsed.hostname and not parsed.username and not parsed.password
        except ValueError:
            valid = False
        if not valid:
            raise ManagedError("Managed Hermes endpoint must be an explicit HTTP or HTTPS URL without embedded credentials")
    key_name = KEY_ENV[provider]
    key = configured_key or env.get(key_name) or os.environ.get(key_name)
    if key is not None and (not isinstance(key, str) or not key.isascii() or any(s in key for s in ("${", "$(", "`"))):
        raise ManagedError("Managed Hermes API key cannot use interpolation or executable secret sources")
    if provider == "anthropic" and key and key.strip().startswith("sk-ant-oat"):
        raise ManagedError("Managed Hermes requires an Anthropic API key; OAuth tokens are unavailable")
    local = provider in {"ollama", "vllm"}
    if not key and not local:
        raise ManagedError("Managed Hermes requires an API key; no OAuth refresh or credential fallback is attempted")
    if (local or provider == "custom") and not route.get("base_url"):
        raise ManagedError("Custom and local Hermes routing require an explicit base URL")
    route.update(default=chosen, provider=provider)
    return route, key or "no-key-required"


def isolate_environment(home):
    # Endpoint selection is captured above. Unrelated shell endpoints must not
    # silently redirect the official resolver away from the selected profile.
    for name in list(os.environ):
        if name.startswith("HERMES_") or name.startswith("MCP_") or name.endswith("_BASE_URL"):
            os.environ.pop(name, None)
    os.environ.update(HERMES_HOME=str(home), HERMES_RUNTIME_DIR=str(home / "runtime"), HERMES_DISABLE_LAZY_INSTALLS="1", XDG_CACHE_HOME=str(home / "cache"), TMPDIR=str(home / "tmp"))


def load_api(input):
    options = input["options"]
    source, home = Path(options["source"]).resolve(), Path(options["stateDir"]).resolve()
    if sys.version_info < (3, 14) or not (source / "run_agent.py").is_file() or sys.prefix == sys.base_prefix:
        raise ManagedError("setup_required: select an already prepared Hermes Python 3.14 environment and source with CODEMUX_HERMES_PYTHON and CODEMUX_HERMES_SOURCE")
    # A clean source environment is required. Official startup recovery must
    # never repair a personal installation on behalf of a managed attempt.
    if (source / ".env").exists() or any((source / name).exists() for name in (".update-incomplete", ".lazy-refresh-incomplete")):
        raise ManagedError("setup_required: managed Hermes source must have no repository .env or pending update recovery")
    home.mkdir(parents=True, exist_ok=True)
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(source))
    import yaml
    from dotenv import dotenv_values
    route, key = read_route(options["profileHome"], input.get("model"), yaml, dotenv_values)
    isolate_environment(home)
    (home / "tmp").mkdir()
    config = {"model": route, "plugins": {"enabled": []}, "mcp_servers": {}, "context": {"engine": "compressor"}, "security": {"allow_lazy_installs": False}, "compression": {"enabled": False}}
    (home / "config.yaml").write_text(yaml.safe_dump(config), encoding="utf-8")
    guard_filesystem(home)
    from hermes_cli.runtime_provider import resolve_runtime_provider
    runtime = resolve_runtime_provider(requested=route["provider"], explicit_api_key=key, explicit_base_url=route.get("base_url"), target_model=route["default"])
    if runtime.get("api_mode") not in API_MODES or not isinstance(runtime.get("api_key"), str) or runtime.get("credential_pool") or runtime.get("command") or runtime.get("acp_command"):
        raise ManagedError("Managed Hermes resolved an unsupported runtime")
    from run_agent import AIAgent
    from tools.registry import registry
    return AIAgent, registry, runtime, route["default"]


class Runtime:
    def __init__(self, host, api_loader=load_api):
        self.host, self.api_loader = host, api_loader
        self.agent = None
        self.active = None
        self.cancelled = threading.Event()
        self.closed = False
        self.expected = {}
        self.history = None
        self.usage_totals = None

    def usage(self, result, duration_ms):
        value = {"durationMs": duration_ms, "numTurns": result.get("api_calls", 1)}
        if not isinstance(value["numTurns"], int) or isinstance(value["numTurns"], bool) or value["numTurns"] < 0:
            value["numTurns"] = 1
        names = {"input_tokens": "inputTokens", "output_tokens": "outputTokens", "cache_read_tokens": "cacheReadTokens", "cache_write_tokens": "cacheWriteTokens"}
        current = {target: result.get(source) for source, target in names.items()}
        if all(isinstance(count, int) and not isinstance(count, bool) and count >= 0 for count in current.values()):
            previous = self.usage_totals or {name: 0 for name in current}
            if all(current[name] >= previous[name] for name in current):
                tokens = {name: count - previous[name] for name, count in current.items()}
                tokens["totalTokens"] = sum(tokens.values())
                value["tokens"] = tokens
                self.usage_totals = current
        model = result.get("served_model", result.get("model"))
        if isinstance(model, str) and model:
            value["model"] = model
        # Hermes's estimated_cost_usd is not a provider invoice reading.
        return value

    def catalog(self):
        schemas = self.agent.tools
        actual = {t["function"]["name"]: t["function"].get("parameters") for t in schemas}
        if len(schemas) != len(self.expected) or actual != self.expected or set(self.agent.valid_tool_names) != set(self.expected):
            raise ManagedError("Hermes did not expose exactly the captured host tool schemas")

    async def initialize(self, value):
        if self.agent is not None or self.closed:
            raise ManagedError("Managed Hermes cannot initialize twice")
        input = parse_initialize(value)
        AIAgent, registry, route, model = await run_thread(self.api_loader, input)
        required = {"enabled_toolsets", "skip_context_files", "skip_memory", "skip_background_review", "session_db", "checkpoints_enabled", "load_soul_identity", "max_iterations", "run_budget_seconds"}
        if not required <= set(inspect.signature(AIAgent).parameters):
            raise ManagedError("setup_required: Hermes library lacks the managed isolation constructor contract")
        self.expected = {t["name"]: t["inputSchema"] for t in input["tools"]}
        for tool in input["tools"]:
            if registry.get_schema(tool["name"]) is not None:
                raise ManagedError("Managed tool name collides with Hermes registry")
            name = tool["name"]

            # Capture one name per registration; no identities come from model args.
            def bind(tool_name):
                def call(arguments, **kwargs):
                    if self.active is None or self.closed or self.cancelled.is_set():
                        raise ManagedError("Managed tool caller is not active")
                    self.catalog()
                    return json.dumps(self.host.request_sync("managed/tool_call", {"name": tool_name, "arguments": arguments}), ensure_ascii=False)
                return call

            registry.register(name=name, toolset=TOOLSET, schema={"name": name, "description": tool["description"], "parameters": tool["inputSchema"]}, handler=bind(name), max_result_size_chars=MAX_LINE)
        effort = input.get("effort")
        if effort is not None and effort not in {"low", "medium", "high", "xhigh"}:
            raise ManagedError("Unsupported managed Hermes reasoning level")
        self.agent = await run_thread(AIAgent, model=model, provider=route["provider"], api_mode=route["api_mode"], base_url=route.get("base_url"), api_key=route["api_key"], enabled_toolsets=[TOOLSET], max_iterations=64, run_budget_seconds=3600, quiet_mode=True, save_trajectories=False, skip_context_files=True, load_soul_identity=False, skip_memory=True, skip_background_review=True, session_db=None, checkpoints_enabled=False, fallback_model=None, credential_pool=None, cwd=input["cwd"], session_id=str(uuid.uuid4()), reasoning_config={"effort": effort} if effort else None)
        self.catalog()
        return {"protocolVersion": 1, "provider": "hermes", "adapterVersion": ADAPTER_VERSION, "sessionId": self.agent.session_id, "tools": list(self.expected), "isolation": {"nativeTools": [], "nativeFanout": False, "ambientConfig": False}}

    async def start(self, value):
        if not self.agent or self.closed or self.active is not None or not isinstance(value, dict) or not all(isinstance(value.get(k), str) for k in ("turnId", "prompt")):
            raise ManagedError("Invalid or concurrent managed Hermes turn")
        self.catalog()
        self.active = value["turnId"]
        self.cancelled.clear()
        asyncio.create_task(self.run(value))
        return {}

    async def run(self, value):
        started = time.monotonic()
        status, message = "success", None
        result = {}
        try:
            result = await run_thread(self.agent.run_conversation, user_message=value["prompt"], conversation_history=self.history)
            self.catalog()
            if self.cancelled.is_set() or result.get("interrupted"):
                status = "cancelled"
            elif result.get("error") or result.get("failed") or result.get("partial") or result.get("completed") is not True:
                status, message = "error", "Hermes did not complete the managed turn"
            else:
                self.history = result.get("messages")
                final = result.get("final_response")
                if isinstance(final, str) and final:
                    self.host.notify("text", {"turnId": value["turnId"], "text": final})
        except Exception:
            status, message = "error", "Managed Hermes runtime failed; verify the prepared library and routing configuration"
        finally:
            self.active = None
            payload = {"turnId": value["turnId"], "status": status, "usage": self.usage(result if isinstance(result, dict) else {}, int((time.monotonic() - started) * 1000))}
            if message:
                payload["message"] = message
            self.host.notify("complete", payload)

    async def cancel(self):
        self.cancelled.set()
        if self.agent:
            await run_thread(self.agent.interrupt, hard_cancel=True, tool_reason="codemux_managed_stop")
        return {}

    async def dispose(self):
        self.closed = True
        await self.cancel()
        if self.agent and self.active is None:
            await run_thread(self.agent.close)
        return {}


class Rpc:
    def __init__(self, output, loop):
        self.output, self.loop = output, loop
        self.lock = threading.Lock()
        self.pending = {}
        self.sequence = 0

    def emit(self, value):
        line = json.dumps(value, ensure_ascii=False)
        if len(line) > MAX_LINE:
            raise ManagedError("Managed RPC message exceeds limit")
        with self.lock:
            self.output.write(line + "\n")
            self.output.flush()

    def notify(self, method, params):
        self.emit({"jsonrpc": "2.0", "method": method, "params": params})

    def request_sync(self, method, params):
        with self.lock:
            self.sequence += 1
            id = "tool-" + str(self.sequence)
            future = concurrent.futures.Future()
            self.pending[id] = future
        self.emit({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        try:
            return future.result(timeout=60)
        finally:
            with self.lock:
                self.pending.pop(id, None)

    def response(self, value):
        with self.lock:
            future = self.pending.get(value.get("id"))
            if future is None or future.done():
                raise ManagedError("Unknown managed RPC response")
            if "error" in value:
                future.set_exception(ValueError("Host rejected managed tool call"))
            else:
                future.set_result(value.get("result"))


async def serve():
    output = sys.stdout
    sys.stdout = sys.stderr
    rpc = Rpc(output, asyncio.get_running_loop())
    runtime = Runtime(rpc)
    try:
        while True:
            line = await run_thread(sys.stdin.readline, MAX_LINE + 1)
            if not line:
                break
            if len(line) > MAX_LINE or not line.endswith("\n"):
                raise ManagedError("Invalid managed RPC framing")
            value = json.loads(line)
            if not isinstance(value, dict) or value.get("jsonrpc") != "2.0":
                raise ManagedError("Invalid managed RPC message")
            if "method" not in value:
                rpc.response(value)
                continue
            id, method = value.get("id"), value["method"]
            try:
                if method == "initialize":
                    result = await runtime.initialize(value.get("params"))
                elif method == "startTurn":
                    result = await runtime.start(value.get("params"))
                elif method == "cancel":
                    result = await runtime.cancel()
                elif method == "shutdown":
                    result = await runtime.dispose()
                else:
                    raise ManagedError("Unknown managed Hermes method")
                rpc.emit({"jsonrpc": "2.0", "id": id, "result": result})
            except Exception as error:
                # Route/library errors must never expose a credential value.
                message = str(error) if isinstance(error, ManagedError) else "Managed Hermes initialization failed; verify prepared source/runtime setup"
                rpc.emit({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": message}})
                if method == "initialize":
                    break
            if method == "shutdown":
                break
    finally:
        await runtime.dispose()


if __name__ == "__main__":
    try:
        asyncio.run(serve())
    finally:
        THREADS.shutdown(wait=False, cancel_futures=True)
