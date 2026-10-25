import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { conversationCardSummary } from '../../../../packages/plugin-sdk/dist/index.js';
import { durableHost, worker } from '../../../../packages/plugin-sdk/test/worker-harness.mjs';
const directory='examples/plugins/platform-frozen-v1';
const SHA256='5aec390d81a7e6cd1e99232276d02fd0a8ea9c14c611578aa56904b2bf4e5b70';
test('unchanged frozen v1 bytes run without rebuilding or importing the current SDK',async t=>{
  const bytes=await readFile(new URL('../plugin.mjs',import.meta.url));assert.equal(createHash('sha256').update(bytes).digest('hex'),SHA256);
  assert.equal((await readFile(new URL('../plugin.sha256',import.meta.url),'utf8')).split(' ')[0],SHA256);
  assert.ok(!bytes.toString().includes('import '));
  const host=await durableHost(t);let f=await worker(t,directory,host);
  let result=await f.invoke('action:snapshot',null,{caller:{kind:'agent',threadId:'verified-source'},context:{threadId:'requested-target'},operationId:'frozen-operation'});
  assert.equal(result.data.apiVersion,1);assert.equal(result.data.activationDenied,true);assert.equal(result.data.caller.threadId,'verified-source');
  assert.equal(result.data.context.threadId,'requested-target');assert.equal(result.data.operationId,'frozen-operation');
  assert.ok(f.manifest.optionalCapabilities.includes('future.example-capability'));assert.ok(!result.data.capabilities.includes('future.example-capability'));
  assert.equal(result.card.installationId,undefined);assert.equal(result.card.dataOwner,undefined);assert.equal(result.unsupportedIntentCount,1);
  const historical=structuredClone(result.card);await f.stop();
  // Rendering the historical summary has no dependency on a running/installed worker.
  assert.equal(conversationCardSummary(historical),'Frozen v1 snapshot remains available as historical text.');
  f=await worker(t,directory,host);result=await f.invoke('action:snapshot');assert.equal(result.data.count,2);
});
test('cancellation aborts the frozen handler and a subsequent invocation still works',async t=>{
  const host=await durableHost(t);let entered;const entry=new Promise(resolve=>entered=resolve);
  host.handlers.set('storage.set',async input=>{const state=await host.read();state.storage[input.key]=input.value;await host.save(state);if(input.key==='entered')entered();return null;});
  const f=await worker(t,directory,host);const task=f.invoke('action:wait',null,{invocationId:'cancel-me'});
  const rejected=assert.rejects(task,/cancelled/);await entry;f.send({type:'cancel',invocationId:'cancel-me'});await rejected;
  assert.equal((await f.invoke('action:snapshot')).data.count,1);
});
