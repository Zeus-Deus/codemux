---
title: Overview
description: What Codemux is — a terminal-first agent development environment with workspaces, browser, and orchestration.
---

# Overview

Codemux is a terminal-first agent development environment (ADE). It combines terminal multiplexing, an embedded browser, full git integration, and multi-agent orchestration into a single desktop application.

## What It Does

- **Prompt-first workspaces** — Describe what you want to do, and Codemux creates a workspace with an auto-generated branch, selected agent, and your task as the initial prompt.
- **Project management** — Open existing repos or create new ones (empty or clone). An onboarding wizard detects your package manager and sets up dependencies automatically.
- **Terminal multiplexer** — Split panes, multiple tabs, canvas rendering. Run AI coding agents (Claude Code, Codex, OpenCode, Gemini) side by side.
- **Agent status indicators** — Real-time status dots show what each agent is doing: working (amber), needs input (red), ready for review (green). Visible in the sidebar and tab bar.
- **Preset bar** — Pin agent presets for one-click launch. A Run button with `Ctrl+Shift+G` executes your project's dev command instantly.
- **Embedded browser** — Hybrid browser with three input tiers: CSS selector, CDP coordinate-based, and OS-level (ydotool) for stealth interaction. Agents control it programmatically.
- **Git workflow** — Stage, commit, push, pull, view diffs, create PRs, merge branches locally, review CI checks — all from the sidebar panel.
- **Local branch merge** — Merge a base branch into your feature branch directly from the Changes panel. Conflict resolution with per-file ours/theirs/AI resolve.
- **AI tools** — Generate commit messages with AI, resolve merge conflicts with an AI agent on a safe temp branch.
- **Setup scripts** — Configure per-project setup/teardown commands that run automatically when workspaces open or close. Docker Compose support built in.
- **Auth and settings sync** — Sign in with GitHub OAuth or email/password. Personal settings sync across devices via your account.
- **OpenFlow orchestration** — Run multiple AI agents in coordinated workflows with an orchestrator managing delegation, communication, and phase transitions.

## Architecture

- **Frontend**: React 19 + Tailwind v4 + shadcn/ui, state via Zustand
- **Backend**: Rust (Tauri 2), async via Tokio
- **Terminal**: xterm.js with canvas rendering (WebGL fallback), PTY via portable-pty
- **Browser**: Hybrid 3-tier input (selector → CDP coordinates → OS-level ydotool), screenshot-based rendering
- **Git**: Direct git CLI calls from Rust, GitHub via `gh` CLI

## Key Differences

**Terminal-first**: The terminal is the primary interface, not an afterthought. Panes resize, split, and swap fluidly. Canvas rendering avoids WebGL freezes.

**Agent-native**: Built for AI coding agents. Presets launch Claude Code, Codex, or OpenCode with one click. Browser automation lets agents test their own output. OpenFlow coordinates multiple agents on a single task.

**Workspace isolation**: Each workspace can use git worktrees so agents work on separate branches without interfering with each other.
