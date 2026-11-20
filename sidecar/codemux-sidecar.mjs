// Entry point for the single bundled `codemux-sidecar` executable.
//
// Codemux's three Bun programs share one compiled binary so the installer
// carries one Bun runtime instead of three. Each program still runs in its
// own process; the role only decides which program this process becomes.
//
//   codemux-sidecar                                 -> Claude sidecar
//   codemux-sidecar --codemux-role=claude           -> Claude sidecar
//   codemux-sidecar --codemux-role=managed-workflow -> managed workflow sidecar
//   codemux-sidecar --codemux-role=plugin-host      -> plugin host
//
// Selection order (see sidecar-role.mjs):
//   1. A leading `--codemux-role=<role>` argument. It must be the first
//      argument and is removed before the program starts, so programs that
//      read `process.argv` (`--check`, `--check-native`) see their usual
//      arguments. Codemux always selects the managed workflow and plugin host
//      roles this way.
//   2. `CODEMUX_SIDECAR_ROLE=<role>` in the environment. Codemux sets it only
//      when it starts the plugin host (src-tauri/src/bundled_sidecar.rs), so
//      plugin code that re-executes `process.execPath` (this binary) with
//      ordinary arguments gets the plugin host again rather than Claude, as
//      when the plugin host was its own binary. It has to come from the
//      launcher: Bun child processes do not see assignments to `process.env`.
//   3. Claude. SSH devices install this binary as
//      `~/.local/bin/codemux-claude-sidecar` and start it without arguments,
//      as they did when it was a Claude-only binary.
//
// Programs are imported lazily, so only the selected program's module graph
// is evaluated and an import-time failure in one cannot affect the others.

import { ROLES, ROLE_ENV, selectRole } from "./sidecar-role.mjs";

const programs = {
  claude: () => import("./claude-agent/src/main.ts"),
  "managed-workflow": () => import("./managed-workflow/src/main.ts"),
  "plugin-host": () => import("./plugin-host/main.mjs"),
};

const { role, argv, source } = selectRole(process.argv, process.env);

if (!Object.hasOwn(programs, role)) {
  // stdout belongs to each program's protocol; diagnostics go to stderr.
  const from = source === "environment" ? ` from ${ROLE_ENV}` : "";
  process.stderr.write(
    `codemux-sidecar: unknown role "${role}"${from} (expected one of: ${ROLES.join(", ")})\n`,
  );
  process.exit(2);
}

process.argv.splice(0, process.argv.length, ...argv);

await programs[role]();
