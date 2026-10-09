// Policy tests need no provider calls. The native regression additionally uses
// CODEMUX_TEST_BUN_SIDECAR, staged by CI, and CODEMUX_TEST_PATCHELF or PATH.
import assert from "node:assert/strict";
import { chmod, copyFile, cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { tauriBuildEnv } from "./tauri-env.mjs";

const wrapper = fileURLToPath(new URL("./appimage-patchelf.sh", import.meta.url));
const linux = process.platform === "linux";
const fixtureName = "codemux-managed-workflow-sidecar-x86_64-unknown-linux-gnu";
async function ownedFixture(callback) {
  const root = await mkdtemp(join(tmpdir(), "codemux appimage-"));
  try { await callback(root); } finally { await rm(root, { recursive: true, force: true }); }
}
function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", timeout: 30000, ...options });
  assert.equal(result.status, 0, result.stderr || result.error?.message);
  return result;
}
async function executable(path, source) {
  await writeFile(path, source); await chmod(path, 0o755);
}

test("Tauri protects Linux bundles in a child environment and preserves explicit patcher choice", { skip: !linux }, async () => {
  await ownedFixture(async (root) => {
    const patcher = join(root, "real-patchelf");
    await executable(patcher, "#!/bin/sh\nexit 0\n");
    const env = { ...process.env, PATCHELF: patcher, CODEMUX_TEST_UNRELATED: "kept" };
    const before = { ...env };
    const protectedEnv = tauriBuildEnv(["build"], env);
    assert.equal(protectedEnv.PATCHELF, wrapper);
    assert.equal(protectedEnv.CODEMUX_REAL_PATCHELF, patcher);
    assert.equal(protectedEnv.CODEMUX_TEST_UNRELATED, "kept");
    assert.deepEqual(env, before);
    for (const args of [["--verbose", "build"], ["bundle"], ["build", "--bundles=deb,appimage"], ["bundle", "-b", "all"]]) assert.equal(tauriBuildEnv(args, env).PATCHELF, wrapper);
    for (const [args, platform] of [[["dev"], "linux"], [["build", "--no-bundle"], "linux"], [["build", "--help"], "linux"], [["build"], "win32"], [["build"], "darwin"]]) {
      assert.equal(tauriBuildEnv(args, env, platform), env);
    }
    assert.throws(() => tauriBuildEnv(["build"], { ...env, PATCHELF: wrapper }), /actual patcher/);
    const noPatcher = { ...env, PATCHELF: join(root, "missing-patchelf") };
    for (const args of [["build", "--bundles", "deb"], ["bundle", "-b", "rpm"], ["build", "--bundles=deb,rpm"], ["bundle", "-b=deb"], ["build", "--bundles", "deb", "rpm"]]) {
      assert.equal(tauriBuildEnv(args, noPatcher), noPatcher);
    }
  });
});

test("only exact RPATH writes to owned GNU Bun sidecars are skipped; other arguments and failures pass through", { skip: !linux }, async () => {
  await ownedFixture(async (root) => {
    const patcher = join(root, "real-patchelf"), log = join(root, "arguments");
    await executable(patcher, '#!/bin/bash\nprintf "%s\\0" "$@" > "$CODEMUX_TEST_ARGS"\nexit 27\n');
    await executable(join(root, "readelf"), '#!/bin/sh\nprintf "[4] .bun PROGBITS\\n"\n');
    const env = { ...process.env, PATH: `${root}:${process.env.PATH}`, CODEMUX_REAL_PATCHELF: patcher, CODEMUX_TEST_ARGS: log };
    const resource = join(root, "codemux.AppDir/usr/lib/codemux/binaries");
    for (const name of ["codemux-managed-workflow-sidecar", "codemux-claude-sidecar"]) {
      for (const target of ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]) run(wrapper, ["--set-rpath", "$ORIGIN", join(resource, `${name}-${target}`)], { env });
    }
    const owned = join(resource, fixtureName);
    for (const args of [
      ["--print-rpath", owned], ["--remove-rpath", owned], ["--set-rpath", "$ORIGIN", owned, "extra"],
      ["--set-rpath", "$ORIGIN", join(root, fixtureName)],
      ["--set-rpath", "$ORIGIN", join(resource, "codemux-managed-workflow-sidecar-x86_64-unknown-linux-musl")],
      ["--set-rpath", "$ORIGIN", join(resource, "codemux-managed-workflow-sidecar-x86_64-pc-windows-msvc.exe")],
      ["--set-rpath", "$ORIGIN", join(resource, "node_modules/@cursor/sdk-linux-x64/vendor/tree-sitter/binding.node")],
    ]) {
      const result = spawnSync(wrapper, args, { env });
      assert.equal(result.status, 27);
      assert.deepEqual((await readFile(log, "utf8")).split("\0").slice(0, -1), args);
    }
    // A matching filename alone cannot opt an ordinary ELF out of relocation.
    await executable(join(root, "readelf"), '#!/bin/sh\nprintf "[4] .text PROGBITS\\n"\n');
    assert.equal(spawnSync(wrapper, ["--set-rpath", "$ORIGIN", owned], { env }).status, 27);
    for (const [arch, helper] of [["x64", "rg"], ["arm64", "rg"], ["x64", "cursorsandbox"], ["arm64", "cursorsandbox"]]) {
      const native = join(resource, `node_modules/@cursor/sdk-linux-${arch}/bin/${helper}`);
      run(wrapper, ["--set-rpath", "$ORIGIN", native], { env });
      // Either a needed shared library or an interpreter keeps real relocation.
      for (const metadata of ["(NEEDED) Shared library: libc.so", " INTERP "]) {
        await executable(join(root, "readelf"), `#!/bin/sh\nprintf '%s\\n' '${metadata}'\n`);
        assert.equal(spawnSync(wrapper, ["--set-rpath", "$ORIGIN", native], { env }).status, 27);
      }
      await executable(join(root, "readelf"), '#!/bin/sh\nprintf "[4] .text PROGBITS\\n"\n');
    }
  });
});

test("actual Bun sidecar stays byte-identical, dynamically loadable and native-ready after AppImage relocation", { skip: !linux || !process.env.CODEMUX_TEST_BUN_SIDECAR }, async () => {
  await ownedFixture(async (root) => {
    const source = resolve(process.env.CODEMUX_TEST_BUN_SIDECAR);
    const resource = join(root, "codemux.AppDir/usr/lib/codemux/binaries");
    await mkdir(resource, { recursive: true });
    const sidecar = join(resource, fixtureName);
    await copyFile(source, sidecar);
    await cp(join(dirname(source), "node_modules"), join(resource, "node_modules"), { recursive: true });
    const real = process.env.CODEMUX_TEST_PATCHELF || "patchelf";
    run("ldd", [sidecar]);
    run(wrapper, ["--set-rpath", "$ORIGIN", sidecar], { env: { ...process.env, CODEMUX_REAL_PATCHELF: real } });
    assert.deepEqual(await readFile(sidecar), await readFile(source));
    run("ldd", [sidecar]);
    const readiness = JSON.parse(run(sidecar, ["--check-native"]).stdout);
    assert.equal(readiness.modelTurns, 0);
    assert.equal(readiness.ripgrepReady, true);
    assert.equal(readiness.parserReady, true);
    assert.equal(readiness.workspaceReady, true);
    const rg = join(resource, "node_modules/@cursor/sdk-linux-x64/bin/rg");
    const rgBytes = await readFile(rg);
    run(wrapper, ["--set-rpath", "$ORIGIN", rg], { env: { ...process.env, CODEMUX_REAL_PATCHELF: real } });
    assert.deepEqual(await readFile(rg), rgBytes);
    assert.match(run(rg, ["--version"]).stdout, /^ripgrep /);
    const sandbox = join(resource, "node_modules/@cursor/sdk-linux-x64/bin/cursorsandbox");
    const sandboxBytes = await readFile(sandbox);
    run(wrapper, ["--set-rpath", "$ORIGIN", sandbox], { env: { ...process.env, CODEMUX_REAL_PATCHELF: real } });
    assert.deepEqual(await readFile(sandbox), sandboxBytes);
    assert.match(run(sandbox, ["--help"]).stdout, /Sandboxing helper/);
    // The SDK's native files still use the real patcher; this compiled native
    // fixture proves relocation and execution, even under a matching filename.
    const nativeSource = join(root, "fixture.c"), native = join(resource, "codemux-claude-sidecar-x86_64-unknown-linux-gnu");
    await writeFile(nativeSource, '#include <stdio.h>\nint main(void) { puts("native helper"); return 0; }\n');
    run("cc", [nativeSource, "-o", native]);
    run(wrapper, ["--set-rpath", "$ORIGIN", native], { env: { ...process.env, CODEMUX_REAL_PATCHELF: real } });
    assert.equal(run(real, ["--print-rpath", native]).stdout.trim(), "$ORIGIN");
    assert.equal(run(native, []).stdout.trim(), "native helper");
  });
});
