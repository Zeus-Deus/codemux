import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import { JSDOM, VirtualConsole } from 'jsdom';
import { nativeModel, sourceThread } from '../../../../packages/plugin-sdk/test/worker-harness.mjs';
const html=await readFile(new URL('../view.html',import.meta.url),'utf8');
const context=id=>({thread:{threadId:id,projectId:'project'},card:{summary:`Historical ${id} proposal`,data:{planId:id}}});
const plan=(id='source',state='proposed')=>({id,title:`Plan ${id}`,defaults:{modelSelection:sourceThread.modelSelection,nativePermissionMode:'workspace-write'},tasks:[{id:'task',title:'Review tests',prompt:'Inspect tests',launch:{state}}]});
async function until(predicate){for(let step=0;step<200;step++){if(predicate())return;await delay(5);}assert.fail('View condition timed out');}
function view(t,invoke,initial=context('source')){
  const calls=[],listeners=[],errors=[];
  const console=new VirtualConsole();console.on('jsdomError',error=>errors.push(error));
  const dom=new JSDOM(html,{runScripts:'dangerously',virtualConsole:console,beforeParse(window){window.structuredClone=structuredClone;window.codemux={ready:Promise.resolve(initial),onContext:listener=>listeners.push(listener),
    invoke:async(name,input)=>{calls.push({name,input:structuredClone(input)});return invoke(name,input);},intent:async()=>{}};}});
  t.after(()=>{dom.window.close();assert.deepEqual(errors,[]);});
  return {document:dom.window.document,calls,context:value=>listeners.forEach(listener=>listener(value))};
}
test('author card view keeps recorded summary when current plan access is unavailable',async t=>{
  const f=view(t,async()=>{throw new Error('Plugin state unavailable under current grants');});
  await until(()=>f.document.getElementById('error').textContent.includes('unavailable'));
  assert.equal(f.document.getElementById('summary').textContent,'Historical source proposal');
  assert.equal(f.document.querySelectorAll('article').length,0);
});
test('lost start reply reads the durable journal and never retries; model/mode/worktree choices reach the handler',async t=>{
  let state='proposed';const f=view(t,async(name)=>{
    if(name==='read')return {data:{plan:plan('source',state),canStart:true,canWorktree:true}};
    if(name==='models')return {data:{models:[nativeModel],nextCursor:null}};
    if(name==='start'){state='uncertain';throw new Error('Reply lost after admission');}
    throw new Error('Unexpected view call');
  });
  await until(()=>f.document.querySelector('article'));
  f.document.getElementById('catalog').click();await until(()=>!f.document.getElementById('catalog').disabled);
  const workspace=f.document.getElementById('workspace');workspace.value='worktree';workspace.dispatchEvent(new f.document.defaultView.Event('change'));
  f.document.getElementById('base').value='feature/local';f.document.getElementById('branch').value='task/review';
  const start=[...f.document.querySelectorAll('article button')].find(button=>button.textContent==='Start');assert.ok(!start.disabled);start.click();
  await until(()=>f.document.querySelector('article').textContent.includes('uncertain'));
  f.document.getElementById('refresh').click();await delay(10);
  assert.equal(f.calls.filter(call=>call.name==='start').length,1);
  const input=f.calls.find(call=>call.name==='start').input;
  assert.equal(input.nativePermissionMode,'workspace-write');assert.deepEqual(input.modelSelection.options,sourceThread.modelSelection.options);
  assert.deepEqual(input.workspaceStrategy,{type:'worktree',baseRef:'feature/local',branch:'task/review',startFromOrigin:false});
  assert.equal([...f.document.querySelectorAll('article button')].filter(button=>button.textContent==='Start').length,0);
});
test('late response from a previous thread cannot overwrite the current card',async t=>{
  const reads=new Map();const f=view(t,(_name,input)=>new Promise(resolve=>reads.set(input.planId,resolve)));
  await until(()=>reads.has('source'));f.context(context('other'));await until(()=>reads.has('other'));
  reads.get('other')({data:{plan:plan('other'),canStart:true,canWorktree:false}});await until(()=>f.document.getElementById('title').textContent==='Plan other');
  reads.get('source')({data:{plan:plan('source'),canStart:true,canWorktree:true}});await delay(10);
  assert.equal(f.document.getElementById('title').textContent,'Plan other');assert.equal(f.document.getElementById('summary').textContent,'Historical other proposal');
});
test('model pagination preserves reviewed options and native mode',async t=>{
  const selected={...nativeModel,options:[{id:'effort',label:'Effort',type:'select',options:[{id:'xhigh',label:'Extra high'},{id:'high',label:'High'}]}]};
  const f=view(t,async(name,input)=>name==='models'?{data:{models:input.cursor===0?[selected]:[{...nativeModel,modelSelection:{instanceId:'codex',model:'other'}}],nextCursor:input.cursor===0?1:null}}:
    {data:{plan:plan(),canStart:true,canWorktree:true}});
  await until(()=>f.document.querySelector('article'));f.document.getElementById('catalog').click();await until(()=>!f.document.getElementById('catalog').disabled);
  const effort=f.document.querySelector('#options select');effort.value='high';effort.dispatchEvent(new f.document.defaultView.Event('change'));f.document.getElementById('mode').value='danger-full-access';
  f.document.getElementById('more').click();await until(()=>!f.document.getElementById('catalog').disabled);
  assert.equal(f.document.querySelector('#options select').value,'high');assert.equal(f.document.getElementById('mode').value,'danger-full-access');
});
