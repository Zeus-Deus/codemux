/**
 * A platform plugin view is one HTML document served by the plugin host,
 * shown in a sandboxed frame with an opaque origin (`allow-scripts` only). It
 * holds no credentials and reaches Codemux only through this bridge, which can
 * call the view's own `view:<viewId>:<name>` handlers on the installation
 * generation that served it, and ask for local intents on this client.
 *
 * Handshake: the bridge script (first in the document, before author code)
 * makes a MessageChannel and hands the host one port with a per-document
 * token. Nothing is ever posted to the frame's window, so a page the frame
 * navigates to reads neither pending answers nor context.
 */
import type { PluginClientIntent, PluginConversationCard, PluginFailure, PluginInvokeResult } from "./contract";
import { decodePluginIntent, pluginUnsupportedIntentCount } from "./intents";
import { isPluginViewHandlerName, pluginFailure } from "./model";

export const PLUGIN_VIEW_BRIDGE_VERSION = 1;
const MAX_MESSAGE_BYTES = 256 * 1024;
const MAX_PENDING_REQUESTS = 8;
const encoder = new TextEncoder();

/**
 * What this client's bridge adds beyond `ready`, `context`, `invoke` and
 * `onContext`. A view checks `codemux.features` and keeps a fallback.
 */
export type PluginViewBridgeFeature = "changes" | "local-intents" | "resize" | "card-context";

export interface PluginViewContext {
  readonly theme: "light" | "dark";
  /** CSS custom properties applied to the document's root element. */
  readonly styles: Readonly<Record<string, string>>;
  readonly platform: "web" | "desktop" | "mobile";
  readonly placement: "page" | "side-panel" | "conversation";
  readonly view: { readonly installationId: string; readonly viewId: string };
  readonly thread: { readonly threadId: string; readonly projectId: string | null } | null;
  readonly features?: ReadonlyArray<PluginViewBridgeFeature>;
  /** The card this conversation view renders, as recorded. Presentation only: it authorizes nothing. */
  readonly card?: PluginConversationCard;
}

/** Local intents need only this client; change signals need the host's; cards size to content. */
export function pluginViewBridgeFeatures(input: {
  readonly placement: PluginViewContext["placement"];
  readonly changes: boolean;
}): ReadonlyArray<PluginViewBridgeFeature> {
  return [
    ...(input.changes ? (["changes"] as const) : []),
    "local-intents",
    ...(input.placement === "conversation" ? (["resize", "card-context"] as const) : []),
  ];
}

/** Tallest a card may ask to be; taller content scrolls inside it. */
export const MAX_PLUGIN_CARD_HEIGHT = 720;

export interface PluginViewHostOptions<Origin> {
  /** Random per document, baked into its bridge. */
  readonly token: string;
  readonly post: (message: unknown) => void;
  readonly context: () => PluginViewContext;
  /** Where the user is when the view asks; intents in the answer are judged against it. */
  readonly captureOrigin: () => Origin;
  /** Calls `view:<viewId>:<name>`; the client fixes installation, generation and view. */
  readonly invoke: (name: string, input: unknown, origin: Origin) => Promise<PluginInvokeResult>;
  /** Intents go to the client that shows the view, never back into the frame. */
  readonly onIntents: (intents: ReadonlyArray<PluginClientIntent>, origin: Origin, unsupported: number) => void;
  /** Must match the list baked into the document by `buildPluginViewDocument`. */
  readonly features?: ReadonlyArray<PluginViewBridgeFeature>;
  /** The height the view asked for, clamped; only with the `resize` feature. */
  readonly onResize?: (height: number) => void;
}

export interface PluginViewHost {
  /** One message from the frame's port. */
  readonly receive: (data: unknown) => void;
  readonly sendContext: () => void;
  /** Tells the view a topic it may care about changed; it decides whether to fetch again. */
  readonly changed: (topic: string) => void;
  readonly dispose: () => void;
}

const jsonBytes = (value: unknown): number => {
  try {
    return encoder.encode(JSON.stringify(value) ?? "").byteLength;
  } catch {
    return Number.POSITIVE_INFINITY;
  }
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const isRequestId = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value);

export const pluginViewFailure = (error: unknown): PluginFailure => pluginFailure(error);

export function makePluginViewHost<Origin>(options: PluginViewHostOptions<Origin>): PluginViewHost {
  let disposed = false;
  let pending = 0;
  const features = new Set(options.features ?? []);
  const post = (message: Record<string, unknown>) => {
    if (!disposed) options.post({ codemuxPlugin: PLUGIN_VIEW_BRIDGE_VERSION, ...message });
  };
  const fail = (id: number, reason: string, message: string) =>
    post({ kind: "result", id, ok: false, error: { reason, message } });

  return {
    receive: (data) => {
      if (
        disposed ||
        !isRecord(data) ||
        data.codemuxPlugin !== PLUGIN_VIEW_BRIDGE_VERSION ||
        data.token !== options.token
      ) {
        return;
      }
      if (data.kind === "connect") return;
      if (data.kind === "ready") {
        post({ kind: "context", context: options.context() });
        return;
      }
      if (data.kind === "resize") {
        if (!features.has("resize") || typeof data.height !== "number" || !Number.isFinite(data.height)) return;
        options.onResize?.(Math.min(Math.max(Math.ceil(data.height), 0), MAX_PLUGIN_CARD_HEIGHT));
        return;
      }
      if (data.kind === "intent" && isRequestId(data.id)) {
        if (!features.has("local-intents")) {
          fail(data.id, "unsupported", "This app does not take requests from views directly.");
          return;
        }
        if (jsonBytes(data) > MAX_MESSAGE_BYTES) {
          fail(data.id, "invalid_input", "Request too large.");
          return;
        }
        const intent = decodePluginIntent(data.intent);
        if (intent === null) {
          fail(data.id, "unsupported", "This app cannot do that request.");
          return;
        }
        // Judged like a result's intents, against where the user is right now.
        options.onIntents([intent], options.captureOrigin(), 0);
        post({ kind: "result", id: data.id, ok: true, value: {} });
        return;
      }
      if (data.kind !== "invoke" || !isRequestId(data.id)) return;
      const id = data.id;
      if (jsonBytes(data) > MAX_MESSAGE_BYTES) {
        fail(id, "invalid_input", "Request too large.");
        return;
      }
      if (!isPluginViewHandlerName(data.name)) {
        fail(id, "invalid_handler", "Handler names use letters, digits, '.', '_' and '-'.");
        return;
      }
      if (pending >= MAX_PENDING_REQUESTS) {
        fail(id, "busy", "Too many requests in flight.");
        return;
      }
      pending += 1;
      const origin = options.captureOrigin();
      void options.invoke(data.name, data.input, origin).then(
        (result) => {
          pending -= 1;
          if (disposed) return;
          const intents = result.intents ?? [];
          const unsupported = pluginUnsupportedIntentCount(result);
          if (intents.length > 0 || unsupported > 0) options.onIntents(intents, origin, unsupported);
          post({
            kind: "result",
            id,
            ok: true,
            value: {
              ...(result.message === undefined ? {} : { message: result.message }),
              ...(result.data === undefined ? {} : { data: result.data }),
              ...(result.images === undefined ? {} : { images: result.images }),
            },
          });
        },
        (error: unknown) => {
          pending -= 1;
          const failure = pluginViewFailure(error);
          fail(id, failure.reason, failure.message);
        },
      );
    },
    sendContext: () => post({ kind: "context", context: options.context() }),
    changed: (topic) => {
      if (features.has("changes")) post({ kind: "changed", topic });
    },
    dispose: () => {
      disposed = true;
    },
  };
}

/**
 * The view may load nothing from the network: data comes through the bridge,
 * and the plugin's backend owns any outside calls. Combined with the frame
 * sandbox (scripts only: no same-origin, popups, forms or top navigation).
 */
export const PLUGIN_VIEW_CONTENT_SECURITY_POLICY = [
  "default-src 'none'",
  "script-src 'unsafe-inline'",
  "style-src 'unsafe-inline'",
  "img-src data: blob:",
  "font-src data:",
  "media-src data: blob:",
  "connect-src 'none'",
  "frame-src 'none'",
  "worker-src 'none'",
  "object-src 'none'",
  "form-action 'none'",
  "base-uri 'none'",
].join("; ");

// The author-facing API, in ES5 so it runs before any author code on every
// engine. `window.codemux` is the API; `window.t3` aliases it so views written
// for the same v1 vocabulary run unchanged.
const bridgeScript = (token: string, features: ReadonlyArray<PluginViewBridgeFeature>) => `(function(){"use strict";
var token=${JSON.stringify(token).replace(/</g, "\\u003c")},features=Object.freeze(${JSON.stringify(features)}),
channel=new MessageChannel(),port=channel.port1,
pending={},next=1,ctx=null,listeners=[],changeListeners=[],resolveReady,ready=new Promise(function(r){resolveReady=r;});
function send(m){m.codemuxPlugin=1;m.token=token;port.postMessage(m);}
function has(f){return features.indexOf(f)>=0;}
function apply(c){var root=document.documentElement;root.dataset.codemuxTheme=c.theme;root.style.colorScheme=c.theme;
for(var k in c.styles){if(Object.prototype.hasOwnProperty.call(c.styles,k))root.style.setProperty(k,c.styles[k]);}}
function request(m){return new Promise(function(resolve,reject){var id=next++;pending[id]=[resolve,reject];m.id=id;
try{send(m);}catch(e){delete pending[id];reject(e);}});}
function listen(list,f){list.push(f);return function(){var i=list.indexOf(f);if(i>=0)list.splice(i,1);};}
port.onmessage=function(e){var m=e.data;if(!m||m.codemuxPlugin!==1)return;
if(m.kind==="context"){ctx=Object.freeze(m.context);apply(ctx);resolveReady(ctx);
listeners.slice().forEach(function(f){try{f(ctx);}catch(_){}});return;}
if(m.kind==="changed"){var c=Object.freeze({topic:String(m.topic)});
changeListeners.slice().forEach(function(f){try{f(c);}catch(_){}});return;}
if(m.kind==="result"){var p=pending[m.id];if(!p)return;delete pending[m.id];
if(m.ok)p[0](m.value);else{var err=new Error(m.error&&m.error.message||"Request failed.");err.reason=m.error&&m.error.reason;p[1](err);}}};
var api={apiVersion:1,ready:ready,features:features,
get context(){return ctx;},
invoke:function(name,input){return request({kind:"invoke",name:String(name),input:input});},
onContext:function(f){return listen(listeners,f);}};
if(has("changes"))api.onChanged=function(f){return listen(changeListeners,f);};
if(has("local-intents"))api.intent=function(intent){return request({kind:"intent",intent:intent}).then(function(){});};
if(has("resize")){api.resize=function(height){send({kind:"resize",height:Number(height)});};
if(typeof ResizeObserver==="function")ready.then(function(){var last=-1;new ResizeObserver(function(){
var h=Math.ceil(document.documentElement.getBoundingClientRect().height);if(h!==last){last=h;api.resize(h);}}).observe(document.documentElement);});}
var frozen=Object.freeze(api);
Object.defineProperty(window,"codemux",{value:frozen,writable:false,configurable:false});
Object.defineProperty(window,"t3",{value:frozen,writable:false,configurable:false});
window.parent.postMessage({codemuxPlugin:1,token:token,kind:"connect"},"*",[channel.port2]);
send({kind:"ready"});})();`;

/**
 * Puts the policy and bridge ahead of everything the author wrote, without
 * looking for the author's `<head>`: one hidden in a comment or after body
 * content would leave the policy where the browser ignores it.
 */
export function buildPluginViewDocument(
  html: string,
  token: string,
  features: ReadonlyArray<PluginViewBridgeFeature> = [],
): string {
  const preamble = `<meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${PLUGIN_VIEW_CONTENT_SECURITY_POLICY}"><script>${bridgeScript(token, features)}</script>`;
  const body = html.replace(/^﻿?\s*<!doctype[^>]*>/i, "");
  return `<!doctype html><html><head>${preamble}</head>${body}`;
}

/** The port a view's bridge hands over, if `data` is that hand-over for this document. */
export function acceptPluginViewPort(
  data: unknown,
  ports: ReadonlyArray<MessagePort>,
  token: string,
): MessagePort | null {
  return isRecord(data) &&
    data.codemuxPlugin === PLUGIN_VIEW_BRIDGE_VERSION &&
    data.kind === "connect" &&
    data.token === token &&
    ports.length === 1
    ? ports[0]!
    : null;
}

/**
 * Talks to one view document over the port its bridge created. Only a
 * hand-over from the frame's own window with this document's token is
 * accepted, and only the first one.
 */
export function bindPluginViewChannel(input: {
  readonly token: string;
  readonly source: () => Window | null;
  readonly receive: (data: unknown) => void;
}) {
  let port: MessagePort | null = null;
  const onWindowMessage = (event: MessageEvent) => {
    if (port !== null || event.source === null || event.source !== input.source()) return;
    const accepted = acceptPluginViewPort(event.data, event.ports, input.token);
    if (accepted === null) return;
    port = accepted;
    port.addEventListener("message", (message) => input.receive(message.data));
    port.start();
  };
  window.addEventListener("message", onWindowMessage);
  return {
    post: (message: unknown) => port?.postMessage(message),
    dispose: () => {
      window.removeEventListener("message", onWindowMessage);
      port?.close();
      port = null;
    },
  };
}

/** A random per-document token. */
export const newPluginViewToken = () =>
  Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) => byte.toString(16).padStart(2, "0")).join("");
