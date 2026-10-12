#!/usr/bin/env python3
# Synthetic ACP peer for native guard/queue/approval/Stop regressions.
# The Rust fixture supplies a distinct disposable rig through the child env.
import json,sys,os,pathlib,threading,time
held={};session='review-session';mode='';rig=pathlib.Path(os.environ['CODEMUX_AGENT_CONTROL_FIXTURE_ROOT'])
rig.mkdir(exist_ok=True)
(rig/'peer-pid').write_text(str(os.getpid()))
def record(v):
 with (rig/'peer-wire.jsonl').open('a') as f:f.write(json.dumps(v)+'\n')
def out(v):
 v=dict(jsonrpc='2.0',**v);record({'direction':'out','message':v});print(json.dumps(v),flush=True)
def options():return [{'id':'model','category':'model','type':'select','currentValue':'fake-model','options':[{'value':'fake-model','name':'Synthetic'},{'value':'review-fatal','name':'Synthetic RPC failure'},{'value':'stop-race-held','name':'Held actual config'},{'value':'stop-race-reject','name':'Held config error'}]},{'id':'mode','category':'mode','type':'select','currentValue':'ask','options':[{'value':'ask','name':'Ask'},{'value':'agent','name':'Agent'}]}]
def update(v):out({'method':'session/update','params':{'sessionId':session,'update':v}})
for line in sys.stdin:
 m=json.loads(line);record({'direction':'in','message':m});method=m.get('method');p=m.get('params',{});id=m.get('id')
 if method is None:
  original=held.pop(str(id),None)
  if original is not None:
   if mode=='before-terminal':sys.exit(37)
   chosen=m.get('result',{}).get('outcome',{}).get('optionId','cancelled')
   update({'sessionUpdate':'tool_call_update','toolCallId':'review-tool','status':'completed','content':[{'type':'content','content':{'type':'text','text':'visible result '+chosen}}]})
   update({'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'wire-option:'+chosen}})
   out({'id':original,'result':{'stopReason':'end_turn'}})
  continue
 if method in ['initialize','authenticate','session/load']:out({'id':id,'result':{}})
 elif method=='session/new':out({'id':id,'result':{'sessionId':session,'configOptions':options()}})
 elif method=='session/set_config_option':
  if p.get('value')=='stop-race-reject':
   (rig/'config-reject-held.json').write_text(json.dumps(m))
   def reject_later(request_id=id):
    while not (rig/'release-config-reject').is_file():time.sleep(0.001)
    out({'id':request_id,'error':{'code':-32000,'message':'synthetic held configuration rejection'}})
   threading.Thread(target=reject_later,daemon=True).start()
   continue
  if p.get('value')=='stop-race-held':
   (rig/'config-held.json').write_text(json.dumps(m))
   continue
  if p.get('value')=='review-fatal':out({'id':id,'error':{'code':-32000,'message':'synthetic config transport failure'}})
  else:out({'id':id,'result':{'configOptions':options()}})
 elif method=='session/prompt':
  text=' '.join(b.get('text','') for b in p.get('prompt',[]))
  if 'review-hold' in text or 'review-approval' in text or 'fr1-hold' in text:
   if 'fr1-hold' in text:
    mode=text.split()[-1];(rig/'fr1-peer-pid').write_text(str(os.getpid()))
   update({'sessionUpdate':'tool_call','toolCallId':'review-tool','title':'Synthetic review tool','kind':'execute','status':'pending','rawInput':{'command':'synthetic'}})
   rid='peer-'+str(id);held[rid]=id
   kinds=['allow_always'] if 'only-always' in text else ['allow_once','allow_always','reject_once','reject_always']
   out({'id':rid,'method':'session/request_permission','params':{'sessionId':session,'toolCall':{'toolCallId':'review-tool'},'options':[{'optionId':'opaque-'+k,'kind':k,'name':k} for k in kinds]}})
  elif text=='review-runtime-fatal':out({'id':id,'error':{'code':-32000,'message':'synthetic runtime transport failure'}})
  else:
   update({'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':text}})
   out({'id':id,'result':{'stopReason':'end_turn'}})
 elif method=='session/cancel':
  for rid,original in list(held.items()):out({'id':original,'result':{'stopReason':'cancelled'}})
  held.clear()
 elif id is not None:out({'id':id,'result':{}})
