// Reproducible standalone measurements. Passing does not establish packaged GUI performance.
import { fault } from "./host-limits.mjs";
import { cpus, release, platform, tmpdir } from "node:os";
import { readFile, writeFile, mkdtemp, rm } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve, join } from "node:path";
const host = resolve(process.argv[2]);
const manifest = JSON.parse(
  await readFile("src-tauri/addon-protocol/fixtures/hello.json", "utf8"),
);
const directory = await mkdtemp(join(tmpdir(), "cmx-limits-"));
const workloads = [
  "while(true){}",
  "function f(){f()} f()",
  "new ArrayBuffer(128*1024*1024)",
  "Promise.resolve().then(function loop(){Promise.resolve().then(loop)})",
  "throw Error('synthetic')",
];
const evidence = {
  packaged: false,
  os: platform() + " " + release(),
  cpu: cpus()[0].model,
  logicalCpus: cpus().length,
  sha256: createHash("sha256")
    .update(await readFile(host))
    .digest("hex"),
  samplesPerWorkload: 20,
  deadlineMs: 2000,
  workloads: [],
};
try {
  for (const workload of workloads) {
    const samples = [];
    for (let i = 0; i < 20; i++)
      samples.push(await fault(host, workload, manifest, directory));
    samples.sort((a, b) => a - b);
    evidence.workloads.push({
      workload,
      minMs: samples[0],
      p95Ms: samples[18],
      maxMs: samples[19],
    });
  }
  await writeFile(
    "addon-host-evidence.json",
    JSON.stringify(evidence, null, 2) + "\n",
  );
  console.log(
    "PASS: 100 native-host fault samples; see addon-host-evidence.json for hardware and timing",
  );
} finally {
  await rm(directory, { recursive: true, force: true });
}
