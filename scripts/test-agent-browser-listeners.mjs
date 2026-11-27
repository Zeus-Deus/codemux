// Exercises the real patched sidecar listeners without launching Chromium.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile, rm, chmod } from 'node:fs/promises';
import { createServer, connect } from 'node:net';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { randomBytes } from 'node:crypto';

const binary = resolve(process.argv[2] ?? '');
if (!process.argv[2]) throw new Error('Pass the patched sidecar binary path');

async function freePort() {
  const server = createServer();
  await new Promise((done, reject) => server.listen(0, '127.0.0.1', done).once('error', reject));
  const { port } = server.address();
  await new Promise((done) => server.close(done));
  return port;
}

async function wire(target, request, terminator) {
  return new Promise((done, reject) => {
    const socket = connect(target);
    let result = '';
    const timer = setTimeout(() => finish(new Error('Listener response timed out')), 5000);
    const finish = (error) => {
      clearTimeout(timer);
      socket.destroy();
      if (error) reject(error);
      else done(result);
    };
    socket.on('error', finish);
    socket.on('connect', () => socket.write(request));
    socket.on('data', (data) => {
      result += data.toString();
      if (terminator && result.includes(terminator)) finish();
    });
    socket.on('end', () => finish(terminator ? new Error(`Listener closed early: ${result}`) : undefined));
  });
}

function httpRequest(port, method, path, token, body = '') {
  const headers = [
    `${method} ${path} HTTP/1.1`,
    `Host: 127.0.0.1:${port}`,
    `Origin: http://127.0.0.1:${port}`,
    'Connection: close',
  ];
  if (token) headers.push(`X-Codemux-Browser-Token: ${token}`);
  if (body) headers.push('Content-Type: application/json', `Content-Length: ${Buffer.byteLength(body)}`);
  return `${headers.join('\r\n')}\r\n\r\n${body}`;
}

function wsRequest(port, token, origin = '') {
  const headers = [
    'GET / HTTP/1.1',
    `Host: 127.0.0.1:${port}`,
    'Upgrade: websocket',
    'Connection: Upgrade',
    `Sec-WebSocket-Key: ${randomBytes(16).toString('base64')}`,
    'Sec-WebSocket-Version: 13',
  ];
  if (origin) headers.push(`Origin: ${origin}`);
  if (token) headers.push(`X-Codemux-Browser-Token: ${token}`);
  return `${headers.join('\r\n')}\r\n\r\n`;
}

async function waitForFile(path, child) {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (child.exitCode !== null) throw new Error(`Sidecar exited: ${child.exitCode}`);
    try { return (await readFile(path, 'utf8')).trim(); } catch {}
    await new Promise((done) => setTimeout(done, 50));
  }
  throw new Error(`Sidecar did not publish ${path}`);
}

const directory = await mkdtemp(join(process.platform === 'win32' ? tmpdir() : '/var/tmp', 'cmx-ab-auth-'));
if (process.platform !== 'win32') await chmod(directory, 0o700);
const policyPath = join(directory, 'policy.json');
await writeFile(policyPath, '{"confirm":["screenshot"]}');
const children = [];

try {
  const sessions = [];
  for (const name of ['auth-a', 'auth-b']) {
    const token = randomBytes(32).toString('hex');
    const requestedPort = await freePort();
    const child = spawn(binary, [], {
      env: {
        ...process.env,
        AGENT_BROWSER_DAEMON: '1',
        AGENT_BROWSER_SESSION: name,
        AGENT_BROWSER_SOCKET_DIR: directory,
        AGENT_BROWSER_STREAM_PORT: String(requestedPort),
        ...(name === 'auth-a' ? { AGENT_BROWSER_ACTION_POLICY: policyPath } : {}),
        CODEMUX_BROWSER_SECRET: token,
      },
      stdio: ['ignore', 'ignore', 'pipe'],
    });
    children.push(child);
    let stderr = '';
    child.stderr.on('data', (chunk) => { stderr += chunk.toString(); });
    const port = Number(await waitForFile(join(directory, `${name}.stream`), child));
    assert(port > 0, `No stream port for ${name}: ${stderr}`);
    const daemonTarget = process.platform === 'win32'
      ? { host: '127.0.0.1', port: Number(await waitForFile(join(directory, `${name}.port`), child)) }
      : { path: join(directory, `${name}.sock`) };
    sessions.push({ name, token, port, daemonTarget });
  }

  const [a, b] = sessions;
  const streamTarget = { host: '127.0.0.1', port: a.port };
  const status = (token) => wire(streamTarget, httpRequest(a.port, 'GET', '/api/status', token), '\r\n\r\n');
  assert.match(await status(), /^HTTP\/1\.1 403 /);
  assert.match(await status('f'.repeat(64)), /^HTTP\/1\.1 403 /);
  assert.match(await status(b.token), /^HTTP\/1\.1 403 /);
  assert.match(await status(a.token), /^HTTP\/1\.1 200 /);
  for (const path of ['/api/tabs', '/api/not-found']) {
    assert.match(await wire(streamTarget, httpRequest(a.port, 'GET', path), '\r\n\r\n'), /^HTTP\/1\.1 403 /);
    assert.doesNotMatch(await wire(streamTarget, httpRequest(a.port, 'GET', path, a.token), '\r\n\r\n'), /^HTTP\/1\.1 403 /);
  }

  const commandBody = JSON.stringify({ id: 'auth-test', action: 'stream_status' });
  const command = (token) => wire(streamTarget, httpRequest(a.port, 'POST', '/api/command', token, commandBody), '\r\n\r\n');
  assert.match(await command(), /^HTTP\/1\.1 403 /);
  assert.match(await command(b.token), /^HTTP\/1\.1 403 /);
  const commandResponse = await wire(streamTarget,
    httpRequest(a.port, 'POST', '/api/command', a.token, commandBody), null);
  assert.match(commandResponse, /^HTTP\/1\.1 200 /);
  const commandResult = JSON.parse(commandResponse.split('\r\n\r\n', 2)[1]);
  assert.equal(commandResult.success, true);
  assert.match(await wire(streamTarget, httpRequest(a.port, 'OPTIONS', '/api/command'), '\r\n\r\n'), /^HTTP\/1\.1 403 /);

  const websocket = (token, origin) => wire(streamTarget, wsRequest(a.port, token, origin), '\r\n\r\n');
  assert.match(await websocket(), /^HTTP\/1\.1 403 /);
  assert.match(await websocket(undefined, 'file://'), /^HTTP\/1\.1 403 /);
  assert.match(await websocket(b.token), /^HTTP\/1\.1 403 /);
  assert.match(await websocket('f'.repeat(64), 'file://'), /^HTTP\/1\.1 403 /);
  assert.match(await websocket(a.token), /^HTTP\/1\.1 101 /);

  const daemonRequest = (token) => wire(a.daemonTarget,
    `${JSON.stringify({ id: 'auth-test', action: 'stream_status', ...(token ? { codemuxToken: token } : {}) })}\n`, '\n');
  for (const token of [undefined, 'f'.repeat(64), b.token]) {
    const response = JSON.parse(await daemonRequest(token));
    assert.equal(response.success, false);
    assert.equal(response.error, 'Unauthorized');
  }
  const authorized = JSON.parse(await daemonRequest(a.token));
  assert.equal(authorized.success, true);

  const daemonCommand = async (command) => JSON.parse(await wire(a.daemonTarget,
    `${JSON.stringify({ ...command, codemuxToken: a.token })}\n`, '\n'));
  const pending = await daemonCommand({ id: 'screenshot-1', action: 'screenshot' });
  const confirmationId = pending.data.confirmation_id;
  assert.match(confirmationId, /^[a-f0-9-]{36}$/);
  assert.notEqual(confirmationId, 'screenshot-1');
  for (const decision of [
    { id: 'decision-1', action: 'confirm' },
    { id: 'decision-2', action: 'confirm', confirmationId: 'wrong' },
    { id: 'decision-3', action: 'deny', confirmationId: 'wrong' },
  ]) {
    const rejected = await daemonCommand(decision);
    assert.equal(rejected.success, false);
    assert.match(rejected.error, /Confirmation ID/);
  }
  const denied = await daemonCommand({ id: 'decision-4', action: 'deny', confirmationId });
  assert.equal(denied.data.denied, true);

  console.log('Browser WS, HTTP API, and daemon listeners enforce tokens; wrong confirmation IDs preserve the pending command.');
} finally {
  await Promise.all(children.map(async (child) => {
    if (child.exitCode !== null) return;
    child.kill();
    await Promise.race([
      new Promise((done) => child.once('exit', done)),
      new Promise((done) => setTimeout(done, 2000)),
    ]);
  }));
  await rm(directory, { recursive: true, force: true });
}
