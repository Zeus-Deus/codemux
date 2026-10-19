import json,subprocess,tempfile,threading,time,queue,pathlib,os
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
out=pathlib.Path.home()/'.cache/codemux-agents/proof/codemux-plugin-probe';out.mkdir(parents=True,exist_ok=True)
log=[];events=queue.Queue();stage=0;stop=threading.Event()
def note(kind,**data):
 entry={'kind':kind,**data};log.append(entry);print(json.dumps(entry),flush=True)
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*a): pass
 def do_GET(self):
  note('mcp_get',path=self.path)
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
  while not stop.is_set():
   try: data=events.get(timeout=.5)
   except queue.Empty:
    data=': keepalive\n\n'
   try:self.wfile.write(data.encode());self.wfile.flush()
   except (BrokenPipeError,ConnectionResetError): break
 def do_POST(self):
  raw=self.rfile.read(int(self.headers.get('content-length',0)))
  try:d=json.loads(raw)
  except:d={}
  if self.path.endswith('/responses'):
   names=[]
   for t in d.get('tools',[]):
    names.append(t.get('name') or t.get('type'))
   note('inference_tools',names=names)
   self.send_response(400);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{"error":{"message":"probe capture only","type":"invalid_request_error"}}');return
  method=d.get('method');note('mcp_post',method=method,stage=stage)
  if 'id' not in d:self.send_response(202);self.end_headers();return
  if method=='initialize':result={'protocolVersion':'2025-06-18','capabilities':{'tools':{'listChanged':True}},'serverInfo':{'name':'probe','version':'1'}}
  elif method=='tools/list':result={'tools':([] if stage==0 else [{'name':'probe_new_tool','description':'A tool added after the session starts.','inputSchema':{'type':'object','properties':{}}}])}
  elif method=='tools/call':result={'content':[{'type':'text','text':'probe success'}]}
  else:result={}
  if method=='initialize':
   self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Mcp-Session-Id','probe-session');self.end_headers()
   self.wfile.write(('event: message\ndata: '+json.dumps({'jsonrpc':'2.0','id':d['id'],'result':result})+'\n\n').encode());self.wfile.flush()
   while not stop.is_set():
    try:event=events.get(timeout=.5)
    except queue.Empty:continue
    try:self.wfile.write(event.encode());self.wfile.flush()
    except (BrokenPipeError,ConnectionResetError):break
   return
  body=json.dumps({'jsonrpc':'2.0','id':d['id'],'result':result}).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.send_header('Mcp-Session-Id','probe-session');self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start();port=server.server_port
home=tempfile.mkdtemp(prefix='codemux-codex-probe-',dir=out)
env=dict(os.environ,CODEX_HOME=home,PROBE_MCP_TOKEN='probe-token')
args=['codex','app-server','--listen','stdio://','-c','model_provider="probe"','-c','model_providers.probe.name="Probe"','-c',f'model_providers.probe.base_url="http://127.0.0.1:{port}/v1"','-c','model_providers.probe.wire_api="responses"','-c','model_providers.probe.requires_openai_auth=false','-c',f'mcp_servers.probe.url="http://127.0.0.1:{port}/mcp"','-c','mcp_servers.probe.bearer_token_env_var="PROBE_MCP_TOKEN"','-c','model="gpt-5.4"']
p=subprocess.Popen(args,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=open(out/'stderr.log','w'),text=True,env=env)
responses=queue.Queue()
def read():
 for line in p.stdout:
  try:responses.put(json.loads(line))
  except:pass
threading.Thread(target=read,daemon=True).start();seq=0
pending=[]
def send(method,params,timeout=25):
 global seq
 seq+=1;ident=seq;p.stdin.write(json.dumps({'jsonrpc':'2.0','id':ident,'method':method,'params':params})+'\n');p.stdin.flush();end=time.monotonic()+timeout
 while time.monotonic()<end:
  d=responses.get(timeout=max(.1,end-time.monotonic()))
  if d.get('id')==ident:note('rpc',method=method,result=d.get('result'),error=d.get('error'));return d
  pending.append(d)
 raise TimeoutError(method)
try:
 send('initialize',{'clientInfo':{'name':'codemux-probe','version':'1'},'capabilities':{'experimentalApi':True}})
 p.stdin.write('{"jsonrpc":"2.0","method":"initialized"}\n');p.stdin.flush()
 tool=lambda name:{'type':'function','name':name,'description':'Probe dynamic tool','inputSchema':{'type':'object','properties':{}}}
 d=send('thread/start',{'cwd':home,'approvalPolicy':'never','sandbox':'read-only','dynamicTools':[tool('dynamic_before')]});tid=d['result']['thread']['id']
 send('turn/start',{'threadId':tid,'input':[{'type':'text','text':'Say hello.'}]})
 deadline=time.time()+25
 while not any(e['kind']=='inference_tools' for e in log) and time.time()<deadline:time.sleep(.1)
 stage=1;events.put('event: message\ndata: {"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\n\n');note('sent_list_changed')
 time.sleep(2)
 send('config/mcpServer/reload',None)
 time.sleep(2)
 send('thread/resume',{'threadId':tid,'dynamicTools':[tool('dynamic_after')]})
 send('turn/start',{'threadId':tid,'input':[{'type':'text','text':'Say hello again.'}]})
 deadline=time.time()+25
 while len([e for e in log if e['kind']=='inference_tools'])<2 and time.time()<deadline:time.sleep(.1)
 note('summary',lists=[e for e in log if e.get('method')=='tools/list'],inference=[e for e in log if e['kind']=='inference_tools'])
finally:
 stop.set();p.terminate()
 try:p.wait(timeout=5)
 except: p.kill();p.wait()
 (out/'evidence.json').write_text(json.dumps(log,indent=2));server.shutdown()
