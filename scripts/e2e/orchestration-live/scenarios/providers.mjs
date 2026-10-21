// Opt-in provider matrix. Missing executables/accounts are explicit gaps.
import { randomUUID } from 'node:crypto';
export const name = 'providers';
export const description = 'OpenCode, Cursor, Grok and Hermes act as coordinators and as workers, returning an authenticated thread_send report.';
const CLAUDE = {provider:'claude',model:process.env.ORCH_LIVE_CLAUDE_MODEL??'claude-haiku-4-5-20251001',permissionMode:'bypassPermissions'};
const presets = {
  opencode:{provider:'opencode',model:process.env.ORCH_LIVE_OPENCODE_MODEL??'openai/gpt-6-luna'},
  cursor:{provider:'cursor',model:process.env.ORCH_LIVE_CURSOR_MODEL??'default',permissionMode:'agent'},
  grok:{provider:'grok',model:process.env.ORCH_LIVE_GROK_MODEL??null,permissionMode:'agent'},
};
const q = s=>`'${s.replaceAll("'","''")}'`;
async function send(ctx,chat,provider,text) {await ctx.driver.invoke('agent_chat_send_turn',{provider,input:{thread_id:chat.threadId,text,display_text:text,images:[],skill_ids:[],include_plugins:true,model_override:null,effort_override:null,permission_mode_override:null}});}
async function report(ctx,coordinator,worker) {
  const tag=randomUUID().slice(0,8), expected=`RETURN-${tag}-97`, ack=`ACK-${tag}-97`;
  const parent=await ctx.startChat({...coordinator,text:'For this greeting, do not use tools or delegate. Compute 37 plus 29. Reply only READY followed by the sum.'});
  await ctx.waitFor(()=>ctx.assert.reply(parent.threadId,'READY.*66').catch(()=>null),{timeout:180000,interval:1000,label:`${coordinator.provider} coordinator ready`});
  await ctx.waitFor(async()=>{const state=await ctx.codemux('thread_status',{workspace_id:ctx.env.WORKSPACE_ID,thread_id:parent.threadId});return state.settled;},{timeout:30000,label:'coordinator finished first turn'});
  const brief=`Compute 46 plus 51. Call the injected Codemux thread_send tool once with thread_id ${JSON.stringify(parent.threadId)}, client_request_id "return-${tag}", delivery auto, message "RETURN-${tag}-<sum>" replacing <sum> by your computed sum. Use the injected MCP tools; if absent use the host's authenticated mcp-call fallback. End your turn after sending. Do not poll.`;
  const target = {instance_id:worker.instance??worker.provider,model:worker.model,options:{}};
  const launch={target,title:`Matrix ${worker.provider} ${tag}`,message:brief,client_request_id:`matrix-${tag}`};
  if(worker.mode)launch.permission_mode=worker.mode;
  await send(ctx,parent,coordinator.provider,`The user requests one separate independent chat. Call Codemux thread_launch exactly once with ${JSON.stringify(launch)}. End your turn after the launch without polling. When a later worker message starts RETURN-${tag}, reply ACK-${tag}-<reported sum>. Do not thread_send an acknowledgment.`);
  await ctx.waitFor(()=>ctx.assert.reply(parent.threadId,ack).catch(()=>null),{timeout:300000,interval:1000,label:`${coordinator.provider} acknowledges ${worker.provider} report`});
  const rows=await ctx.assert.sql(`SELECT sender_thread_id,created_by,json_extract(payload,'$.text') AS text FROM agent_chat_messages WHERE thread_id=${q(parent.threadId)} AND json_extract(payload,'$.type')='user_message'`);
  const report=rows.find(r=>r.created_by==='agent' && r.text?.includes(expected));
  ctx.check(`${coordinator.provider} coordinator / ${worker.provider} worker: authenticated return`,!!report?.sender_thread_id,report);
  const [launchReceipt]=await ctx.assert.sql(`SELECT thread_id FROM agent_control_operations WHERE tool='thread_launch' AND principal=${q(`thread:${parent.threadId}`)} AND request_key=${q(`matrix-${tag}`)}`);
  const [selected]=await ctx.assert.sql(`SELECT provider,cwd FROM agent_chat_sessions WHERE thread_id=${q(report.sender_thread_id)}`);
  ctx.check('report came from this launch, with the selected provider',launchReceipt?.thread_id===report.sender_thread_id&&selected?.provider===worker.provider,{launched:launchReceipt?.thread_id,sender:report.sender_thread_id,selected});
  if(worker.provider==='hermes') {
    const binding=await ctx.driver.invoke('hermes_binding',{threadId:report.sender_thread_id});
    ctx.check('worker retains the selected Hermes profile',binding?.profile?.id===worker.instance?.slice('hermes:'.length),{profile:binding?.profile?.id});
  }
  ctx.observe(`${coordinator.provider}->${worker.provider}`,{parent:parent.threadId,child:report.sender_thread_id});
  await ctx.openTab(parent.tabId);
  await ctx.shot(`pr4-${coordinator.provider}-coordinator-${worker.provider}-worker`,{phone:async(driver)=>{
    const visible=`(() => {const host=document.querySelector('[data-transcript-cache-host*="${parent.threadId}"]');return !!host && host.offsetParent!==null;})()`;
    if(!await driver.eval(visible))await driver.clickSelector('.mobile-workspace-open').catch(()=>{});
    await driver.waitFor(visible,{timeout:20000,label:'coordinator visible on phone'});
  }});
}
export async function run(ctx) {
  const selected=(process.env.ORCH_LIVE_MATRIX_PROVIDERS??'opencode,cursor,grok,hermes').split(',');
  const matrix=[];
  const claudeCaps=await ctx.driver.invoke('list_chat_provider_capabilities',{provider:'claude',force:false,accountId:null});
  const claudeModel=claudeCaps.models.find(m=>m.id===process.env.ORCH_LIVE_CLAUDE_MODEL)??claudeCaps.models.find(m=>/haiku/i.test(m.id));
  if(!claudeModel)throw new Error('No advertised inexpensive Claude model for matrix worker');
  CLAUDE.model=claudeModel.id;
  ctx.observe('Claude exact catalog model',CLAUDE.model);
  for(const provider of selected) {
    let config=presets[provider];
    try {
      if(provider==='hermes') {
        const profiles=await ctx.driver.invoke('hermes_profiles',{});const profile=profiles.find(p=>p.id==='default');
        if(!profile)throw new Error('no configured default Hermes profile');
        const catalog=await ctx.driver.invoke('hermes_catalog',{profile});
        if(catalog.state!=='ready')throw new Error(catalog.message??'Hermes catalog is unavailable');
        const model=catalog.session.models.currentModelId;
        const mode=catalog.session.modes?.availableModes?.some(m=>m.id==='dont_ask')?'dont_ask':'default';
        config={provider,model,extra:{hermes_profile:profile},instance:'hermes:default',permissionMode:mode,mode};
      } else {
        if(!config)throw new Error(`Unknown matrix provider ${provider}`);
        const health=await ctx.driver.invoke('agent_chat_provider_health',{provider});
        if(health.status!=='ready')throw new Error(health.message??'Provider readiness is unconfirmed');
        const caps=await ctx.driver.invoke('list_chat_provider_capabilities',{provider,force:false,accountId:null});
        if(!caps.models.some(m=>m.id===config.model)) {
          const cheap=caps.models.find(m=>/luna|haiku|mini|flash|free/i.test(m.id));
          if(!cheap)throw new Error('No known inexpensive advertised model; set the provider model override explicitly');
          config={...config,model:cheap.id};
        }
      }
      ctx.observe(`${provider} selected model`,config.model);
    } catch(error) {
      for(const role of ['coordinator','worker'])matrix.push({provider,role,status:'skip',reason:String(error)});
      continue;
    }
    for(const role of ['coordinator','worker']) {
      try {await report(ctx,role==='coordinator'?config:{...CLAUDE,permissionMode:provider==='opencode'||config.permissionMode==='default'?'default':'bypassPermissions'},role==='worker'?config:{...CLAUDE,mode:config.permissionMode==='agent'||config.permissionMode==='dont_ask'?'bypassPermissions':'default'});matrix.push({provider,role,status:'pass'});}
      catch(error){matrix.push({provider,role,status:'fail',reason:String(error)});ctx.observe(`failure ${provider} ${role}`,String(error));await ctx.shot(`pr4-${provider}-${role}-failure`).catch(()=>{});}
    }
  }
  ctx.observe('provider matrix',matrix);
  // Gaps remain visible in the machine report and never count as a pass.
  ctx.check('all requested provider roles passed',matrix.length===selected.length*2&&matrix.every(r=>r.status==='pass'),matrix);
}
