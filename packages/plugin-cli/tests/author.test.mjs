// The starter and every CLI command, run like a third-party author: packed
// SDK and CLI tarballs installed in a directory outside the app checkout.
// Set CODEMUX_AUTHOR_TARBALLS to the directory holding the packed SDK and CLI
// (scripts/addons/build-examples.sh does).
import assert from 'node:assert/strict';
import {spawn, spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {mkdtemp, readFile, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {test} from 'node:test';
import {fileURLToPath} from 'node:url';
import {gunzipSync} from 'node:zlib';
const packs = process.env.CODEMUX_AUTHOR_TARBALLS;
const tarballs = packs && ['sdk', 'cli'].map(name => join(packs, `codemux-plugin-${name}-1.0.0.tgz`));
const source = fileURLToPath(new URL('../src/cli.mjs', import.meta.url));
function run(command, args, cwd, expected = 0) {
 const result = spawnSync(command, args, {cwd, encoding: 'utf8', shell: process.platform === 'win32' && command === 'npm'});
 assert.equal(result.status, expected, `${command} ${args.join(' ')}:\n${result.stdout}\n${result.stderr}`);
 return result;
}
function entries(archive) {
 const tar = gunzipSync(archive), names = [];
 for (let offset = 0; tar[offset] !== 0; ) {
  names.push(tar.toString('utf8', offset, offset + 100).replace(/\0.*$/s, ''));
  const size = parseInt(tar.toString('ascii', offset + 124, offset + 135), 8);
  offset += 512 + Math.ceil(size / 512) * 512;
 }
 return names;
}
const digest = async path => {try {return createHash('sha256').update(await readFile(path)).digest('hex');} catch {return null;}};
async function until(predicate, what, ms = 30000) {
 const end = Date.now() + ms;
 while (Date.now() < end) {
  const value = await predicate();
  if (value) return value;
  await new Promise(r => setTimeout(r, 100));
 }
 throw Error('Timed out waiting for ' + what);
}
test('starter builds, checks, packs and watches from packed tarballs', {skip: !tarballs && 'CODEMUX_AUTHOR_TARBALLS is not set', timeout: 300000}, async t => {
 const workspace = await mkdtemp(join(tmpdir(), 'codemux-author-'));
 t.after(() => rm(workspace, {recursive: true, force: true}));
 const project = join(workspace, 'My_Plugin');
 run(process.execPath, [source, 'init', project]);
 const manifest = JSON.parse(await readFile(join(project, 'manifest.json'), 'utf8'));
 assert.equal(manifest.id, 'example.my-plugin', 'the ID is derived from the directory');
 assert.deepEqual(manifest.permissions, ['workspace.read']);
 assert.deepEqual(manifest.contributes.panels.map(p => p.icon), ['file-text']);
 assert.match(await readFile(join(project, 'LICENSE'), 'utf8'), /Permission is hereby granted, free of charge/);
 assert.match(await readFile(join(project, '.gitignore'), 'utf8'), /node_modules\/\n.*\*\.cmxaddon/s);
 run(process.execPath, [source, 'init', join(workspace, 'explicit'), '--id', 'acme.notes']);
 assert.equal(JSON.parse(await readFile(join(workspace, 'explicit/manifest.json'), 'utf8')).id, 'acme.notes');
 run(process.execPath, [source, 'init', join(workspace, 'reserved'), '--id', 'con.notes'], undefined, 1);
 run('npm', ['install', '--no-save', '--package-lock=false', '--ignore-scripts', '--no-audit', '--no-fund', ...tarballs], project);
 // npm run check includes strict TypeScript over the starter source.
 for (const script of ['build', 'check', 'pack']) run('npm', ['run', script], project);
 const packed = join(project, 'example.my-plugin-1.0.0.cmxaddon');
 assert.deepEqual(entries(await readFile(packed)), ['manifest.json', 'plugin.js', 'README.md', 'LICENSE']);
 const cli = join(project, 'node_modules/@codemux/plugin-cli/src/cli.mjs');
 // Optional developer source map, removed again by a plain build.
 run(process.execPath, [cli, 'build', '--sourcemap'], project);
 run(process.execPath, [cli, 'pack', '--out', join(project, 'dist/mapped.cmxaddon')], project);
 assert.ok(entries(await readFile(join(project, 'dist/mapped.cmxaddon'))).includes('source.map'));
 run(process.execPath, [cli, 'build'], project);
 assert.equal(await digest(join(project, 'source.map')), null);
 // Invalid manifests fail `check` with the rejected value named.
 const valid = await readFile(join(project, 'manifest.json'), 'utf8');
 for (const [change, message] of [
  [m => {m.contributes.panels[0].icon = 'star';}, /Unknown icon "star"/],
  [m => {m.version = 'v1.0.0';}, /Invalid package version "v1\.0\.0"/],
  [m => {m.api = '>=1.0.0 <2.0.0';}, /Invalid API range/],
  [m => {m.contributes.commands[1].id = 'open';}, /Duplicate or more than 20 commands/],
 ]) {
  const m = JSON.parse(valid);
  change(m);
  await writeFile(join(project, 'manifest.json'), JSON.stringify(m));
  assert.match(run(process.execPath, [cli, 'check'], project, 1).stderr, message);
 }
 await writeFile(join(project, 'manifest.json'), valid);
 // `dev` keeps watching after a failed first build and repacks one stable path.
 const entry = join(project, 'src/index.tsx');
 const original = await readFile(entry, 'utf8');
 await writeFile(entry, 'export default {');
 const output = join(project, 'dist/example.my-plugin.cmxaddon');
 const dev = spawn(process.execPath, [cli, 'dev'], {cwd: project, stdio: ['ignore', 'pipe', 'pipe']});
 let errors = '';
 dev.stderr.on('data', data => (errors += data));
 const exited = new Promise(r => dev.once('exit', r));
 try {
  await until(() => errors.length > 0, 'the first build error');
  assert.equal(dev.exitCode, null, 'dev must keep watching after a failed build');
  assert.equal(await digest(output), null);
  await writeFile(entry, original);
  const first = await until(() => digest(output), 'the first development package');
  await writeFile(entry, original.replace('Hello from your plugin', 'Hello again'));
  await until(async () => {const next = await digest(output); return next && next !== first;}, 'a rebuilt development package');
 } finally {
  dev.kill();
  await exited;
 }
});
