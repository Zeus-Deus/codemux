# Application Theming — Completed Implementation Record

- Purpose: Preserve the shipped implementation decisions for the application-theming program.
- Audience: Maintainers investigating the origin of the current system.
- Authority: Historical implementation record; current behavior lives in `docs/features/theming.md`.
- Status: SHIPPED

## Goal

Replace the preset-only appearance row and split color ownership with a coherent, extensible dark-theme system spanning app chrome, terminal, editor, and chat.

## Landed

1. Defined a versioned theme schema with 26 semantic roles, 16 ANSI slots, five managed built-ins, and a namespaced CSS bridge.
2. Added atomic application plus a validated pre-React boot shadow to prevent startup flash.
3. Unified xterm, CodeMirror, and Shiki behind one app/system syntax-theme source.
4. Added a polished Appearance theme gallery and Theme Studio with live preview.
5. Added contrast-aware two-color OKLCH generation.
6. Added Codemux JSON, shadcn CSS-variable, and VS Code JSONC import; file/paste flows; export and deletion. Generated themes retain editable seeds, while imported themes expose every semantic and ANSI role for direct editing.
7. Synced the selected theme and custom-theme payloads with backward-compatible Rust/TypeScript defaults.
8. Migrated the legacy warm setting and removed the fake/no-op appearance controls.
9. Added engine, round-trip, import, boot-parity, terminal teardown, UI, and hardcoded-color contract coverage.

## Deliberate Scope

- Dark application themes only.
- No marketplace or URL installation.
- VS Code token-scope rules are ignored so syntax remains consistent across Codemux code surfaces.

## Current Truth

See `docs/features/theming.md` and `docs/reference/DESIGN-SYSTEM.md`.
