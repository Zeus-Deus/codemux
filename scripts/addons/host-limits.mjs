import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { performance } from "node:perf_hooks";
// Real host, empty inherited environment, a fixed hostile test workload.
export async function fault(host, source, manifest, directory) {
  const started = performance.now();
  const child = spawn(host, [], {
    cwd: directory,
    env:
      process.platform === "win32"
        ? { SystemRoot: process.env.SystemRoot }
        : {},
    stdio: ["pipe", "pipe", "ignore"],
  });
  child.stdin.on("error", () => {});
  let accepted = false,
    timer;
  const closed = new Promise((resolve) => child.once("close", resolve));
  const stopped = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", () => {
      if (!accepted)
        reject(new Error("Host exited before accepting the test workload"));
      else resolve(performance.now() - started);
    });
    timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("Host exceeded the 2 s fault deadline"));
    }, 2000);
  });
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const frame = JSON.parse(line);
    if (frame.result?.accepted) {
      accepted = true;
      child.stdin.write(
        JSON.stringify({
          jsonrpc: "2.0",
          generation: "limits-smoke",
          id: 2,
          method: "activate",
          params: {},
        }) + "\n",
      );
    }
  });
  child.stdin.write(
    JSON.stringify({
      jsonrpc: "2.0",
      generation: "limits-smoke",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: 1,
        manifest,
        source: `__codemuxRegister({},()=>()=>{${source}});`,
        final: true,
      },
    }) + "\n",
  );
  try {
    const elapsed = await stopped;
    assert.ok(elapsed < 2000);
    return elapsed;
  } finally {
    clearTimeout(timer);
    lines.close();
    child.stdin.destroy();
    if (child.exitCode === null && child.signalCode === null)
      child.kill("SIGKILL");
    await closed;
  }
}
