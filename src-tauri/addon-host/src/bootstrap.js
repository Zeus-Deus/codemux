// No OS, imports, networking, or renderer objects are exposed to the child.
(() => {
  const sendNative = globalThis.__nativeSend;
  const now = globalThis.__nativeNow;
  const setTimer = globalThis.__nativeTimer;
  const clearTimer = globalThis.__nativeClearTimer;
  // Plugin code shares this realm. Capture every intrinsic used below so a
  // replaced builtin can only run inside the plugin's own accounted callbacks.
  const {apply} = Reflect, {parse, stringify} = JSON, {create, defineProperty, freeze, fromEntries} = Object;
  let generation, manifest, plugin, dispatch;
  let active = false;
  // Rust owns timer limits and wakeups; this null-prototype table maps IDs to callbacks.
  const timers = create(null);
  const send = (method, params, id) => sendNative(stringify({jsonrpc:'2.0',generation,method,params,...(id === undefined ? {} : {id})}));
  defineProperty(globalThis, '__configure', {value(g, m) { if (generation) throw Error('Already configured'); generation=g; manifest=parse(m); }});
  defineProperty(globalThis, '__codemuxRegister', {value(definition, adapter) { if (plugin) throw Error('Duplicate plugin'); plugin=definition; dispatch=adapter({manifest,send,now}); }});
  defineProperty(globalThis, '__dispatch', {value(raw) {
    const message=parse(raw);
    if (!dispatch) throw Error('No plugin registered');
    if (message.method === 'activate') { if (active) throw Error('Already active'); active=true; }
    if (!active) throw Error('Not active');
    dispatch(message,plugin);
  }});
  defineProperty(globalThis, '__tick', {value(id) {
    const timer=timers[id];
    if (!timer) return;
    if (!timer.repeat) delete timers[id];
    apply(timer.callback,undefined,timer.args);
  }});
  const timer=(repeat,callback,delay,args) => {
    if (typeof callback!=='function') throw Error('Timer limit');
    const id=setTimer(+delay,repeat); timers[id]={callback,args,repeat}; return id;
  };
  globalThis.setTimeout=(callback,delay,...args)=>timer(false,callback,delay,args);
  globalThis.setInterval=(callback,delay,...args)=>timer(true,callback,delay,args);
  globalThis.clearTimeout=globalThis.clearInterval=id=>{ if (typeof id==='number' && timers[id]) { delete timers[id]; clearTimer(id); } };
  globalThis.console=freeze(fromEntries(['log','info','warn','error','debug'].map(level=>[level,(...args)=>send('log',{level,message:args.map(x=>String(x).slice(0,1024)).join(' ').slice(0,4096)})])));
  // UTF-8 helpers only. Native networking remains behind the broker.
  globalThis.TextEncoder=class { encode(value='') { const s=unescape(encodeURIComponent(String(value))); return Uint8Array.from(s,c=>c.charCodeAt(0)); } };
  globalThis.TextDecoder=class { decode(value=new Uint8Array()) { let s=''; for (const byte of value) s+=String.fromCharCode(byte); return decodeURIComponent(escape(s)); } };
  delete globalThis.__nativeSend; delete globalThis.__nativeNow; delete globalThis.__nativeTimer; delete globalThis.__nativeClearTimer;
})();
