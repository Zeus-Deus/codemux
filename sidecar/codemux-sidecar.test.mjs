// Role selection for the bundled sidecar. The unit tests run anywhere; the
// binary tests need the compiled artifact, because only there is
// `process.execPath` the multi-role binary itself:
//
//   CODEMUX_SIDECAR_TEST_BINARY=src-tauri/binaries/codemux-sidecar-<triple> \
//     node --test sidecar/codemux-sidecar.test.mjs
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { ROLE_ENV, selectRole } from "./sidecar-role.mjs";

const argv = (...rest) => ["/bin/codemux-sidecar", "/$bunfs/root/codemux-sidecar", ...rest];

test("a leading role argument wins over the environment and is removed", () => {
  assert.deepEqual(selectRole(argv("--codemux-role=managed-workflow", "--check"), { [ROLE_ENV]: "plugin-host" }), {
    role: "managed-workflow", argv: argv("--check"), source: "argument",
  });
});
test("a role argument anywhere but first is an ordinary argument", () => {
  assert.deepEqual(selectRole(argv("--check", "--codemux-role=plugin-host"), {}), {
    role: "claude", argv: argv("--check", "--codemux-role=plugin-host"), source: "default",
  });
});
test("the environment applies when there is no role argument", () => {
  assert.deepEqual(selectRole(argv("-e", "1"), { [ROLE_ENV]: "plugin-host" }), {
    role: "plugin-host", argv: argv("-e", "1"), source: "environment",
  });
});
test("no argument and no (or an empty) environment role means Claude", () => {
  for (const env of [{}, { [ROLE_ENV]: "" }]) {
    assert.deepEqual(selectRole(argv(), env), { role: "claude", argv: argv(), source: "default" });
  }
});

const binary = process.env.CODEMUX_SIDECAR_TEST_BINARY;
const skip = binary ? false : "set CODEMUX_SIDECAR_TEST_BINARY to the compiled codemux-sidecar";
function run(args, { env = {}, input } = {}) {
  const result = spawnSync(binary, args, {
    // Only PATH and the variables under test: an inherited role must not leak in.
    env: { PATH: process.env.PATH, ...env }, input, encoding: "utf8", timeout: 30_000,
  });
  assert.equal(result.error, undefined);
  return result;
}
const PLUGIN_HOST_CHECK = '{"protocol":1,"runtime":"codemux-plugin-host"}\n';

test("compiled: no role runs the Claude sidecar", { skip }, () => {
  const result = run([], { input: '{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}\n' });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /"id":1,"result":\{"pong":true/);
});
test("compiled: role arguments select the plugin host and managed workflow sidecar", { skip }, () => {
  assert.equal(run(["--codemux-role=plugin-host", "--check"]).stdout, PLUGIN_HOST_CHECK);
  const managed = JSON.parse(run(["--codemux-role=managed-workflow", "--check"]).stdout);
  assert.equal(managed.protocolVersion, 1);
});
test("compiled: the environment selects the role without an argument, and the argument wins", { skip }, () => {
  assert.equal(run(["--check"], { env: { [ROLE_ENV]: "plugin-host" } }).stdout, PLUGIN_HOST_CHECK);
  const managed = run(["--codemux-role=managed-workflow", "--check"], { env: { [ROLE_ENV]: "plugin-host" } });
  assert.equal(JSON.parse(managed.stdout).protocolVersion, 1);
});
test("compiled: an unknown role fails before any program starts", { skip }, () => {
  for (const [args, env] of [[["--codemux-role=nope"], {}], [[], { [ROLE_ENV]: "nope" }]]) {
    const result = run(args, { env });
    assert.equal(result.status, 2);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /unknown role "nope"/);
  }
});
