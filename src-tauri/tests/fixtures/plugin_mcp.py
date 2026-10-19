#!/usr/bin/env python3
"""Local MCP fixture: empty → paginated tools, notifications, images, slow calls."""
import argparse
import json
import os
import sys
import threading
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--state")
parser.add_argument("--empty", action="store_true")
parser.add_argument("--loop-cursor", action="store_true")
parser.add_argument("--init-delay", type=float, default=0)
args = parser.parse_args()
lock = threading.Lock()


def send(message):
    with lock:
        print(json.dumps(message), flush=True)


def state():
    try:
        return Path(args.state).read_text().strip() if args.state else "ready"
    except FileNotFoundError:
        return "empty" if args.empty else "ready"


def watch():
    previous = state()
    while True:
        time.sleep(0.03)
        current = state()
        if current != previous:
            previous = current
            send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})


threading.Thread(target=watch, daemon=True).start()


def handle(message):
    method = message.get("method")
    if "id" not in message:
        return
    params = message.get("params", {})
    if method == "initialize":
        time.sleep(args.init_delay)
        result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {"listChanged": True}}, "serverInfo": {"name": "plugin-fixture", "version": "1"}}
    elif method == "tools/list":
        names = [] if state() == "empty" else ["echo", "image", "slow", "new_tool"]
        offset = int(params.get("cursor", "0"))
        result = {"tools": [{"name": name, "description": "Return the fixture's secret verification word." if name == "new_tool" else "Local fixture " + name, "inputSchema": {"type": "object", "properties": {}, "additionalProperties": True}} for name in names[offset:offset + 2]]}
        if offset + 2 < len(names):
            result["nextCursor"] = "0" if args.loop_cursor else str(offset + 2)
    elif method == "tools/call":
        name = params.get("name")
        if name == "slow":
            time.sleep(120)
        if name == "image":
            result = {"content": [{"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jfsoAAAAASUVORK5CYII="}], "structuredContent": {"intact": True}}
        else:
            result = {"content": [{"type": "text", "text": "ORCHID-742" if name == "new_tool" else json.dumps({"cwd": os.getcwd(), "arguments": params.get("arguments", {}), "env": os.getenv("PLUGIN_FIXTURE_ENV")})}]}
    else:
        result = {}
    send({"jsonrpc": "2.0", "id": message["id"], "result": result})


for line in sys.stdin:
    threading.Thread(target=handle, args=(json.loads(line),), daemon=True).start()
