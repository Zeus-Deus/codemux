// Run `tauri build`, then verify the exact installers it left in the bundle
// directory. The release action uses this as its build command, so it uploads
// only these verified files and never a second, unverified build.
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
function run(args, env = process.env) {
  const result = spawnSync(process.execPath, args, { stdio: "inherit", env });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
const args = process.argv.slice(2);
// Only a release build of the host platform writes the checked bundle path.
if (
  args[0] !== "build" ||
  args.some((arg) => /^(-d|--debug|-t|--target)(=|$)/.test(arg))
)
  throw Error("Usage: verified-tauri-build.mjs build [bundle options]");
run([require.resolve("@tauri-apps/cli/tauri.js"), ...args]);
// Inspecting and executing the packaged host needs no signing or upload token.
const secret =
  /^(TAURI_SIGNING_|ACTIONS_ID_TOKEN_)|^(GITHUB_TOKEN|GH_TOKEN|ACTIONS_RUNTIME_TOKEN)$/i;
const env = Object.fromEntries(
  Object.entries(process.env).filter(([name]) => !secret.test(name)),
);
run(
  ["scripts/addons/packaged-smoke.mjs", "src-tauri/target/release/bundle"],
  env,
);
