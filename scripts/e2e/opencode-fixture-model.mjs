// Scripted loopback model used ONLY by OpenCode compatibility acceptance.
// OpenCode itself performs tools, permissions, persistence and SSE delivery.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { join } from 'node:path';

export async function startFixtureModel(directory) {
  const requests = [];
  const errors = [];
  const held = new Set();
  const handler = async (request, response) => {
    if (request.method !== 'POST' || !request.url.endsWith('/chat/completions')) {
      response.writeHead(404).end();
      return;
    }
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks).toString());
    const messages = body.messages ?? [];
    const user = [...messages].reverse().find(message => message.role === 'user');
    const text = typeof user?.content === 'string' ? user.content : JSON.stringify(user?.content);
    requests.push({ model: body.model, text, toolNames: (body.tools ?? []).map(tool => tool.function?.name) });
    const writeResult = messages.some(message => message.role === 'tool');
    const tool = body.tools?.find(tool => tool.function?.name === 'write');
    // OpenCode's automatic title request can quote the user's edit marker
    // without supplying tools. Only the actual coding request emits a call.
    const shouldWrite = text?.includes('OC_FIXTURE_EDIT') && !writeResult && !!tool;
    const hold = text?.includes('OC_FIXTURE_HOLD');
    let message;
    if (shouldWrite) {
      const properties = tool.function.parameters.properties;
      const fileKey = properties.filePath ? 'filePath' : properties.path ? 'path' : properties.file ? 'file' : undefined;
      assert.ok(fileKey, 'Unknown OpenCode write-tool schema');
      message = { role: 'assistant', content: null, tool_calls: [{ id: 'fixture_write', type: 'function', function: { name: 'write', arguments: JSON.stringify({ [fileKey]: join(directory, 'opencode-proof.txt'), content: 'OpenCode compatibility verified\n' }) } }] };
    } else {
      message = { role: 'assistant', content: hold ? 'Fixture turn waiting' : 'OpenCode fixture response complete' };
    }
    const finish = shouldWrite ? 'tool_calls' : 'stop';
    const usage = { prompt_tokens: 12, completion_tokens: 8, total_tokens: 20 };
    if (!body.stream) {
      response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ id: 'fixture_completion', object: 'chat.completion', created: 1, model: body.model, choices: [{ index: 0, message, finish_reason: finish }], usage }));
      return;
    }
    response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
    const emit = (delta, finish_reason = null, includeUsage = false) => response.write(`data: ${JSON.stringify({ id: 'fixture_completion', object: 'chat.completion.chunk', created: 1, model: body.model, choices: [{ index: 0, delta, finish_reason }], ...(includeUsage ? { usage } : {}) })}\n\n`);
    emit({ role: 'assistant' });
    if (shouldWrite) {
      const call = message.tool_calls[0];
      emit({ tool_calls: [{ index: 0, ...call }] });
    } else {
      for (const content of ['OpenCode ', hold ? 'fixture waiting' : 'fixture response complete']) emit({ content });
    }
    if (hold) {
      held.add(response);
      response.once('close', () => held.delete(response));
      return;
    }
    emit({}, finish, true);
    response.end('data: [DONE]\n\n');
  };
  const server = createServer((request, response) => {
    void handler(request, response).catch(error => {
      errors.push(String(error));
      if (!response.headersSent) response.writeHead(500);
      response.end('Scripted model fixture failed');
    });
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const baseURL = `http://127.0.0.1:${server.address().port}/v1`;
  return {
    baseURL, requests, errors,
    get heldCount() { return held.size; },
    async stop() {
      for (const response of held) response.destroy();
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    },
  };
}

export function fixtureConfig(version, baseURL) {
  const model = { name: 'Compatibility fixture', limit: { context: 32768, output: 4096 } };
  if (version === 2) {
    return {
      update: 'disable', snapshots: false, share: 'disabled', model: 'compatibility/fixture',
      permissions: [{ action: 'edit', resource: '*', effect: 'ask' }],
      providers: { compatibility: { name: 'Compatibility fixture', package: '@opencode/ai/providers/openai-compatible', settings: { baseURL, apiKey: 'test-only' }, models: { fixture: { ...model, capabilities: { tools: true, input: ['text', 'image'], output: ['text'] }, variants: [{ id: 'low', settings: { reasoningEffort: 'low' } }, { id: 'high', settings: { reasoningEffort: 'high' } }] } } } },
    };
  }
  return {
    autoupdate: false, snapshot: false, share: 'disabled', model: 'compatibility/fixture',
    permission: { edit: 'ask' },
    provider: { compatibility: { name: 'Compatibility fixture', npm: '@ai-sdk/openai-compatible', options: { baseURL, apiKey: 'test-only' }, models: { fixture: { ...model, modalities: { input: ['text', 'image'], output: ['text'] }, variants: { low: { reasoningEffort: 'low' }, high: { reasoningEffort: 'high' } } } } } },
  };
}
