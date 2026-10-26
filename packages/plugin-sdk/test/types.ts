import {
  apiVersion, hasCapability, operationDisposition,
  type PluginManifest, type PluginSetting, type PluginModule, type PluginContext, type PluginConversationCard,
  type PluginModelSelection, type PluginOperationReceipt, type PluginViewBridge,
} from '../src/index.js';
const api: 1 = apiVersion;
void api;
const requiredOnly: PluginManifest = {id:'example',name:'Example',version:'1.0.0',apiVersion:1,entry:'plugin.mjs',requiredCapabilities:['storage']};
void requiredOnly;
// @ts-expect-error A secret setting cannot embed a default in an installed package.
const secret: PluginSetting = {key:'token',label:'Token',type:'secret',default:'embedded'};
void secret;
const plugin: PluginModule = {
  activate(ctx) {
    ctx.handle('action:check', async (_input, invocation) => {
      if (invocation.caller.kind !== 'agent' || !invocation.caller.threadId) throw new Error('Verified agent required');
      invocation.signal.throwIfAborted();
      const source = await ctx.threads.metadata({ threadId: invocation.caller.threadId });
      const result = await ctx.threads.create({ projectId: source.projectId, text: 'Review', idempotencyKey: 'review',
        modelSelection: { instanceId: 'codex', model: 'gpt-6.1-sol', options: [{id:'effort',value:'xhigh'}] }, nativePermissionMode:'workspace-write' });
      const current = await ctx.workflow.query({ query:'operation', operationId:result.operation_id });
      const effect: PluginOperationReceipt = await ctx.workflow.query({ query:'effect', idempotencyKey:'review' });
      void effect;
      const status = await ctx.workflow.query({ query:'thread-status', threadId:source.id });
      const page = await ctx.workflow.query({ query:'models', projectId:source.projectId });
      const nativeModes: readonly string[] = page.models[0]?.permissionModes ?? [];
      const options = page.models[0]?.options ?? [];
      for (const option of options) if (option.type === 'select') {
        const value: string | undefined = option.options[0]?.id;
        void value;
      }
      if (hasCapability(ctx, 'ui.changes')) await ctx.ui.invalidate(['view:review']);
      return {message:`Command ${operationDisposition(current)}; thread phase ${status.status.phase}`,data:{nativeModes}};
    });
    ctx.events.onRunFinalized(async event => {
      if (event.effectsAllowed && event.outcome === 'completed' && hasCapability(ctx,'threads.send')) {
        await ctx.threads.send({threadId:event.threadId,text:'Review',idempotencyKey:'review'});
      }
    });
    ctx.trees.register('projects', () => ({nodes:[{id:'project',kind:'project',projectId:'example'}]}));
  }
};
void plugin;
function contracts(ctx: PluginContext, receipt: PluginOperationReceipt, bridge: PluginViewBridge) {
  // @ts-expect-error Native receipt keys retain snake_case; there is no synthetic completion.
  const thread: string = receipt.threadId;
  // @ts-expect-error Portable auto has no reviewed native mapping.
  ctx.threads.create({projectId:'project',text:'review',idempotencyKey:'once',runtimeMode:'auto'});
  // @ts-expect-error Mutations require a stable idempotency key.
  ctx.threads.send({threadId:'thread',text:'review'});
  // @ts-expect-error Caller identity is host-owned and immutable.
  ctx.handle('action:test', (_input, invocation) => { invocation.caller.kind = 'client'; });
  // @ts-expect-error Effect recovery requires the original mutation's stable key.
  ctx.workflow.query({query:'effect'});
  // @ts-expect-error Canonical model option selections are strongly typed.
  const model: PluginModelSelection = {instanceId:'codex',model:'gpt',options:[{id:'effort',value:12}]};
  // @ts-expect-error Authors cannot stamp installation provenance into card references.
  const card: PluginConversationCard = {viewId:'review',summary:'Review proposal',installationId:'fake'};
  // @ts-expect-error HTML views cannot select another installation.
  bridge.invoke({installationId:'someone-else'},{});
  void thread; void model; void card;
}
void contracts;
