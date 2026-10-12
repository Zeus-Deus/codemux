// Cross-platform, credential-free acceptance against the actual npm CLIs.
// Run after staging Tauri build resources. CI executes the ignored Rust tests
// explicitly, so the real-package lane cannot pass by silently skipping them.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { access, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { homedir, tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { startFixtureModel, fixtureConfig } from './opencode-fixture-model.mjs';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const root = await mkdtemp(join(tmpdir(), 'codemux-opencode-'));
const evidenceDir = process.env.CODEMUX_OPENCODE_EVIDENCE ?? join(root, 'evidence');
await mkdir(evidenceDir, { recursive: true });
const summary = { checks: [], scriptedLoopbackModel: true };
const owned = new Set();
const pathKey = Object.keys(process.env).find(key => key.toLowerCase() === 'path');
assert.ok(pathKey);
const hostCargoHome = process.env.CARGO_HOME ?? join(homedir(), '.cargo');
const hostRustupHome = process.env.RUSTUP_HOME ?? join(homedir(), '.rustup');

async function run(program, args, env, logName, timeoutMs = 180000) {
  const child = spawn(program, args, { env, cwd: repository, detached: process.platform !== 'win32', windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  owned.add(child);
  let output = '';
  child.stdout.on('data', data => { output += data; });
  child.stderr.on('data', data => { output += data; });
  const done = new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', (code, signal) => resolve({ code, signal })); });
  const timer = setTimeout(() => { void stop(child); }, timeoutMs);
  try {
    const result = await done;
    await writeFile(join(evidenceDir, logName), output);
    assert.equal(result.code, 0, `${logName}: ${output.slice(-12000)}`);
    return output;
  } finally { clearTimeout(timer); owned.delete(child); }
}

async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, 'exit').catch(() => {});
  if (process.platform === 'win32') {
    const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
    await once(killer, 'exit');
  } else {
    try { process.kill(-child.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
  }
  await exited;
}

function isolatedEnvironment(runDir) {
  const env = { [pathKey]: process.env[pathKey], HOME: join(runDir, 'home'), USERPROFILE: join(runDir, 'home'), USER: 'tester', LANG: 'C.UTF-8', CARGO_HOME: hostCargoHome, RUSTUP_HOME: hostRustupHome };
  for (const key of ['SystemRoot', 'SYSTEMROOT', 'WINDIR', 'ComSpec', 'COMSPEC', 'PATHEXT', 'RUSTUP_HOME', 'CARGO_BUILD_TARGET', 'CARGO_PROFILE_DEV_DEBUG', 'CARGO_PROFILE_TEST_DEBUG']) if (process.env[key]) env[key] = process.env[key];
  for (const kind of ['CONFIG', 'DATA', 'CACHE', 'STATE']) env[`XDG_${kind}_HOME`] = join(runDir, kind.toLowerCase());
  env.APPDATA = join(runDir, 'appdata'); env.LOCALAPPDATA = join(runDir, 'localappdata');
  env.TMPDIR = env.TEMP = env.TMP = join(runDir, 'tmp');
  env.OPENCODE_CONFIG_DIR = join(runDir, 'opencode');
  env.OPENCODE_DISABLE_MODELS_FETCH = env.OPENCODE_DISABLE_AUTOUPDATE = 'true';
  env.OPENCODE_DISABLE_PROJECT_CONFIG = env.OPENCODE_CONFIG_PROJECT_DISABLE = 'true';
  env.OPENCODE_DISABLE_FILEWATCHER = env.OPENCODE_FILEWATCHER_DISABLE = 'true';
  // Regression: v2 must not honor an ambient password instead of the owned
  // server's freshly minted secret. No actual credential is involved here.
  env.OPENCODE_PASSWORD = 'hostile-test-only-password';
  return env;
}

async function npmCli() {
  const candidates = [join(dirname(process.execPath), 'node_modules/npm/bin/npm-cli.js'), join(dirname(process.execPath), '../lib/node_modules/npm/bin/npm-cli.js')];
  for (const candidate of candidates) { try { await access(candidate); return candidate; } catch {} }
  throw Error('Cannot locate the prepared Node installation\'s npm CLI; no shell fallback');
}

const versions = [[1, '1.18.35', 'opencode-ai'], [2, '2.0.27', '@opencode/cli']];
// The legacy npm wrapper is a different Windows layout: gate it against the
// real package too, not only portable marker-file resolver tests.
if (process.platform === 'win32') versions.push([1, '1.14.31', 'opencode-ai']);
try {
  for (const [major, version, name] of versions) {
    const runDir = join(root, `version ${version} with spaces`);
    const env = isolatedEnvironment(runDir);
    for (const path of new Set(Object.entries(env).filter(([key]) => ['HOME', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'TMPDIR', 'TEMP', 'TMP', 'OPENCODE_CONFIG_DIR'].includes(key) || key.startsWith('XDG_')).map(([, value]) => value))) await mkdir(path, { recursive: true });
    const checkout = join(runDir, 'checkout'); await mkdir(checkout);
    const model = await startFixtureModel(checkout);
    try {
      await writeFile(join(env.OPENCODE_CONFIG_DIR, 'opencode.json'), JSON.stringify(fixtureConfig(major, model.baseURL)));
      const legacyWindows = process.platform === 'win32' && version === '1.14.31';
      let binary = process.env[legacyWindows ? 'CODEMUX_TEST_OPENCODE_LEGACY_BINARY' : `CODEMUX_TEST_OPENCODE_V${major}_BINARY`];
      let launcher;
      if (!binary) {
        const prefix = join(runDir, 'npm install');
        const userconfig = join(runDir, 'npmrc'); await writeFile(userconfig, '');
        const globalconfig = join(runDir, 'npm-globalrc'); await writeFile(globalconfig, '');
        await run(process.execPath, [await npmCli(), 'install', '--prefix', prefix, '--cache', join(runDir, 'npm-cache'), '--userconfig', userconfig, '--globalconfig', globalconfig, '--registry', 'https://registry.npmjs.org', '--no-audit', '--no-fund', '--save-exact', '--ignore-scripts=false', `${name}@${version}`], env, `npm-v${version}.log`);
        const packageDir = join(prefix, 'node_modules', name);
        const metadata = JSON.parse(await readFile(join(packageDir, 'package.json'), 'utf8'));
        assert.equal(metadata.name, name); assert.equal(metadata.version, version);
        binary = join(packageDir, typeof metadata.bin === 'string' ? metadata.bin : metadata.bin.opencode);
        launcher = process.platform === 'win32' ? join(prefix, 'node_modules', '.bin', 'opencode.cmd') : binary;
        if (legacyWindows) {
          assert.equal(metadata.bin.opencode, './bin/opencode');
          const packages = process.arch === 'x64'
            ? ['opencode-windows-x64-baseline', 'opencode-windows-x64']
            : process.arch === 'arm64' ? ['opencode-windows-arm64'] : [];
          binary = undefined;
          for (const dependency of packages) {
            for (const modules of [join(packageDir, 'node_modules'), join(prefix, 'node_modules')]) {
              const dependencyDir = join(modules, dependency);
              try {
                const info = JSON.parse(await readFile(join(dependencyDir, 'package.json'), 'utf8'));
                assert.equal(info.name, dependency); assert.equal(info.version, version);
                const candidate = join(dependencyDir, 'bin', 'opencode.exe');
                await access(candidate); binary = candidate; break;
              } catch (error) { if (error.code !== 'ENOENT') throw error; }
            }
            if (binary) break;
          }
          assert.ok(binary, 'Legacy Windows fixture requires the real installed native optional dependency');
        }
      } else { binary = resolve(binary); launcher = binary; }
      await access(binary); await access(launcher);
      const detected = (await run(binary, ['--version'], env, `version-v${version}.log`, 20000)).trim();
      assert.ok(detected.endsWith(version), 'Actual executable version must match the reviewed package');
      env.CODEMUX_OPENCODE_BINARY = launcher;
      env.CODEMUX_TEST_OPENCODE_BINARY = binary;
      env.CODEMUX_TEST_OPENCODE_CWD = checkout;
      env.CODEMUX_TEST_OPENCODE_API = String(major);
      const executable = process.env.CODEMUX_TEST_OPENCODE_TEST_EXECUTABLE;
      const output = executable
        ? await run(resolve(executable), ['--ignored', '--test-threads=1', '--nocapture'], env, `native-v${version}.log`)
        : await run('cargo', ['test', '-j', '2', '--locked', '--manifest-path', join(repository, 'src-tauri/Cargo.toml'), '--test', 'opencode_native_protocol', '--', '--ignored', '--test-threads=1', '--nocapture'], env, `native-v${version}.log`, 600000);
      const results = [...output.matchAll(/test result: ok\. (\d+) passed; 0 failed;/g)];
      assert.equal(results.length, 1, 'The native acceptance target must run, not silently skip');
      assert.equal(Number(results[0][1]), 3, 'All three actual-runtime acceptance cases must pass');
      assert.equal(await readFile(join(checkout, 'opencode-proof.txt'), 'utf8'), 'OpenCode compatibility verified\n');
      assert.deepEqual(model.errors, [], 'The loopback peer must not hide fixture failures');
      assert.equal(model.heldCount, 0, 'Interrupt/cleanup must close the real model stream');
      summary.checks.push({ major, version, nativeModels: true, approvalAndFileWrite: true, interruption: true, durableResume: true, childReaped: true, explicitShutdownReaped: true, modelRequests: model.requests.length });
      console.log(`Real OpenCode ${version}: catalog, approval, native tool write, interruption, durable resume and owned cleanup passed (scripted loopback model).`);
    } finally { await model.stop(); }
  }
} catch (error) {
  summary.failure = String(error);
  throw error;
} finally {
  for (const child of owned) await stop(child);
  await writeFile(join(evidenceDir, 'summary.json'), JSON.stringify(summary, null, 2));
}
