import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { tauriBuildEnv } from "./tauri-env.mjs";

const require = createRequire(import.meta.url);
const args = process.argv.slice(2);
const result = spawnSync(process.execPath, [require.resolve("@tauri-apps/cli/tauri.js"), ...args], {
  stdio: "inherit",
  env: tauriBuildEnv(args),
});
if (result.error) throw result.error;
process.exit(result.status ?? 1);
