import '@remote-dom/core/polyfill';
import { RemoteElement, RemoteRootElement } from '@remote-dom/core/elements';
import { createRemoteComponent } from '@remote-dom/preact';
import { h, render, options } from 'preact';
import { useState } from 'preact/hooks';
options.debounceRendering = (fn) => Promise.resolve().then(fn);
class Text extends RemoteElement {}
class Button extends RemoteElement { static remoteEvents = ['press']; }
customElements.define('cmx-text', Text);
customElements.define('cmx-button', Button);
customElements.define('remote-root', RemoteRootElement);
const CButton = createRemoteComponent('cmx-button', Button, { eventProps: { onPress: { event: 'press' } } });
let callbackSequence = 0;
const callbacks = new Map();
function serialize(value) { return JSON.stringify(value, (key, item) => { if (typeof item !== 'function') return item; const id = String(++callbackSequence); callbacks.set(id, item); return { callbackId: id }; }); }
const root = document.createElement('remote-root');
root.connect({ mutate(records) { __sendJson(serialize({ type: 'ui', records })); }, call() { throw new Error('No unrestricted host methods'); } });
function Brief() {
  const [count, setCount] = useState(0);
  return h('cmx-text', null, 'Brief for demo · clicks: ' + count, h(CButton, { onPress() { setCount(count + 1); __sendJson(JSON.stringify({type:'host.request',method:'composer.appendText',params:{text:'Branch: demo'}})); } }, 'Add to draft'));
}
render(h(Brief), root);
globalThis.__dispatchProbeEvent = () => { const callback = callbacks.values().next().value; if (!callback) throw new Error('No event callback registered'); callback(null); };
__sendJson(JSON.stringify({type:'globals',process:typeof globalThis.process,require:typeof globalThis.require,fetch:typeof globalThis.fetch,tauri:typeof globalThis.__TAURI_INTERNALS__}));
