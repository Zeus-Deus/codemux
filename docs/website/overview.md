---
title: Overview
description: What Codemux is — a terminal-first agent development environment with workspaces, browser, and orchestration.
---

# Overview

Codemux is a terminal-first agent development environment (ADE). It combines terminal multiplexing, an embedded browser, full git integration, and multi-agent orchestration into a single desktop application.

## What It Does

- **Workspaces** — Each workspace is an isolated coding context with its own terminal tabs, panes, browser, and git state. Switch between projects instantly.
- **Terminal multiplexer** — Split panes, multiple tabs, canvas rendering. Run AI coding agents (Claude Code, Codex, OpenCode, Gemini) side by side.
- **Embedded browser** — Live browser pane inside the app. Agents can control it programmatically for testing and debugging.
- **Git workflow** — Stage, commit, push, pull, view diffs, create PRs, review CI checks — all from the sidebar panel without leaving the terminal.
- **AI tools** — Generate commit messages with AI, resolve merge conflicts with an AI agent on a safe temp branch.
- **OpenFlow orchestration** — Run multiple AI agents in coordinated workflows with an orchestrator managing delegation, communication, and phase transitions.

## Architecture

- **Frontend**: React 19 + Tailwind v4 + shadcn/ui, state via Zustand
- **Backend**: Rust (Tauri 2), async via Tokio
- **Terminal**: xterm.js with WebGL rendering, PTY via portable-pty
- **Browser**: agent-browser with screenshot-based rendering
- **Git**: Direct git CLI calls from Rust, GitHub via `gh` CLI

## Key Differences

**Terminal-first**: The terminal is the primary interface, not an afterthought. Panes resize, split, and swap fluidly. Canvas rendering avoids WebGL freezes.

**Agent-native**: Built for AI coding agents. Presets launch Claude Code, Codex, or OpenCode with one click. Browser automation lets agents test their own output. OpenFlow coordinates multiple agents on a single task.

**Workspace isolation**: Each workspace can use git worktrees so agents work on separate branches without interfering with each other.
