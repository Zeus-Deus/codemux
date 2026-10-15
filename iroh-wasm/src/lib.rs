//! Browser (WASM) iroh client for the Codemux web-remote **relay** transport.
//!
//! The desktop half lives in `src-tauri/src/web_remote/iroh.rs`: it binds an
//! iroh endpoint and accepts bi-streams carrying the `/ws` frame contract. This
//! crate is the browser half — compiled to WebAssembly and lazily loaded by
//! `src/remote/iroh-wasm-loader.ts` on the hosted origin — that dials a
//! desktop's `node_id` over iroh (hole-punched, or relay-forwarded ciphertext)
//! and exposes the opened QUIC bi-stream as a raw byte pipe.
//!
//! Deliberately thin: it does **not** know the kind-tagged codec or the
//! `hello-account` handshake. Those live in TypeScript (`iroh-codec.ts` /
//! `iroh-transport.ts`) so they are unit-testable without a browser, and so this
//! crate's only job — and only place it can go wrong — is moving bytes over
//! QUIC. The JS boundary is three async calls: [`connect`], then
//! [`IrohStream::write`] / [`IrohStream::read`] / [`IrohStream::close`].
//!
//! The ALPN (`codemux/web-remote/0`) MUST match the desktop's `IROH_ALPN`.

use std::rc::Rc;
use std::str::FromStr;

use futures_util::lock::Mutex;
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, TransportAddr};
use tokio::io::AsyncWriteExt;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

/// ALPN identifying this protocol on the QUIC handshake — must equal the
/// desktop's `IROH_ALPN` in `src-tauri/src/web_remote/iroh.rs`.
const IROH_ALPN: &[u8] = b"codemux/web-remote/0";

/// Read window per `read()` call.
const READ_CHUNK: usize = 64 * 1024;

#[wasm_bindgen(start)]
pub fn __start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// An open iroh bi-stream to a desktop endpoint: a raw, ordered byte pipe. The
/// endpoint + connection are held here so they outlive the stream (dropping
/// either would tear the QUIC connection down).
#[wasm_bindgen]
pub struct IrohStream {
    _endpoint: Endpoint,
    conn: Connection,
    // `Rc<Mutex<…>>` (async mutex) so the wasm-bindgen methods can take `&self`,
    // clone the handle into the returned future, and hold the guard across
    // `.await` without borrowing `self` across it.
    send: Rc<Mutex<SendStream>>,
    recv: Rc<Mutex<RecvStream>>,
}

/// Dial `node_id` over iroh and open a bi-stream. `relay_url` is an optional
/// hint that speeds up the first connection (the browser has no UDP, so a relay
/// is always in play until/unless a direct path is found).
#[wasm_bindgen]
pub async fn connect(
    node_id: String,
    relay_url: Option<String>,
) -> Result<IrohStream, JsValue> {
    let relays = browser_relay_urls();
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(RelayMap::from_iter(relays.clone())))
        .alpns(vec![IROH_ALPN.to_vec()])
        .bind()
        .await
        .map_err(js_err)?;

    let id = EndpointId::from_str(node_id.trim()).map_err(js_err)?;
    // Address lookup reports the desktop's home relay with the trailing dot
    // WebKit can't use, so also offer every default relay without it: a relay
    // forwards by endpoint id, so whichever one the desktop is homed on works.
    let mut addr = EndpointAddr::new(id).with_addrs(relays.into_iter().map(TransportAddr::Relay));
    if let Some(url) = relay_url {
        let url = url.trim();
        if !url.is_empty() {
            let url = RelayUrl::from_str(url).map_err(js_err)?;
            addr = addr.with_relay_url(without_trailing_dot(&url));
        }
    }

    let conn = endpoint.connect(addr, IROH_ALPN).await.map_err(js_err)?;
    let (send, recv) = conn.open_bi().await.map_err(js_err)?;

    Ok(IrohStream {
        _endpoint: endpoint,
        conn,
        send: Rc::new(Mutex::new(send)),
        recv: Rc::new(Mutex::new(recv)),
    })
}

#[wasm_bindgen]
impl IrohStream {
    /// Write all `data` bytes to the QUIC send stream (then flush). Returns a
    /// `Promise<void>`.
    pub fn write(&self, data: Vec<u8>) -> js_sys::Promise {
        let send = self.send.clone();
        future_to_promise(async move {
            let mut s = send.lock().await;
            s.write_all(&data).await.map_err(js_err)?;
            s.flush().await.map_err(js_err)?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Read the next chunk. Resolves to a `Uint8Array`, or `null` at
    /// end-of-stream. Returns a `Promise<Uint8Array | null>`.
    pub fn read(&self) -> js_sys::Promise {
        let recv = self.recv.clone();
        future_to_promise(async move {
            let mut r = recv.lock().await;
            let mut buf = vec![0u8; READ_CHUNK];
            // iroh's `RecvStream::read` is quinn-style: `Ok(None)` at end of
            // stream, `Ok(Some(n))` for n bytes read.
            match r.read(&mut buf).await.map_err(js_err)? {
                None => Ok(JsValue::NULL),
                Some(n) => Ok(js_sys::Uint8Array::from(&buf[..n]).into()),
            }
        })
    }

    /// Close the connection (and thus the stream) immediately.
    pub fn close(&self) {
        self.conn.close(0u32.into(), b"client-closed");
    }
}

/// iroh's default relays, without the trailing dot iroh writes their hostnames
/// with (`euc1-1.relay.n0.iroh.link.`). WebKit — every iOS browser — fails the
/// TLS handshake on a trailing-dot host ("Unacceptable TLS certificate"), so
/// with the stock names the dial hangs forever on Safari.
fn browser_relay_urls() -> Vec<RelayUrl> {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<RelayUrl>>()
        .iter()
        .map(without_trailing_dot)
        .collect()
}

fn without_trailing_dot(url: &RelayUrl) -> RelayUrl {
    let mut stripped = (**url).clone();
    if let Some(host) = url.host_str().and_then(|h| h.strip_suffix('.')) {
        if stripped.set_host(Some(host)).is_err() {
            return url.clone();
        }
    }
    RelayUrl::from(stripped)
}

fn js_err<E: std::fmt::Display>(e: E) -> JsValue {
    JsValue::from_str(&e.to_string())
}
