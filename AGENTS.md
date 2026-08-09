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

When `$CODEMUX` is set, use `codemux browser` instead of a system browser. For UI changes, run `npm run dev`, inspect `http://localhost:1420`, and capture visual evidence with `codemux browser screenshot`. The development server uses the Tauri mock; use `npm run tauri:dev` when real IPC is required. Run `codemux browser --help` to discover commands.

## Process Safety

- Never kill processes by name or pattern; stop only processes you started.

## Pull Requests

- Rebase onto the latest `main` before opening a pull request. Stale branches create unnecessary conflicts.
