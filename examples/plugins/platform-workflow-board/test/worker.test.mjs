import assert from 'node:assert/strict';
import { test } from 'node:test';
import { durableHost, worker } from '../../../../packages/plugin-sdk/test/worker-harness.mjs';
const directory='examples/plugins/platform-workflow-board';
const event={deliveryId:'delivery-1',sequence:1,threadId:'source',projectId:'project',runId:'run-1',outcome:'completed',summary:'Committed summary',effectsAllowed:true};
const background={caller:{kind:'background'},operationId:'event-delivery-1',principal:'background'};
test('observations persist across process restart and replay even after removing the visible note',async t=>{
  const host=await durableHost(t);let f=await worker(t,directory,host);
  await f.invoke('event:run.finalized',event,background);await f.stop();
  f=await worker(t,directory,host);let result=await f.invoke('event:run.finalized',event,background);
  assert.equal(result.data.items.length,1);assert.equal(result.data.observations,1);
  await f.invoke('view:board:remove',{id:'run-delivery-1'});
  result=await f.invoke('event:run.finalized',event,background);assert.equal(result.data.items.length,0);
  assert.equal((await host.read()).dispatches.length,0);
});
test('follow-ups require opt-in, capability, verified provenance and completed outcome',async t=>{
  const host=await durableHost(t,{settings:{autoFollowup:true}});
  let f=await worker(t,directory,host,{capabilities:['ui','storage','settings','threads.read','events.run-finalized']});
  await f.invoke('event:run.finalized',event,background);assert.equal((await host.read()).dispatches.length,0);await f.stop();
  f=await worker(t,directory,host);
  for(const [id,patch] of [['unverified',{effectsAllowed:false}],['failed',{outcome:'failed'}]])
    await f.invoke('event:run.finalized',{...event,...patch,deliveryId:id},{...background,operationId:`event-${id}`});
  assert.equal((await host.read()).dispatches.length,0);
  await f.invoke('event:run.finalized',{...event,deliveryId:'verified'},{...background,operationId:'event-verified'});
  const state=await host.read();assert.equal(state.dispatches.length,1);assert.equal(state.dispatches[0].input.idempotencyKey,'completion-review');
  assert.equal(state.storage['board.v1'].deliveries.at(-1).followup.receipt.state,'succeeded');
});
test('lost send response recovers through durable event identity without a second dispatch',async t=>{
  const host=await durableHost(t,{settings:{autoFollowup:true}});host.loseNextSend=true;
  let f=await worker(t,directory,host);
  await assert.rejects(f.invoke('event:run.finalized',event,background),/Reply lost/);
  assert.equal((await host.read()).storage['board.v1'].deliveries[0].followup.state,'pending');await f.stop();
  f=await worker(t,directory,host);await f.invoke('event:run.finalized',event,background);
  const state=await host.read();assert.equal(state.dispatches.length,1);assert.equal(state.storage['board.v1'].deliveries[0].followup.state,'recorded');
});
test('saved selections use stable operation identity and reject agent editing',async t=>{
  const host=await durableHost(t),f=await worker(t,directory,host);
  const options={operationId:'save-note',context:{threadId:'source',selection:'A selected note'}};
  await f.invoke('action:save-selection',null,options);await f.invoke('action:save-selection',null,options);
  assert.equal((await f.invoke('view:board:list')).data.items.length,1);
  await assert.rejects(f.invoke('view:board:add',{text:'forged'},{caller:{kind:'agent',threadId:'source'}}),/Only a person/);
});
test('turning opt-in off prevents a retained pending follow-up from dispatching on replay',async t=>{
  const host=await durableHost(t,{settings:{autoFollowup:true}}),f=await worker(t,directory,host);
  // Reproduce a crash after saving intent but before dispatching any native operation.
  const state=await host.read();state.settings.autoFollowup=false;
  state.storage['board.v1']={version:1,items:[],deliveries:[{id:event.deliveryId,threadId:'source',runId:event.runId,outcome:'completed',followup:{state:'pending',request:{threadId:'source',idempotencyKey:'completion-review',text:'Frozen review request'}}}]};
  await host.save(state);await f.invoke('event:run.finalized',event,background);
  assert.equal((await host.read()).dispatches.length,0);assert.equal((await host.read()).storage['board.v1'].deliveries[0].followup.state,'pending');
});
test('large saved notes page below worker result bounds without losing retained content',async t=>{
  const host=await durableHost(t),f=await worker(t,directory,host);
  const state=await host.read();state.storage['board.v1']={version:1,deliveries:[],items:Array.from({length:100},(_,id)=>({id:`note-${id}`,text:'🚀'.repeat(2000)}))};await host.save(state);
  const ids=new Set();let cursor=0,pages=0;
  do{const result=await f.invoke('view:board:list',{cursor,limit:100});assert.ok(Buffer.byteLength(JSON.stringify(result))<128000);
    for(const item of result.data.items)ids.add(item.id);cursor=result.data.nextCursor;pages++;
  }while(cursor!==null);
  assert.equal(ids.size,100);assert.ok(pages>1);assert.equal((await host.read()).storage['board.v1'].items.length,100);
});
