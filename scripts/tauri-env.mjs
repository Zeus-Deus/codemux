import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const protectedPatchelf = fileURLToPath(new URL("./appimage-patchelf.sh", import.meta.url));

function includesAppImage(args) {
  const formats = [];
  for (let index = 0; index < args.length && args[index] !== "--"; index++) {
    const arg = args[index];
    if (arg === "--bundles" || arg === "-b" || arg.startsWith("--bundles=") || arg.startsWith("-b=")) {
      if (arg.includes("=")) formats.push(...arg.slice(arg.indexOf("=") + 1).split(","));
      while (index + 1 < args.length && !args[index + 1].startsWith("-")) formats.push(...args[++index].split(","));
    }
  }
  return formats.length === 0 || formats.includes("all") || formats.includes("appimage");
}

// Keep the override inside the Tauri child process; never replace installed tools
// or change a user's shell configuration. linuxdeploy forwards it to GTK's pass.
export function tauriBuildEnv(args, env = process.env, platform = process.platform) {
  const command = args.find((arg) => !arg.startsWith("-"));
  if (platform !== "linux" || !["build", "bundle"].includes(command) || args.some((arg) => ["--no-bundle", "--help", "-h"].includes(arg)) || !includesAppImage(args)) return env;
  const result = spawnSync("which", [env.PATCHELF || "patchelf"], { encoding: "utf8", env });
  if (result.status !== 0 || !result.stdout.trim()) throw Error("Linux bundle builds require patchelf on PATH (or an explicit PATCHELF executable)");
  const realPatchelf = realpathSync(result.stdout.trim());
  if (realPatchelf === realpathSync(protectedPatchelf)) throw Error("PATCHELF must name the actual patcher, not CodeMux's wrapper");
  return { ...env, CODEMUX_REAL_PATCHELF: realPatchelf, PATCHELF: protectedPatchelf };
}
