import assert from 'node:assert/strict';
import { test } from 'node:test';
import { durableHost, worker, sourceThread, nativeModel } from '../../../../packages/plugin-sdk/test/worker-harness.mjs';
const directory='examples/plugins/platform-task-launcher';
const agent={caller:{kind:'agent',threadId:'source'},principal:'agent:source',operationId:'proposal'};
async function propose(f){const result=await f.invoke('action:propose',{title:'Review',tasks:[{title:'Inspect',prompt:'Inspect the change without editing.'}]},agent);
  assert.ok(result.card.summary.includes('1 tasks'));assert.equal(result.card.installationId,undefined);
  const plan=(await f.invoke('view:launcher:read',{planId:result.card.data.planId})).data.plan;
  return {result,plan,task:plan.tasks[0]};}
test('proposal identity is verified independently of spoofed input/target; only people launch',async t=>{
  const host=await durableHost(t),f=await worker(t,directory,host);
  await assert.rejects(f.invoke('action:propose',{title:'Fake',tasks:[{title:'T',prompt:'P'}]},{caller:{kind:'agent'},context:{threadId:'source',projectId:'project'}}),/host-verified/);
  await assert.rejects(f.invoke('action:propose',{tasks:[{title:'T',prompt:'P'}]},{...agent,context:{threadId:'other'}}),/target context/);
  await assert.rejects(f.invoke('action:propose',{tasks:[{title:'T',prompt:'P'}],caller:{kind:'agent',threadId:'source'}},agent),/Unsupported input field/);
  const {plan,task}=await propose(f);
  await assert.rejects(f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id},agent),/Only a person/);
  await assert.rejects(f.invoke('view:launcher:read',{planId:plan.id},{context:{threadId:'other'}}),/not_found/);
  assert.equal((await host.read()).dispatches.length,0);
});
test('native model options, permission mode and worktree choices remain frozen; repeat start never dispatches twice',async t=>{
  const host=await durableHost(t),f=await worker(t,directory,host),{plan,task}=await propose(f);
  const models=await f.invoke('view:launcher:models',{});assert.deepEqual(models.data.models,[nativeModel]);
  const input={planId:plan.id,taskId:task.id,modelSelection:sourceThread.modelSelection,nativePermissionMode:'workspace-write',interactionMode:'default',
    workspaceStrategy:{type:'worktree',baseRef:'feature/local',branch:'task/inspect',startFromOrigin:false}};
  let result=await f.invoke('view:launcher:start',input,{operationId:'start-stable'});
  assert.equal(result.data.plan.tasks[0].launch.state,'accepted');assert.equal(result.data.plan.tasks[0].launch.receipt.state,'succeeded');
  assert.equal(result.data.plan.tasks[0].launch.receipt.result.accepted,true);
  await f.invoke('view:launcher:start',{...input,nativePermissionMode:'danger-full-access'},{operationId:'another-client-operation'});
  const state=await host.read();assert.equal(state.dispatches.length,1);assert.deepEqual(state.dispatches[0].input.workspaceStrategy,input.workspaceStrategy);
  assert.deepEqual(state.dispatches[0].input.modelSelection.options,[{id:'effort',value:'xhigh'},{id:'service_tier',value:'priority'}]);
  assert.equal(state.dispatches[0].input.idempotencyKey,`launch-${task.id}`);
});
test('accepted native operation remains pending and can be inspected without a second launch',async t=>{
  const host=await durableHost(t);host.createState='accepted';const f=await worker(t,directory,host),{plan,task}=await propose(f);
  let result=await f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id});
  assert.equal(result.data.plan.tasks[0].launch.state,'pending');assert.equal(result.data.plan.tasks[0].launch.receipt.state,'accepted');
  const state=await host.read();Object.values(state.receipts)[0].receipt.state='succeeded';await host.save(state);
  result=await f.invoke('view:launcher:inspect',{planId:plan.id,taskId:task.id});assert.equal(result.data.plan.tasks[0].launch.state,'accepted');
  assert.equal((await host.read()).dispatches.length,1);
});
test('response lost after native launch survives restart as uncertain and cannot duplicate launch',async t=>{
  const host=await durableHost(t);host.loseNextCreate=true;let f=await worker(t,directory,host),{plan,task,result:proposal}=await propose(f);
  let result=await f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id},{operationId:'lost-response'});
  assert.equal(result.data.plan.tasks[0].launch.state,'uncertain');await f.stop();
  f=await worker(t,directory,host);result=await f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id},{operationId:'new-operation'});
  assert.equal(result.data.plan.tasks[0].launch.state,'uncertain');
  await f.invoke('view:launcher:inspect',{planId:plan.id,taskId:task.id});
  await assert.rejects(f.invoke('view:launcher:acknowledge',{planId:plan.id,taskId:task.id}),/Explicitly acknowledge/);
  await f.invoke('view:launcher:acknowledge',{planId:plan.id,taskId:task.id,confirmUncertain:true});
  result=await f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id});assert.equal(result.data.plan.tasks[0].launch.state,'abandoned');
  assert.equal((await host.read()).dispatches.length,1);assert.equal(proposal.card.summary,'Proposed 1 tasks. Review before starting.');
});
test('revoked scope and missing optional worktree grant fail before dispatch',async t=>{
  const host=await durableHost(t),f=await worker(t,directory,host,{capabilities:['storage','threads.read','workflow.query','threads.create','ui','ui.conversation-cards','ui.changes']}),{plan,task}=await propose(f);
  await assert.rejects(f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id,workspaceStrategy:{type:'worktree',baseRef:'main'}}),/not granted/);
  host.threads.delete('source');
  await assert.rejects(f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id}),/not_found/);
  assert.equal((await host.read()).dispatches.length,0);
});
test('missing source model and damaged storage are rejected without overwriting retained plans',async t=>{
  const host=await durableHost(t),f=await worker(t,directory,host);
  host.threads.set('source',{...sourceThread,modelSelection:{instanceId:null,model:null,options:[]}});
  await assert.rejects(f.invoke('action:propose',{tasks:[{title:'T',prompt:'P'}]},agent),/provider instance/);
  assert.equal((await host.read()).storage['plans.v1'],undefined);
  const saved=await host.read();saved.storage['plans.v1']={version:99,plans:[]};await host.save(saved);
  host.threads.set('source',sourceThread);await assert.rejects(f.invoke('view:launcher:read',{}),/Unsupported or damaged/);
  assert.deepEqual((await host.read()).storage['plans.v1'],{version:99,plans:[]});
});
test('unknown or needs-attention native receipt state never turns into a successful launch or redispatch',async t=>{
  const host=await durableHost(t);host.createState='needs_attention';const f=await worker(t,directory,host),{plan,task}=await propose(f);
  let result=await f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id});assert.equal(result.data.plan.tasks[0].launch.state,'uncertain');
  await f.invoke('view:launcher:start',{planId:plan.id,taskId:task.id});
  const state=await host.read();Object.values(state.receipts)[0].receipt.state='future-native-state';await host.save(state);
  result=await f.invoke('view:launcher:inspect',{planId:plan.id,taskId:task.id});assert.equal(result.data.plan.tasks[0].launch.state,'uncertain');assert.equal((await host.read()).dispatches.length,1);
});
