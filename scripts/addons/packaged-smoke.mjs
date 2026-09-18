// Inspect actual installer payloads, then execute only their bundled native host.
// This is a packaged-runtime gate, not a substitute for desktop GUI acceptance.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { cpus, platform, release, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fault } from "./host-limits.mjs";
import { verifyAppImageElf } from "./elf-provenance.mjs";
const bundleRoot = resolve(
  process.argv[2] ?? "src-tauri/target/release/bundle",
);
const root = await mkdtemp(join(tmpdir(), "cmx-addon-bundle-"));
const hostName =
  process.platform === "win32"
    ? "codemux-addon-host-windows-x64.exe"
    : "codemux-addon-host-linux-x64";
const releaseHost = await readFile(`src-tauri/binaries/${hostName}`);
const expected = createHash("sha256").update(releaseHost).digest("hex");
assert.match(
  await readFile(`src-tauri/binaries/.${hostName}.profile`, "utf8"),
  /^profile=release$/m,
);
const manifest = JSON.parse(
  await readFile("src-tauri/addon-protocol/fixtures/hello.json", "utf8"),
);
const evidence = {
  platform: platform(),
  os: release(),
  cpu: cpus()[0]?.model,
  logicalCpus: cpus().length,
  hostSha256: expected,
  bundles: [],
};
async function files(path) {
  const found = [];
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) found.push(...(await files(child)));
    else if (entry.isFile()) found.push(child);
  }
  return found;
}
function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    stdio: "inherit",
    timeout: 180000,
    ...options,
  });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${command} failed`);
}

try {
  const bundles = await files(bundleRoot);
  const formats =
    process.platform === "win32" ? [".exe"] : [".deb", ".AppImage"];
  for (const format of formats) {
    const matches = bundles.filter((file) => file.endsWith(format));
    assert.equal(matches.length, 1, `Expected exactly one ${format} installer`);
    const unpack = join(root, format.slice(1));
    const { mkdir } = await import("node:fs/promises");
    await mkdir(unpack);
    if (format === ".deb") run("dpkg-deb", ["-x", matches[0], unpack]);
    else if (format === ".AppImage")
      run(matches[0], ["--appimage-extract"], { cwd: unpack, stdio: "ignore" });
    else run(matches[0], ["/S", `/D=${unpack}`]);
    const hosts = (await files(unpack)).filter((file) =>
      file.endsWith(hostName),
    );
    assert.equal(
      hosts.length,
      1,
      "Installer must contain exactly one platform host",
    );
    const host = hosts[0];
    const packagedBytes = await readFile(host);
    const packagedSha256 = createHash("sha256")
      .update(packagedBytes)
      .digest("hex");
    let provenance = "exact-sha256";
    if (format === ".AppImage" && packagedSha256 !== expected) {
      provenance = verifyAppImageElf(releaseHost, packagedBytes);
    } else {
      assert.equal(
        packagedSha256,
        expected,
        "Installer host differs from release build",
      );
    }
    run(process.execPath, ["scripts/addons/sdk-native.mjs", host]);
    const workloads = [
      "while(true){}",
      "function f(){f()} f()",
      "new ArrayBuffer(128*1024*1024)",
      "Promise.resolve().then(function loop(){Promise.resolve().then(loop)})",
      "throw Error('fixture')",
    ];
    const faults = [];
    for (const workload of workloads)
      faults.push({
        workload,
        elapsedMs: await fault(host, workload, manifest, root),
      });
    evidence.bundles.push({
      format,
      packagedSha256,
      provenance,
      hostPath: host.slice(unpack.length + 1),
      faults,
    });
    await writeFile(
      "addon-packaged-evidence.json",
      JSON.stringify(evidence, null, 2) + "\n",
    );
    if (format === ".exe") {
      const uninstall = (await files(unpack)).find((file) =>
        /uninstall\.exe$/i.test(file),
      );
      assert.ok(uninstall, "NSIS must include its uninstaller");
      run(uninstall, ["/S"]);
    }
  }
  await writeFile(
    "addon-packaged-evidence.json",
    JSON.stringify(evidence, null, 2) + "\n",
  );
  console.log(
    "PASS: installer host provenance, clean-environment SDK callbacks and packaged hostile-runtime deadlines",
  );
} finally {
  await rm(root, {
    recursive: true,
    force: true,
    maxRetries: 10,
    retryDelay: 500,
  });
}
