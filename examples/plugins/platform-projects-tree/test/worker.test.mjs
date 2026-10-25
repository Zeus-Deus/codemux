import assert from 'node:assert/strict';
import { test } from 'node:test';
import { durableHost, worker, sourceThread } from '../../../../packages/plugin-sdk/test/worker-harness.mjs';
const directory='examples/plugins/platform-projects-tree';
test('real tree worker pages safe metadata and emits native rows without transcript reads',async t=>{
  const host=await durableHost(t);
  host.handlers.set('projects.list',input=>input.cursor===0?{projects:[{id:'project',title:'Project'}],nextCursor:1}:{projects:[{id:'empty',title:'Empty project'}],nextCursor:null});
  host.handlers.set('threads.list',input=>input.projectId==='empty'?{threads:[],nextCursor:null}:
    input.cursor===0?{threads:[sourceThread],nextCursor:1}:{threads:[{...sourceThread,id:'child',lineage:{parentThreadId:'source',relationshipToParent:'fork',rootThreadId:'source'}}],nextCursor:null});
  const f=await worker(t,directory,host), result=await f.invoke('tree:projects');
  const nodes=result.data.nodes, parent=nodes.find(node=>node.threadId==='source'),child=nodes.find(node=>node.threadId==='child');
  assert.ok(parent);assert.equal(child.parentId,parent.id);assert.ok(nodes.some(node=>node.kind==='project'&&node.projectId==='empty'));
  assert.equal(host.calls.filter(call=>call.method==='threads.read').length,0);
  assert.equal(host.calls.filter(call=>call.method==='projects.list').length,2);
});
test('broken pagination is bounded and returns the valid partial snapshot',async t=>{
  const host=await durableHost(t);
  host.handlers.set('projects.list',()=>({projects:[{id:'project',title:'Project'}],nextCursor:0}));
  host.handlers.set('threads.list',()=>({threads:[sourceThread],nextCursor:0}));
  const f=await worker(t,directory,host),result=await f.invoke('tree:projects');
  assert.equal(result.data.nodes.filter(node=>node.kind==='thread').length,1);
  assert.ok(host.calls.length<10);
});
