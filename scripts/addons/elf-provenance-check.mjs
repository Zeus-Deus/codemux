import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, writeFile, copyFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { test } from "node:test";
import { verifyAppImageElf } from "./elf-provenance.mjs";
test(
  "accept only RPATH relocation, reject changed program bytes and dependencies",
  { skip: process.platform !== "linux" },
  async () => {
    const root = await mkdtemp(join(tmpdir(), "cmx-elf-check-"));
    const run = (bin, args) => {
      const result = spawnSync(bin, args, { encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr || result.error?.message);
    };
    try {
      const source = join(root, "fixture.c"),
        original = join(root, "original"),
        patched = join(root, "patched");
      await writeFile(
        source,
        '#include <stdio.h>\nint main(void) { puts("fixture"); return 0; }\n',
      );
      run("cc", [source, "-o", original]);
      await copyFile(original, patched);
      run("patchelf", ["--set-rpath", "$ORIGIN", patched]);
      const before = await readFile(original),
        after = await readFile(patched);
      assert.equal(
        verifyAppImageElf(before, after),
        "elf-sections-and-dynamic-metadata",
      );
      const changed = Buffer.from(after);
      const location = changed.indexOf(Buffer.from("fixture\0"));
      assert.ok(location >= 0);
      changed[location] = 0x58;
      assert.throws(
        () => verifyAppImageElf(before, changed),
        /program section differs/,
      );
      run("patchelf", ["--add-needed", "unapproved.so", patched]);
      const dependency = await readFile(patched);
      assert.throws(
        () => verifyAppImageElf(before, dependency),
        /Dynamic-link metadata/,
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  },
);
