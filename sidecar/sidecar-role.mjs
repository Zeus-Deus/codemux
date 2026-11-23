// Role selection for the bundled `codemux-sidecar` executable. Kept apart from
// the entry point so tests can exercise it without starting a program.

export const ROLE_ARG_PREFIX = "--codemux-role=";
export const ROLE_ENV = "CODEMUX_SIDECAR_ROLE";
export const DEFAULT_ROLE = "claude";
export const ROLES = ["claude", "managed-workflow", "plugin-host"];

/**
 * Pick the role for this process.
 *
 * A leading `--codemux-role=<role>` argument wins and is removed from the
 * returned argv. Otherwise a non-empty ROLE_ENV applies, then the default.
 *
 * @param {string[]} argv `process.argv`, not modified.
 * @param {Record<string, string | undefined>} env
 * @returns {{ role: string, argv: string[], source: "argument" | "environment" | "default" }}
 */
export function selectRole(argv, env) {
  const first = argv[2];
  if (first !== undefined && first.startsWith(ROLE_ARG_PREFIX)) {
    return {
      role: first.slice(ROLE_ARG_PREFIX.length),
      argv: [...argv.slice(0, 2), ...argv.slice(3)],
      source: "argument",
    };
  }
  const inherited = env[ROLE_ENV];
  if (inherited) return { role: inherited, argv: [...argv], source: "environment" };
  return { role: DEFAULT_ROLE, argv: [...argv], source: "default" };
}
