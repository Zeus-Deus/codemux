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
// The role is an argv token rather than an environment variable so it is never
// inherited by processes a role spawns (the plugin host runs third-party code,
// and the Claude sidecar runs the `claude` CLI and its tools). It must be the
// first argument, and it is removed before the program starts so programs that
// read `process.argv` (`--check`, `--check-native`) see their usual arguments.
//
// No role means Claude: SSH devices install this binary as
// `~/.local/bin/codemux-claude-sidecar` and start it without arguments, as
// they did when it was a Claude-only binary.
//
// Programs are imported lazily, so only the selected program's module graph
// is evaluated and an import-time failure in one cannot affect the others.

const ROLE_PREFIX = "--codemux-role=";

const programs = {
  claude: () => import("./claude-agent/src/main.ts"),
  "managed-workflow": () => import("./managed-workflow/src/main.ts"),
  "plugin-host": () => import("./plugin-host/main.mjs"),
};

let role = "claude";
const first = process.argv[2];
if (first !== undefined && first.startsWith(ROLE_PREFIX)) {
  role = first.slice(ROLE_PREFIX.length);
  process.argv.splice(2, 1);
}

if (!Object.hasOwn(programs, role)) {
  // stdout belongs to each program's protocol; diagnostics go to stderr.
  process.stderr.write(
    `codemux-sidecar: unknown role "${role}" (expected one of: ${Object.keys(programs).join(", ")})\n`,
  );
  process.exit(2);
}

await programs[role]();
