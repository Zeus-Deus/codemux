# Codemux

Codemux is a Tauri-based agentic development environment (ADE) with a React/TypeScript frontend and Rust backend.

## Aim

Make Codemux performant, reliable, and easy to use. Do not preserve complexity just because it already exists. Treat these instructions as defaults; the user's explicit direction takes precedence.

## Verification

- Use the smallest relevant check while iterating. Do not run repository-wide test suites unless requested; CI owns full verification.
- For frontend changes, run `npm run check` and only affected tests with `npm run test -- <test-file>`.
- For Rust changes, run `cargo check -j 2 --manifest-path src-tauri/Cargo.toml`.
- Run only relevant Rust tests with `cargo test -j 2 --manifest-path src-tauri/Cargo.toml <filter> -- --test-threads=2`.

## UI Verification

When `$CODEMUX` is set, use `codemux browser` instead of a system browser. For UI changes, run `npm run dev`, inspect `http://localhost:1420`, and capture visual evidence with `codemux browser screenshot`. The development server uses the Tauri mock; use `npm run tauri:dev` when real IPC is required. Stop the development server when finished. Run `codemux browser --help` to discover commands.

## Ports

Several worktrees of one project share the host network, so bringing up a stack on its default ports collides with whatever another worktree is already running. When you hit that, reach for `codemux ports allocate <name>` — it prints a free port reserved for this worktree, and the same name always returns the same port, so an ephemeral compose file or `.env` you write against it keeps working across restarts. No other worktree is ever handed that port. Use `codemux ports list` to see what this worktree owns and `codemux ports release <name>` when you are done with it. Reach for this instead of asking to stop another worktree's stack, and only put allocated ports in ephemeral files you created yourself — never rewrite the project's own compose or config files.

## Process Safety

- Never kill processes by name or pattern; stop only processes you started.

## Pull Requests

- Rebase onto the latest `main` before opening a pull request. Stale branches create unnecessary conflicts.
- Use a concise conventional-commit title in plain language.
- Body: the problem in a sentence or two, then how you fixed it. End with the model and harness that did the work.
- UI changes need before/after screenshots in the pull request. Use mock data and exclude sensitive or personal information.
