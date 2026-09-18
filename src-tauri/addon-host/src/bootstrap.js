// No OS, imports, networking, or renderer objects are exposed to the child.
(() => {
  const sendNative = globalThis.__nativeSend;
  const now = globalThis.__nativeNow;
  let generation, manifest, plugin, dispatch;
  let sequence = 0, active = false;
  const timers = new Map();
  const send = (method, params, id) => sendNative(JSON.stringify({jsonrpc:'2.0',generation,method,params,...(id === undefined ? {} : {id})}));
  Object.defineProperty(globalThis, '__configure', {value(g, m) { if (generation) throw Error('Already configured'); generation=g; manifest=JSON.parse(m); }});
  Object.defineProperty(globalThis, '__codemuxRegister', {value(definition, adapter) { if (plugin) throw Error('Duplicate plugin'); plugin=definition; dispatch=adapter({manifest,send,now}); }});
  Object.defineProperty(globalThis, '__dispatch', {value(raw) {
    const message=JSON.parse(raw);
    if (!dispatch) throw Error('No plugin registered');
    if (message.method === 'activate') { if (active) throw Error('Already active'); active=true; }
    if (!active) throw Error('Not active');
    dispatch(message,plugin);
  }});
  Object.defineProperty(globalThis, '__nextWake', {value() {
    let next=Infinity; for (const timer of timers.values()) next=Math.min(next,timer.at);
    return next===Infinity ? -1 : Math.max(0,next-now());
  }});
  Object.defineProperty(globalThis, '__tick', {value() {
    const time=now();
    for (const [id,timer] of [...timers]) {
      if (timer.at>time || !timers.has(id)) continue;
      if (timer.repeat) timer.at=time+timer.delay; else timers.delete(id);
      timer.callback(...timer.args);
    }
  }});
  const timer=(repeat,callback,delay,args) => {
    if (typeof callback!=='function' || timers.size>=128) throw Error('Timer limit');
    delay=Math.max(100,Math.min(Number(delay)||100,2147483647));
    const id=++sequence; timers.set(id,{callback,delay,args,repeat,at:now()+delay}); return id;
  };
  globalThis.setTimeout=(callback,delay,...args)=>timer(false,callback,delay,args);
  globalThis.setInterval=(callback,delay,...args)=>timer(true,callback,delay,args);
  globalThis.clearTimeout=globalThis.clearInterval=id=>timers.delete(id);
  globalThis.console=Object.freeze(Object.fromEntries(['log','info','warn','error','debug'].map(level=>[level,(...args)=>send('log',{message:args.map(x=>String(x).slice(0,1024)).join(' ').slice(0,4096)})])));
  // UTF-8 helpers only. Native networking remains behind the broker.
  globalThis.TextEncoder=class { encode(value='') { const s=unescape(encodeURIComponent(String(value))); return Uint8Array.from(s,c=>c.charCodeAt(0)); } };
  globalThis.TextDecoder=class { decode(value=new Uint8Array()) { let s=''; for (const byte of value) s+=String.fromCharCode(byte); return decodeURIComponent(escape(s)); } };
  delete globalThis.__nativeSend; delete globalThis.__nativeNow;
})();
