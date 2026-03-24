---
title: Port Detection
description: Auto-detected listening ports with browser integration and process info.
---

# Port Detection

Codemux automatically detects TCP ports listening on your machine and displays them in the sidebar.

## How It Works

The backend periodically scans for listening TCP ports. Each detected port shows:

- **Port number**
- **Process name** — The process that opened the port
- **PID** — Process ID
- **Workspace association** — If the port was opened by a process in a specific workspace

Ports appear in the **Ports** section of the sidebar, grouped by workspace when possible.

## Actions

Right-click or hover a port badge to:

- **Open in Browser** — Opens `http://localhost:{port}` in the embedded browser pane
- **Kill** — Terminates the process listening on the port

## Runtime Only

Port detection is runtime-only. Detected ports are not persisted — they're rescanned on each app launch and when workspace state changes.
