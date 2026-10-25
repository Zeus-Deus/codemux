import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { access, cp, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { apiVersion, hasCapability, operationDisposition, conversationCardSummary } from '../dist/index.js';
const root=fileURLToPath(new URL('../../../',import.meta.url));
const tsc=path.join(root,'node_modules/typescript/bin/tsc');
async function npmCli() {
  const candidates = [
    process.env.npm_execpath,
    path.join(path.dirname(process.execPath), 'node_modules/npm/bin/npm-cli.js'),
    path.resolve(path.dirname(process.execPath), '../lib/node_modules/npm/bin/npm-cli.js'),
  ];
  for (const candidate of candidates) {
    if (!candidate) continue;
    try { await access(candidate); return candidate; } catch {}
  }
  throw new Error('Cannot locate the Node installation\'s npm CLI');
}
test('capability discovery uses the granted host snapshot and future receipt states stay uncertain',()=>{
  assert.equal(apiVersion,1);assert.equal(hasCapability({capabilities:['storage']},'future.optional'),false);
  assert.equal(hasCapability({capabilities:['storage']},'threads.create'),false);
  for(const [state,want] of [['accepted','pending'],['running','pending'],['succeeded','accepted'],['failed','failed'],['needs_attention','uncertain'],['future','uncertain']])
    assert.equal(operationDisposition({state}),want);
  assert.equal(conversationCardSummary({summary:'Historical proposal'}),'Historical proposal');
});
test('author contracts typecheck including rejection of spoofing and synthetic receipt fields',()=>{
  execFileSync(process.execPath,[tsc,'-p','packages/plugin-sdk/tsconfig.test.json'],{cwd:root,stdio:'pipe'});
});
test('published package resolves TypeScript and runtime exports in a standalone external directory',async t=>{
  const directory=await mkdtemp(path.join(tmpdir(),'codemux-external-author-'));t.after(()=>rm(directory,{recursive:true,force:true}));
  const manifest=JSON.parse(await readFile(new URL('../package.json',import.meta.url),'utf8'));
  assert.equal(manifest.name,'@codemux/plugin-sdk');assert.equal(Object.keys(manifest.dependencies??{}).length,0);
  assert.ok(!Object.keys(manifest.scripts).some(name=>/^(preinstall|postinstall|install|prepare|prepublish|prepack)/.test(name)));
  const target=path.join(directory,'node_modules/@codemux/plugin-sdk');await cp(new URL('../dist/',import.meta.url),path.join(target,'dist'),{recursive:true});
  await writeFile(path.join(target,'package.json'),JSON.stringify(manifest));await writeFile(path.join(directory,'package.json'),' {"type":"module"} ');
  await writeFile(path.join(directory,'author.ts'),`import {apiVersion,type PluginModule,type PluginOperationReceipt} from '@codemux/plugin-sdk';
export const version:1=apiVersion;
export const plugin:PluginModule={activate(ctx){ctx.handle('action:test',async()=>{const result:PluginOperationReceipt=await ctx.threads.create({projectId:'project',text:'Review',idempotencyKey:'once'});return {data:result};});}};`);
  await writeFile(path.join(directory,'tsconfig.json'),JSON.stringify({compilerOptions:{module:'NodeNext',moduleResolution:'NodeNext',target:'ES2022',strict:true,noEmit:true,types:[]},files:['author.ts']}));
  execFileSync(process.execPath,[tsc,'-p',path.join(directory,'tsconfig.json')],{cwd:directory,stdio:'pipe'});
  const output=execFileSync(process.execPath,['--input-type=module','-e',"import {apiVersion} from '@codemux/plugin-sdk';process.stdout.write(String(apiVersion));"],{cwd:directory,encoding:'utf8'});
  assert.equal(output,'1');
});
test('package archive contains the emitted self-contained API and no workspace runtime imports',async()=>{
  // Invoke the CLI through Node: Windows npm.cmd is not an executable file.
  const [packed]=JSON.parse(execFileSync(process.execPath,[await npmCli(),'pack','--dry-run','--ignore-scripts','--json'],{cwd:path.join(root,'packages/plugin-sdk'),encoding:'utf8',stdio:['ignore','pipe','pipe']}));
  const files=packed.files.map(file=>file.path);assert.ok(files.includes('dist/index.js'));assert.ok(files.includes('dist/index.d.ts'));assert.ok(files.includes('README.md'));
  assert.ok(files.every(file=>file==='package.json'||file==='README.md'||file==='LICENSE.md'||file==='THIRD_PARTY_NOTICES.md'||file.startsWith('dist/')));
});
