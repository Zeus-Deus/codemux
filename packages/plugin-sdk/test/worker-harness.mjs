// Real worker transport with deterministic domain fixtures. This is not a replacement
// for native domain/transport integration tests: no native agents are launched here.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, writeFile, rename, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
export const ROOT = fileURLToPath(new URL('../../../', import.meta.url));
export const SUPPORTED = ['storage','settings','threads.read','threads.send','threads.create','events.run-finalized','ui','ui.sidebar-views','ui.native-trees','ui.conversation-cards','ui.changes','worktrees.create','workflow.query'];
export const sourceThread = { id:'source', projectId:'project', workspaceId:'workspace', title:'Source',
  modelSelection:{instanceId:'codex',model:'gpt-6.1-sol',options:[{id:'effort',value:'xhigh'},{id:'service_tier',value:'priority'}]},
  runtimeMode:'auto-accept-edits', nativePermissionMode:'workspace-write', interactionMode:'default',
  createdAt:'2026-01-01T00:00:00Z',updatedAt:'2026-10-01T00:00:00Z',archivedAt:null,
  settledAt:null,settledOverride:null,snoozedUntil:null,snoozedAt:null,pinnedAt:null,branch:'main',worktreePath:null,
  lineage:{parentThreadId:null,relationshipToParent:null,rootThreadId:'source'},isRunning:false,needsAttention:false,importedSnapshot:false };
export const nativeModel = { modelSelection:sourceThread.modelSelection,title:'GPT 6.1 Sol',providerTitle:'codex',options:[{id:'effort',label:'Reasoning effort',type:'select',options:[{id:'xhigh',label:'Extra high'}]},{id:'service_tier',label:'Service tier',type:'select',options:[{id:'priority',label:'Priority'}]}],
  nativeCapabilities:{id:'gpt-6.1-sol',effort_levels:['xhigh'],service_tiers:[{value:'priority',label:'Priority'}]},permissionModes:['read-only','workspace-write','danger-full-access'] };
const clone=value=>structuredClone(value);
export async function durableHost(t, options={}) {
  const directory=await mkdtemp(path.join(tmpdir(),'codemux-author-host-'));
  t.after(()=>rm(directory,{recursive:true,force:true}));
  const file=path.join(directory,'state.json');
  await writeFile(file,JSON.stringify({storage:{},receipts:{},dispatches:[],settings:{autoFollowup:false,...options.settings}}));
  let counter=0;
  const host={ directory, calls:[], handlers:new Map(), threads:new Map([['source',sourceThread]]), loseNextCreate:false, loseNextSend:false,
    createState:'succeeded', sendState:'succeeded',
    read:async()=>JSON.parse(await readFile(file,'utf8')),
    save:async state=>{const temp=`${file}.${++counter}.tmp`;await writeFile(temp,JSON.stringify(state));await rename(temp,file);},
    async dispatch(method,input,scope) {
      host.calls.push({method,input:clone(input),scope:clone(scope)});
      if(host.handlers.has(method))return host.handlers.get(method)(input,scope);
      const state=await host.read();
      if(method==='storage.get')return clone(state.storage[input.key]??null);
      if(method==='storage.set'){state.storage[input.key]=clone(input.value);await host.save(state);return null;}
      if(method==='storage.delete'){delete state.storage[input.key];await host.save(state);return null;}
      if(method==='settings.get')return state.settings[input.key]??null;
      if(method==='secrets.get')return null;
      if(method==='ui.invalidate')return null;
      if(method==='threads.metadata') { const result=host.threads.get(input.threadId); if(!result)throw new Error('not_found: Thread unavailable');return clone(result); }
      if(method==='projects.list')return {projects:[{id:'project',title:'Project',workspaceRoot:'/fixture/project'}],nextCursor:null};
      if(method==='workspaces.list')return {workspaces:[{id:'workspace',projectId:'project',title:'Project',cwd:'/fixture/project',projectRoot:'/fixture/project',branch:'main',worktreePath:null,archivedAt:null}],nextCursor:null};
      if(method==='threads.list')return {threads:[...host.threads.values()].filter(thread=>thread.projectId===input.projectId),nextCursor:null};
      if(method==='models.list')return {models:[clone(nativeModel)],nextCursor:null};
      if(method==='workflow.query'){
        if(input.query==='effect')return state.receipts[JSON.stringify([scope.principal,scope.operationId,input.idempotencyKey])]?.receipt??null;
        if(input.query==='operation'){
          const entry=Object.values(state.receipts).find(item=>item.receipt.operation_id===input.operationId && item.principal===scope.principal);
          if(!entry)throw Object.assign(new Error('Receipt is not available to this caller'),{reason:'not_found'});
          return clone(entry.receipt);
        }
        return host.dispatch(`${input.query}.list`,input,scope);
      }
      if(method==='threads.create'||method==='threads.send'){
        const key=JSON.stringify([scope.principal,scope.operationId,input.idempotencyKey]);
        const previous=state.receipts[key];
        if(previous){assert.deepEqual(previous.request,{method,input});return clone(previous.receipt);}
        const id=`native-${state.dispatches.length+1}`,thread=method==='threads.send'?input.threadId:`thread-${state.dispatches.length+1}`;
        const receipt={operation_id:id,tool:method==='threads.create'?'thread_launch':'thread_send',state:method==='threads.create'?host.createState:host.sendState,
          thread_id:thread,workspace_id:'workspace',result:{accepted:true},error:null,delivery_id:null,dispatch_disposition:'queued',created_at:'2026-01-01',updated_at:'2026-01-01'};
        state.receipts[key]={request:{method,input:clone(input)},principal:scope.principal,receipt};
        state.dispatches.push({method,input:clone(input),receipt});await host.save(state);
        host.threads.set(thread,{...clone(sourceThread),id:thread,title:input.title??'Follow-up'});
        if((method==='threads.create'&&host.loseNextCreate)||(method==='threads.send'&&host.loseNextSend)){
          host.loseNextCreate=false;host.loseNextSend=false;
          throw Object.assign(new Error('Reply lost after native admission'),{reason:'outcome_unknown'});
        }
        return clone(receipt);
      }
      throw new Error(`Unsupported fixture method: ${method}`);
    }
  };return host;
}
export async function worker(t, relativeDirectory, host, options={}) {
  const directory=path.resolve(ROOT,relativeDirectory),manifest=JSON.parse(await readFile(path.join(directory,'codemux-plugin.json'),'utf8'));
  const binary=process.env.CODEMUX_PLUGIN_HOST_TEST_BINARY;
  const child=spawn(binary?path.resolve(binary):process.execPath,binary?[]:[path.join(ROOT,'sidecar/plugin-host/main.mjs')],{env:{PATH:process.env.PATH},stdio:['pipe','pipe','pipe']});
  let input='',stderr='',serial=0,exited=false;
  const pending=new Map(),scopes=new Map(),messages=[],waiters=[];
  const send=message=>child.stdin.write(JSON.stringify(message)+'\n');
  const receive=message=>{const waiter=waiters.shift();if(waiter)waiter.resolve(message);else messages.push(message);};
  const next=()=>messages.length?Promise.resolve(messages.shift()):new Promise((resolve,reject)=>waiters.push({resolve,reject}));
  child.stderr.on('data',chunk=>stderr+=chunk);
  child.stdin.on('error',error=>{for(const task of pending.values())task.reject(error);pending.clear();});
  child.once('exit',code=>{exited=true;const error=new Error(`Worker exited ${code}: ${stderr}`);for(const task of pending.values())task.reject(error);for(const task of waiters.splice(0))task.reject(error);});
  child.stdout.on('data',chunk=>{input+=chunk;let newline;while((newline=input.indexOf('\n'))>=0){const message=JSON.parse(input.slice(0,newline));input=input.slice(newline+1);
    if(message.type==='host-call'){
      const scope=scopes.get(message.invocationId);
      Promise.resolve().then(()=>host.dispatch(message.method,message.input,scope)).then(value=>{
        if(!exited)send({type:'host-result',callId:message.callId,invocationId:message.invocationId,ok:true,value});
      },error=>{if(!exited)send({type:'host-result',callId:message.callId,invocationId:message.invocationId,ok:false,error:{message:error.message,reason:error.reason,code:error.code,priorEffect:error.priorEffect}});});
    }else if(['result','failure','cancelled'].includes(message.type)&&message.invocationId){
      const task=pending.get(message.invocationId);if(!task)continue;pending.delete(message.invocationId);
      if(message.type==='result')task.resolve(message.result);else task.reject(Object.assign(new Error(message.message??message.type),{reason:message.type}));
    }else if(message.type!=='log')receive(message);
  }});
  const stop=async()=>{if(exited)return;const exit=once(child,'exit');child.kill('SIGKILL');await exit;};
  t.after(stop);
  assert.equal((await next()).type,'booted');
  const requested=[...new Set([...(manifest.capabilities??[]),...(manifest.requiredCapabilities??[]),...(manifest.optionalCapabilities??[])])];
  send({type:'init',protocol:1,directory,entry:manifest.entry,dataDirectory:host.directory,capabilities:options.capabilities??requested.filter(cap=>SUPPORTED.includes(cap))});
  assert.equal((await next()).type,'ready');
  return {child,stop,send,manifest,
    invoke(handler,input=null,{operationId=`op-${++serial}`,principal='desktop',caller={kind:'client'},context={threadId:'source',projectId:'project'},invocationId=`inv-${++serial}`}={}){
      scopes.set(invocationId,{principal,operationId});
      const promise=new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(new Error(`Invocation timed out: ${handler}`)),5000);timer.unref();pending.set(invocationId,{resolve:value=>{clearTimeout(timer);resolve(value);},reject:error=>{clearTimeout(timer);reject(error);}});});
      send({type:'invoke',invocationId,operationId,handler,input,caller,context});return promise;
    }
  };
}
