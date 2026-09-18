#!/usr/bin/env python3
"""Verify OpenCode safe steering with a local deterministic model and harmless tool.

No external model API is used. Requires an installed OpenCode v1 executable.
Set OPENCODE_BIN to the actual binary if the PATH entry is a version-manager
shim (XDG homes are isolated); set PROBE_TMPDIR to choose temporary storage.
"""
import json, os, shutil, socket, subprocess, tempfile, threading, time, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
requests = []
tool_started = threading.Event()
ended = threading.Event()
evidence = []

class Model(BaseHTTPRequestHandler):

    def log_message(self, *args):
        pass

    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages = data.get('messages', [])
        tools = data.get('tools', [])
        is_main = any((t.get('function', {}).get('name') == 'bash' for t in tools))
        has_result = any((m.get('role') == 'tool' for m in messages))
        if is_main:
            requests.append(messages)
        toolcall = is_main and (not has_result)
        delta = {'tool_calls': [{'index': 0, 'id': 'probe-hold', 'type': 'function', 'function': {'name': 'bash', 'arguments': json.dumps({'command': 'sleep 3; printf TOOL_COMPLETED', 'description': 'Harmless delivery timing probe'})}}]} if toolcall else {'content': 'PROBE_DONE'}
        if data.get('stream'):
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for d, finish in [(delta, None), ({}, 'tool_calls' if toolcall else 'stop')]:
                event = {'id': 'probe-response', 'object': 'chat.completion.chunk', 'created': int(time.time()), 'model': 'probe', 'choices': [{'index': 0, 'delta': d, 'finish_reason': finish}]}
                self.wfile.write(('data: ' + json.dumps(event) + '\n\n').encode())
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()
        else:
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'id': 'probe-response', 'object': 'chat.completion', 'created': int(time.time()), 'model': 'probe', 'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': 'PROBE_DONE'}, 'finish_reason': 'stop'}], 'usage': {'prompt_tokens': 1, 'completion_tokens': 1, 'total_tokens': 2}}).encode())
server = ThreadingHTTPServer(('127.0.0.1', 0), Model)
threading.Thread(target=server.serve_forever, daemon=True).start()
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
with tempfile.TemporaryDirectory(prefix='codemux-opencode-probe-', dir=os.environ.get('PROBE_TMPDIR')) as tmp:
    env = os.environ.copy()
    env.update({'XDG_CONFIG_HOME': tmp + '/config', 'XDG_DATA_HOME': tmp + '/data', 'XDG_STATE_HOME': tmp + '/state', 'XDG_CACHE_HOME': tmp + '/cache', 'OPENCODE_DISABLE_DEFAULT_PLUGINS': 'true', 'OPENCODE_DISABLE_AUTOUPDATE': 'true', 'OPENCODE_CONFIG_CONTENT': json.dumps({'provider': {'probe': {'npm': '@ai-sdk/openai-compatible', 'name': 'Probe', 'options': {'baseURL': f'http://127.0.0.1:{server.server_port}/v1', 'apiKey': 'local-probe'}, 'models': {'probe': {'name': 'Probe', 'limit': {'context': 32768, 'output': 1024}}}}}, 'model': 'probe/probe', 'small_model': 'probe/probe', 'permission': {'bash': 'allow'}, 'share': 'disabled'})})
    log = open(tmp + '/server.log', 'w+')
    proc = subprocess.Popen([os.environ.get('OPENCODE_BIN', shutil.which('opencode') or 'opencode'), 'serve', '--hostname', '127.0.0.1', '--port', str(port)], cwd=tmp, env=env, stdout=log, stderr=log)
    base = f'http://127.0.0.1:{port}'

    def api(path, body=None):
        req = urllib.request.Request(base + path, data=json.dumps(body).encode() if body is not None else None, headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=20) as r:
            return json.loads(r.read() or 'null')
    try:
        for _ in range(100):
            try:
                api('/global/health')
                break
            except Exception:
                time.sleep(0.2)
        session = api('/session', {})['id']

        def events():
            try:
                with urllib.request.urlopen(base + '/event', timeout=25) as r:
                    for line in r:
                        if not line.startswith(b'data:'):
                            continue
                        event = json.loads(line[5:])
                        prop = event.get('properties', {})
                        if prop.get('sessionID', prop.get('part', {}).get('sessionID')) != session:
                            continue
                        part = prop.get('part', {})
                        if part.get('type') == 'tool':
                            status = part.get('state', {}).get('status')
                            evidence.append(status)
                            if status == 'running':
                                tool_started.set()
                        if event.get('type') == 'session.idle':
                            ended.set()
                            return
            except Exception as e:
                evidence.append(str(e))
        threading.Thread(target=events, daemon=True).start()
        time.sleep(0.2)
        api('/session/' + session + '/prompt_async', {'model': {'providerID': 'probe', 'modelID': 'probe'}, 'parts': [{'type': 'text', 'text': 'Run the timing probe'}]})
        assert tool_started.wait(25), 'No tool start: ' + str(evidence)
        api('/session/' + session + '/prompt_async', {'model': {'providerID': 'probe', 'modelID': 'probe'}, 'parts': [{'type': 'text', 'text': 'STEER_PROBE_CORRECTION'}]})
        assert ended.wait(25), 'No completion: ' + str(evidence)
        subsequent = [m for m in requests if any((x.get('role') == 'tool' for x in m))]
        assert subsequent, 'No subsequent model request'
        seen = json.dumps(subsequent[0])
        assert 'STEER_PROBE_CORRECTION' in seen and 'TOOL_COMPLETED' in seen, seen
        assert 'completed' in evidence and 'error' not in evidence, evidence
        print(json.dumps({'passed': True, 'tool_lifecycle': evidence, 'correction_and_tool_result_in_next_request': True, 'main_model_requests': len(requests)}))
    except Exception as e:
        log.flush()
        log.seek(0)
        print(str(e))
        print(log.read()[-4000:])
        raise
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        log.close()
server.shutdown()
