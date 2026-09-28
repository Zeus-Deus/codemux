#!/usr/bin/env python3
"""Runs the Rust adapter against official Hermes, isolated profiles and a loopback model.
No runtime patching, personal profile access, credential injection or paid inference.
Usage: python scripts/test-hermes-integration.py /absolute/path/to/hermes [ACP dependency directory]
"""
import argparse, http.server, json, os, pathlib, subprocess, sys, tempfile, threading, time
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("hermes", type=pathlib.Path)
parser.add_argument("acp_dependencies", type=pathlib.Path, nargs="?")
parser.add_argument("--serve", action="store_true", help="Keep the fixture running for isolated native UI tests")
parser.add_argument("--root", type=pathlib.Path, help="Reuse an existing disposable fixture directory")
parser.add_argument("--port", type=int, default=0)
args = parser.parse_args()
binary = args.hermes.resolve()
root = args.root.resolve() if args.root else pathlib.Path(tempfile.mkdtemp(prefix="codemux-hermes-integration-"))
if args.root and (root.parent != pathlib.Path(tempfile.gettempdir()).resolve() or not root.name.startswith("codemux-hermes-integration-")):
    parser.error("--root must name a disposable codemux-hermes-integration-* directory in the temporary directory")
requests = json.loads((root/"model-requests.json").read_text()) if (root/"model-requests.json").exists() else []
class Model(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def handle(self):
        try: super().handle()
        except (BrokenPipeError, ConnectionResetError): pass  # expected stream cancellation
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(json.dumps({"data": [{"id": "fixture-coder"}, {"id": "fixture-research"}]}).encode())
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        requests.append(body)
        messages = body.get("messages", [])
        index = max((i for i,m in enumerate(messages) if m.get("role") == "user"), default=-1)
        prompt = str(messages[index].get("content", "")) if index >= 0 else ""
        # Official Hermes carries the interrupted prompt into the next turn.
        # Interpret the latest user guidance as our fixture command; recorded requests
        # retain the untouched native messages.
        prompt = prompt.rsplit("\n\nUser correction/guidance after interrupt: ", 1)[-1]
        after = [m for m in messages[index+1:] if m.get("role") == "tool"]
        call = None
        if not after:
            if prompt == "CWD": call = ("terminal", {"command": "pwd > cwd.txt", "timeout": 10})
            elif prompt == "MEMORY": call = ("memory", {"action": "add", "target": "memory", "content": "Synthetic preference: amber tests only."})
            elif prompt == "MEMORY_UPDATE": call = ("memory", {"action":"replace", "target":"memory", "old_text":"Synthetic preference: amber tests only.", "content":"Synthetic preference: violet tests only."})
            elif prompt == "SKILL_CREATE": call = ("skill_manage", {"action": "create", "name": "synthetic-check", "content": "---\nname: synthetic-check\ndescription: Synthetic ACP test.\n---\n# Amber procedure\n"})
            elif prompt == "SKILL_UPDATE": call = ("skill_manage", {"action": "patch", "name": "synthetic-check", "old_string": "Amber procedure", "new_string": "Violet procedure"})
            elif prompt.startswith("EDIT "): call = ("write_file", {"path": prompt[5:], "content": "synthetic edit"})
        if call and call[0] == "skill_manage": call = (call[0], {"operations": [call[1]]})
        msg = {"role": "assistant", "content": None if call else "FIXTURE_OK"}
        if call: msg["tool_calls"] = [{"id": f"fixture-{len(requests)}", "type": "function", "function": {"name": call[0], "arguments": json.dumps(call[1])}}]
        finish = "tool_calls" if call else "stop"
        self.send_response(200)
        if body.get("stream"):
            self.send_header("Content-Type", "text/event-stream"); self.end_headers()
            delta = {"role": "assistant"}
            if call: delta["tool_calls"] = [{"index": 0, **msg["tool_calls"][0]}]
            else: delta.update(content="FIXTURE_OK", reasoning_content="Synthetic thought")
            for chunk in [{"choices": [{"index": 0, "delta": delta, "finish_reason": None}]}, {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}}]:
                chunk.update(id="fixture", object="chat.completion.chunk", created=int(time.time()), model=body.get("model"))
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                self.wfile.flush()
                if prompt == "SLOW_STREAM" and chunk["choices"][0]["finish_reason"] is None: time.sleep(3)
            self.wfile.write(b"data: [DONE]\n\n")
        else:
            self.send_header("Content-Type", "application/json"); self.end_headers()
            self.wfile.write(json.dumps({"id": "fixture", "object": "chat.completion", "created": int(time.time()), "model": body.get("model"), "choices": [{"index":0,"message":msg,"finish_reason":finish}],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}}).encode())
server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), Model)
threading.Thread(target=server.serve_forever, daemon=True).start()
for name in ("coder", "research", "independent", "named", "free"):
    home=root/"profiles"/name; home.mkdir(parents=True, exist_ok=True)
    config={"model":{"provider":"custom","default":"fixture-"+name,"base_url":f"http://127.0.0.1:{server.server_port}/v1","api_key":"synthetic-no-secret"},"compression":{"enabled":False},"auxiliary":{"background_review":{"enabled":False}},"memory":{"memory_enabled":True,"user_profile_enabled":True},"skills":{"creation_nudge_interval":0},"approvals":{"mode":"manual"},"display":{"auto_title":False},"terminal":{"backend":"local"},"mcp_servers":{}}
    if name == "free":
        config["model"] = {"provider":"opencode-free", "default":"deepseek-v4-flash-free"}
    if name == "named":
        config["model"] = {"provider":"custom:fixture", "default":"gpt-5"}
        config["providers"] = {"fixture":{"name":"fixture", "base_url":f"http://127.0.0.1:{server.server_port}/v1", "api_key":"synthetic-no-secret", "default_model":"gpt-5", "models":["gpt-5","gpt-4.1"], "discover_models":False}}
    (home/"config.yaml").write_text(json.dumps(config))
    (home/"SOUL.md").write_text("Synthetic persona: "+name)
# Cargo needs its normal build environment; the runtime child gets a disposable HOME and only
# the fixture's configured local model. No real provider credentials are carried into this run.
env={k:v for k,v in os.environ.items() if k in ("PATH","CARGO_HOME","RUSTUP_HOME","PKG_CONFIG_PATH","LD_LIBRARY_PATH","LANG","RUSTC_WRAPPER","CARGO_TARGET_DIR")}
real_home=pathlib.Path.home()
env.setdefault("CARGO_HOME",str(real_home/".cargo")); env.setdefault("RUSTUP_HOME",str(real_home/".rustup"))
env.update(HOME=str(root/"home"),CODEMUX_HERMES_TEST_BINARY=str(binary),CODEMUX_HERMES_TEST_ROOT=str(root),PYTHONDONTWRITEBYTECODE="1",TOKENIZERS_PARALLELISM="false")
(root/"home").mkdir(exist_ok=True)
if args.acp_dependencies: env["PYTHONPATH"]=str(args.acp_dependencies.resolve())
print("Isolated evidence:",root,flush=True)
subprocess.run([str(binary), "--profile", "coder", "acp", "--check"], env={**env, "HERMES_HOME":str(root)}, check=True)

if args.serve:
    # Reuse exactly the same fixture for the native GUI/IPC acceptance run.
    for name, sub in [("XDG_CONFIG_HOME","config"),("XDG_DATA_HOME","data"),("XDG_CACHE_HOME","cache"),("XDG_STATE_HOME","state"),("XDG_RUNTIME_DIR","run")]:
        directory=root/sub; directory.mkdir(mode=0o700,exist_ok=True); env[name]=str(directory)
    for name in ("DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS"):
        if name in os.environ: env[name]=os.environ[name]
    env.update(GDK_BACKEND="x11",CODEMUX_DISABLE_PTY_DAEMON="1", CODEMUX_DEV_OFFLINE_LOGIN="1", CODEMUX_API_URL="http://127.0.0.1:9")
    (root/"fixture.pid").write_text(str(os.getpid()))
    (root/"dev-env.json").write_text(json.dumps(env))
    print("Fixture ready for native UI; environment:",root/"dev-env.json",flush=True)
    try:
        while True: time.sleep(1)
    except KeyboardInterrupt: pass
    finally:
        (root/"model-requests.json").write_text(json.dumps(requests,indent=2)); server.shutdown()
    sys.exit(0)

try:
    result=subprocess.run(["cargo","test","-j","2","--manifest-path","src-tauri/Cargo.toml","--lib","agent_provider::hermes::tests::official_runtime_contract","--","--ignored","--nocapture","--test-threads=2"],env=env)
finally:
    (root/"model-requests.json").write_text(json.dumps(requests,indent=2)); server.shutdown()
if result.returncode == 0:
    def request_for(marker):
        return next(r for r in requests if r.get("tools") and any(m.get("role") == "user" and m.get("content") == marker for m in r.get("messages", [])))
    fresh = json.dumps(request_for("FRESH_MEMORY_SKILLS"))
    isolated = json.dumps(request_for("ISOLATION"))
    assert "violet tests only" in fresh and "synthetic-check" in fresh and "Synthetic persona: coder" in fresh
    assert "amber tests only" not in isolated and "violet tests only" not in isolated and "synthetic-check" not in isolated and "Synthetic persona: research" in isolated
    assert request_for("ISOLATION")["model"] == "fixture-research"
    assert request_for("FRESH_MEMORY_SKILLS")["model"] == "fixture-coder"
    print("Fresh-process memory, skill, persona and profile model isolation verified.", flush=True)
sys.exit(result.returncode)
