#!/usr/bin/env python3
"""Credential-free ACP fixture. Invoked with Python, never via a shell."""
import json
import os
import sys
import time
from typing import Any

mode = os.environ.get("FAKE_ACP_MODE", "resume")
log_path = os.environ["FAKE_ACP_LOG"]
native_id = "identical native/session ID"
prompt_id = None
callback_id = None
selected_model = "default"
selected_effort = " A effort "

def ordered_catalog(model):
    effort = " D effort " if model == " D model " else " C effort "
    options = [
        {"id": "model selector", "name": "Model", "category": "model", "type": "select", "currentValue": model,
         "options": [{"value": v, "name": v} for v in ["default", "vendor:model [1m]", " D model "]]},
        {"id": "ordered effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": effort,
         "options": [{"value": effort, "name": effort}]},
    ]
    if mode == "ordered-legacy":
        return {"configOptions": options[1:], "models": {"currentModelId": model,
                "availableModels": [{"modelId": v, "name": v} for v in ["default", "vendor:model [1m]", " D model "]]}}
    return {"configOptions": options}

def ordered_reply(request_id, model):
    global selected_model
    response = {"jsonrpc": "2.0", "id": request_id, "result": ordered_catalog(model)}
    if model == "vendor:model [1m]":
        selected_model = " D model "
        notification = {"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id,
                        "update": {"sessionUpdate": "config_option_update", **ordered_catalog(selected_model)}}}
        messages = [notification, response] if mode == "ordered-before" else [response, notification]
        if mode == "ordered-before": selected_model = model
        # One PIPE_BUF-sized write to the actual reader on a single-thread runner.
        os.write(1, ("\n".join(json.dumps(m) for m in messages) + "\n").encode())
    else:
        selected_model = model
        send(response)

def startup_catalog(latest):
    options = [{"id": "startup flag", "name": "Flag", "type": "boolean", "currentValue": latest}]
    if latest:
        options += [
            {"id": "startup model", "name": "Model", "category": "model", "type": "select", "currentValue": " D model ", "options": [{"value": " D model ", "name": "D"}]},
            {"id": "startup effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": " D effort ", "options": [{"value": " D effort ", "name": "D"}]},
        ]
    return {"configOptions": options}

def sparse_legacy_catalog(model):
    effort = " D effort " if model == " D model " else " C effort "
    available = [" D model "] if model == " D model " else ["default", "vendor:model [1m]", " D model "]
    return {"configOptions": [{"id": "legacy effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": effort, "options": [{"value": effort, "name": effort}]}],
            "models": {"currentModelId": model, "availableModels": [{"modelId": value, "name": value} for value in available]}}

def ownership_catalog():
    return {"configOptions": [{"id": "owned effort", "name": "Effort", "category": "thought_level", "type": "select",
            "currentValue": selected_effort, "options": [{"value": v, "name": v} for v in [" E0 ", " E1 "]]}],
            "models": {"currentModelId": selected_model, "availableModels": [{"modelId": v, "name": v} for v in
                ([" D model "] if selected_model == " D model " else ["default", "vendor:model [1m]"])]}}

def ownership_barrier():
    # The real pump prioritizes already-routed notifications before callbacks.
    # Do not send C until it has consumed D and answered this unsupported method.
    send({"jsonrpc": "2.0", "id": "ownership barrier", "method": "fixture/ownership_barrier", "params": {}})
    reply = json.loads(sys.stdin.readline())
    record(reply)
    assert reply["id"] == "ownership barrier" and reply["error"]["code"] == -32601

def ownership_change(request_id, model_change):
    global selected_model, selected_effort
    if model_change:
        selected_effort = " E1 "
        if "retired" in mode:
            selected_model = " D model "
        update = ownership_catalog()
        if "retired" not in mode:
            del update["models"]
    else:
        selected_model = "vendor:model [1m]"
        update = ownership_catalog()
    notification = {"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id,
                    "update": {"sessionUpdate": "config_option_update", **update}}}
    record({"fixture_wire": [notification]})
    send(notification)
    ownership_barrier()
    if model_change:
        acknowledgement = {}
        if "retired" not in mode:
            selected_model = "vendor:model [1m]"
    else:
        selected_effort = " E1 "
        acknowledgement = {"configOptions": ownership_catalog()["configOptions"]}
    record({"fixture_state": {"model": selected_model, "effort": selected_effort},
            "fixture_ack": acknowledgement})
    result(request_id, acknowledgement)

def ownership_startup(request_id, method):
    value: dict[str, Any] = startup_catalog(False)
    value["sessionId"] = native_id
    if method == "session/new" and "unknown" not in mode:
        result(request_id, value)
        return
    if method != "session/new":
        assert params["sessionId"] == native_id
    if method == "session/load":
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id,
              "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "inactive load history"}}}})
    def update(owner, catalog):
        notification = {"jsonrpc": "2.0", "method": "session/update", "params": {
                        "update": {"sessionUpdate": "config_option_update", **catalog}}}
        if owner is not None:
            notification["params"]["sessionId"] = owner
        return notification
    if "foreign" in mode:
        # Known N: matching D is retained while foreign/missing IDs use no budget.
        messages = [update(native_id, startup_catalog(True))]
        messages += [update("foreign native ID", startup_catalog(False)) for _ in range(65)]
        messages.append(update(None, startup_catalog(False)))
    elif "malformed" in mode:
        messages = [update(native_id, {"configOptions": "not an array"})]
    else:
        # Known matching, and unknown new-session queues remain bounded.
        messages = [update(native_id, startup_catalog(True)) for _ in range(65)]
    record({"fixture_burst": messages})
    for notification in messages:
        send(notification)
    ownership_barrier()
    response = {"jsonrpc": "2.0", "id": request_id, "result": value}
    messages = [response, update(native_id, startup_catalog(True))]
    record({"fixture_wire": messages})
    os.write(1, ("\n".join(json.dumps(m) for m in messages) + "\n").encode())

def dynamic_configs():
    effort_id = "effort A" if selected_model == "default" else "effort B"
    if mode == "dynamic-values":
        effort_id = "same effort ID"
    options = [
        {"id": "model selector", "name": "Model", "category": "model", "type": "select",
         "currentValue": selected_model, "options": [{"value": "default", "name": "A"},
         {"value": "vendor:model [1m]", "name": "B"}]},
        {"id": effort_id, "name": "Effort", "category": "thought_level", "type": "select",
         "currentValue": selected_effort, "options": [{"value": selected_effort, "name": "Effort"}]},
    ]
    if mode == "dynamic-notify-retired" and selected_model != "default":
        options[0]["options"] = options[0]["options"][1:]
    return options[:1] if mode == "dynamic-no-effort" and selected_model != "default" else options

def record(value):
    with open(log_path, "a", encoding="utf-8") as log:
        log.write(json.dumps(value) + "\n")

def send(value):
    print(json.dumps(value), flush=True)

def result(request_id, value):
    send({"jsonrpc": "2.0", "id": request_id, "result": value})

def configs() -> list[dict[str, Any]]:
    if mode.startswith("dynamic-"):
        return dynamic_configs()
    if mode == "no-model":
        return []
    options = [
        {"id": "model selector", "name": "Model", "category": "model", "type": "select",
         "currentValue": "default", "options": [{"value": "default", "name": "Literal Default"},
         {"value": "vendor:model [1m]", "name": "Opaque Model"}]},
        {"id": "reasoning selector", "name": "Effort", "category": "thought_level", "type": "select",
         "currentValue": " Big ", "options": [{"value": " Big ", "name": "Big"}]},
        {"id": "boolean selector", "name": "Flag", "type": "boolean", "currentValue": False},
        {"id": "mode selector", "name": "Mode", "category": "mode", "type": "select", "currentValue": "agent", "options": [{"value": "agent", "name": "Agent"}]},
    ]
    if mode == "ack-start":
        options[1]["options"].append({"value": "requested effort", "name": "Requested"})
    return options

record({"launch_argv": sys.argv[1:], "launch_env": os.environ.get("FAKE_LITERAL")})
if mode.startswith("ownership-"):
    record({"owned_pid": os.getpid()})
recovered = False
for line in sys.stdin:
    message = json.loads(line)
    record(message)
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params", {})
    if method == "initialize":
        load = mode == "load" or mode.endswith("-load")
        caps = {"loadSession": load, "sessionCapabilities": {"close": {}}}
        if not load and mode != "no-resume":
            caps["sessionCapabilities"]["resume"] = {}
        value = {"protocolVersion": 1, "agentCapabilities": caps, "authMethods": []}
        if mode == "version":
            value["protocolVersion"] = 2
        if mode == "auth":
            value["authMethods"] = [{"id": "exact-auth ID", "name": "Explicit Auth"}]
        result(request_id, None if mode == "bad-initialize" else value)
    elif method == "authenticate":
        assert mode == "auth", "No implicit authentication allowed"
        assert params == {"methodId": "exact-auth ID"}
        result(request_id, {})
    elif method in ("session/new", "session/resume", "session/load"):
        if mode.startswith("ownership-queue-"):
            ownership_startup(request_id, method)
            continue
        if mode.startswith("ownership-sparse-"):
            selected_effort = " E0 "
            if method != "session/new":
                assert params["sessionId"] == native_id
                recovered = True
                with open(log_path, encoding="utf-8") as log:
                    for entry in log:
                        state = json.loads(entry).get("fixture_state")
                        if state:
                            selected_model, selected_effort = state["model"], state["effort"]
            value = ownership_catalog()
            value["sessionId"] = native_id
            result(request_id, value)
            continue
        if mode.startswith("final-sparse-"):
            if method != "session/new":
                assert params["sessionId"] == native_id
                with open(log_path, encoding="utf-8") as log:
                    if any(json.loads(line).get("method") == "session/set_model" and json.loads(line).get("params", {}).get("modelId") == "vendor:model [1m]" for line in log):
                        selected_model = " D model "
            value = sparse_legacy_catalog(selected_model)
            value["sessionId"] = native_id
            result(request_id, value)
            continue
        if mode.startswith("final-startup-"):
            if method != "session/new":
                assert params["sessionId"] == native_id
            if method == "session/load":
                send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "historical replay MUST NOT duplicate"}}}})
            value = startup_catalog(False)
            value["sessionId"] = native_id
            response = {"jsonrpc": "2.0", "id": request_id, "result": value}
            messages = [response]
            if "-new-" in mode or method != "session/new":
                messages.append({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "config_option_update", **startup_catalog(True)}}})
            os.write(1, ("\n".join(json.dumps(m) for m in messages) + "\n").encode())
            continue
        gate_new = False
        if mode == "gated-new" and method == "session/new":
            with open(log_path, encoding="utf-8") as recorded:
                gate_new = sum(json.loads(entry).get("method") == "session/new" for entry in recorded) > 1
        if (mode == "gated-resume" and method == "session/resume") or gate_new:
            with open(log_path + ".ready", "w", encoding="utf-8") as ready:
                ready.write("ready")
            deadline = time.monotonic() + 10
            while not os.path.exists(log_path + ".release"):
                assert time.monotonic() < deadline, "test did not release resume gate"
                time.sleep(0.005)
        if mode == "resume-fails" and method != "session/new":
            send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32002, "message": "missing native session"}})
            continue
        if method == "session/load":
            send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "historical replay MUST NOT duplicate"}}}})
        response_value: dict[str, Any] = {"configOptions": configs()}
        if mode.startswith("ordered-"):
            response_value = ordered_catalog(selected_model)
        if mode == "invalid-ack-legacy":
            response_value = {"configOptions": configs()[1:], "models": {"currentModelId": "default", "availableModels": [{"modelId": v, "name": v} for v in ["default", "vendor:model [1m]"]]}}
        if method == "session/new":
            response_value["sessionId"] = native_id
        result(request_id, response_value)
    elif method == "session/set_config_option":
        if mode.startswith("ownership-sparse-"):
            assert params["configId"] == "owned effort"
            if recovered:
                assert params["value"] == selected_effort, "cold recovery must not restore obsolete effort"
                result(request_id, {"configOptions": ownership_catalog()["configOptions"]})
            else:
                assert params["value"] == " E1 "
                ownership_change(request_id, False)
            continue
        if mode.startswith("final-sparse-"):
            value = sparse_legacy_catalog(selected_model)
            assert params["configId"] == "legacy effort"
            assert params["value"] == value["configOptions"][0]["currentValue"]
            result(request_id, value)
            continue
        if mode.startswith("final-startup-"):
            target = next(o for o in startup_catalog(True)["configOptions"] if o["id"] == params["configId"])
            assert params["value"] == target["currentValue"]
            result(request_id, startup_catalog(True))
            continue
        if mode.startswith("ordered-"):
            ordered_reply(request_id, params["value"] if params["configId"] == "model selector" else selected_model)
            continue
        if mode == "reject-model" and params["configId"] == "model selector" and params["value"] == "vendor:model [1m]":
            send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32602, "message": "fixture rejects model"}})
            continue
        options = configs()
        target = next(option for option in options if option["id"] == params["configId"])
        if target["type"] == "boolean":
            assert params["type"] == "boolean"
            assert isinstance(params["value"], bool)
        else:
            assert any(option["value"] == params["value"] for option in target["options"])
        target["currentValue"] = "default" if mode in ("ack-model", "ack-start") and params["configId"] == "model selector" else params["value"]
        if mode == "ack-start" and params["configId"] == "reasoning selector":
            target["currentValue"] = " Big "
        if mode == "invalid-ack-model" and params["value"] == "vendor:model [1m]":
            target["currentValue"] = " unadvertised model "
        if mode == "invalid-ack-effort" and params["value"] == "vendor:model [1m]":
            next(option for option in options if option["category"] == "thought_level")["currentValue"] = " unadvertised effort "
        if mode.startswith("dynamic-"):
            if params["configId"] == "model selector":
                selected_model = params["value"]
                selected_effort = " A effort " if selected_model == "default" else " B effort "
            else:
                selected_effort = params["value"]
            options = configs()
        result(request_id, {"configOptions": options})
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "config_option_update", "configOptions": options}}})
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "current_mode_update", "currentModeId": "agent"}}})
    elif method == "session/prompt":
        if mode == "dynamic-notify-retired":
            selected_model = "vendor:model [1m]"
            selected_effort = " B effort "
            send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "config_option_update", "configOptions": configs()}}})
        if mode == "exit":
            sys.exit(17)
        if mode == "bad-json":
            print("{malformed protocol", flush=True)
            continue
        if mode == "bad-update":
            send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": None}})
            continue
        if mode == "notify-catalog":
            options = configs()
            next(option for option in options if option["id"] == "boolean selector")["currentValue"] = True
            send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "config_option_update", "configOptions": options}}})
        if mode == "permission":
            prompt_id = request_id
            callback_id = "permission callback"
            options = [{"optionId": "always ID", "name": "Always", "kind": "allow_always"}, {"optionId": "once ID ", "name": "Once", "kind": "allow_once"}]
            # Same process, wrong session: MUST be cancelled without a UI approval.
            send({"jsonrpc": "2.0", "id": "wrong session", "method": "session/request_permission", "params": {"sessionId": "foreign native ID", "options": options, "toolCall": {"toolCallId": "foreign"}}})
            send({"jsonrpc": "2.0", "id": callback_id, "method": "session/request_permission", "params": {"sessionId": native_id, "options": options, "toolCall": {"toolCallId": "real-tool"}}})
        elif mode == "hold":
            prompt_id = request_id
        else:
            send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "tail chunk"}}}})
            result(request_id, {"stopReason": "end_turn"})
            if mode == "final-exit":
                sys.exit(0)
    elif method == "session/set_model" and mode.startswith("ownership-sparse-"):
        if recovered:
            assert params["modelId"] == selected_model, "cold recovery must not restore obsolete model"
            result(request_id, {})
        else:
            assert params["modelId"] == "vendor:model [1m]"
            ownership_change(request_id, True)
    elif method == "session/set_model" and mode == "ordered-legacy":
        ordered_reply(request_id, params["modelId"])
    elif method == "session/set_model" and mode.startswith("final-sparse-"):
        if params["modelId"] == "vendor:model [1m]":
            acknowledgement = {}
            if "invalid-ack" in mode:
                acknowledgement = sparse_legacy_catalog("default")
                acknowledgement["models"]["currentModelId"] = " unadvertised ACK "
            elif "invalid-update" in mode:
                acknowledgement = sparse_legacy_catalog(params["modelId"])
            selected_model = " D model "
            update = sparse_legacy_catalog(selected_model)
            if "invalid-update" in mode:
                update["models"]["currentModelId"] = " unadvertised update "
            messages = [{"jsonrpc": "2.0", "id": request_id, "result": acknowledgement},
                        {"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": native_id, "update": {"sessionUpdate": "config_option_update", **update}}}]
            record({"fixture_wire": messages})
            os.write(1, ("\n".join(json.dumps(message) for message in messages) + "\n").encode())
        else:
            assert params["modelId"] == selected_model
            result(request_id, {})
    elif method == "session/set_model" and mode == "invalid-ack-legacy":
        result(request_id, {"models": {"currentModelId": " unadvertised model " if params["modelId"] == "vendor:model [1m]" else params["modelId"], "availableModels": [{"modelId": v, "name": v} for v in ["default", "vendor:model [1m]"]]}})
    elif method == "session/cancel":
        # Permission-mode fixture deliberately does NOT settle from cancel alone:
        # client must immediately answer its pending permission callback.
        if mode == "hold" and prompt_id is not None:
            result(prompt_id, {"stopReason": "cancelled"})
            prompt_id = None
    elif method == "session/close":
        result(request_id, {})
    elif method is None and request_id == callback_id:
        assert message.get("result", {}).get("outcome", {}).get("outcome") in ("selected", "cancelled")
        result(prompt_id, {"stopReason": "cancelled" if message["result"]["outcome"]["outcome"] == "cancelled" else "end_turn"})
        prompt_id = None
    elif method is not None:
        send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32601, "message": "unknown method"}})
