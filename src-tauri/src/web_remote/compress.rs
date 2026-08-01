//! Application-level WebSocket compression + outbound byte accounting for the
//! web-remote transport. Both live here because both belong to exactly one
//! place: the per-connection WS writer task in [`super::server`].
//!
//! ## Why not `permessage-deflate`
//!
//! The standard answer to a chatty WebSocket is the `permessage-deflate`
//! extension negotiated during the upgrade handshake. The server's WS stack
//! (axum → tungstenite) does not implement it, so there is nothing to switch
//! on. Instead the *application* frames its own compressed messages inside
//! ordinary binary WS messages, which needs no support from either WS stack.
//!
//! ## Wire contract (mirrored by the client shim in `src/remote/`)
//!
//! - **Negotiation.** The client appends `compress=deflate` to the `/ws` query
//!   string. With it the server MAY send compressed frames on that connection;
//!   without it it MUST NOT — so a client that doesn't ask sees byte-identical
//!   traffic to before. There is no confirmation frame. C→S traffic is never
//!   compressed (the uplink is keystrokes and small JSON), and Close/Ping/Pong
//!   are never compressed in either direction.
//! - **Wrappers.** `[0x02][u32 BE uncompressed_len][deflate-raw bytes]` wraps a
//!   *text* frame — inflating yields the original UTF-8 JSON bytes.
//!   `[0x03][u32 BE uncompressed_len][deflate-raw bytes]` wraps a *binary*
//!   frame — inflating yields the original bytes, which begin with the existing
//!   `0x01` channel-frame tag. Both ride in binary WS messages; the leading tag
//!   keeps them unambiguous against a plain `0x01` channel frame.
//! - **Context takeover.** ONE raw-deflate stream per connection for the S→C
//!   direction (32 KiB window, level 3). Each compressed message is the encoder
//!   output up to and including a `Z_SYNC_FLUSH` boundary and keeps its
//!   `00 00 FF FF` marker. The shared dictionary is what makes repetitive JSON
//!   frames and PTY redraws collapse over the life of a connection.
//! - **Small frames.** A frame with fewer than [`COMPRESS_MIN_BYTES`] payload
//!   bytes passes through unmodified. That decision is made *before*
//!   compressing, so a skipped frame never advances the context on either side
//!   and the two dictionaries stay in lockstep. Conversely, once a frame has
//!   been compressed it is sent compressed even if the output grew — the
//!   context has already moved and the peer must inflate it.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::ws::Message;
use flate2::{Compress, Compression, FlushCompress};

/// Compressed wrapper around a text frame.
pub const TAG_TEXT: u8 = 0x02;
/// Compressed wrapper around a binary frame.
pub const TAG_BINARY: u8 = 0x03;

/// Frames with a payload smaller than this are sent as-is: below ~64 bytes the
/// deflate block overhead eats the win, and skipping them keeps the context
/// symmetric (see the module note).
pub const COMPRESS_MIN_BYTES: usize = 64;

/// The only compression scheme this server offers. Clients opt in with
/// `?compress=deflate` on the `/ws` URL.
pub const COMPRESS_TOKEN: &str = "deflate";

/// Env var enabling the per-connection bandwidth summary log.
pub const STATS_ENV: &str = "CODEMUX_WEB_REMOTE_STATS";

/// Level 3: most of the ratio of level 6 at a fraction of the CPU, which is the
/// right trade for a stream carrying interactive PTY output.
const LEVEL: u32 = 3;
/// Spare capacity handed to deflate per call when its output buffer fills.
const GROW_BYTES: usize = 4096;
/// How many consecutive zero-progress `compress_vec` calls are tolerated before
/// [`FrameCompressor::deflate_sync`] gives up. Unreachable in practice; the
/// bound only exists so a hypothetical stall is loud instead of an infinite
/// loop in the writer task.
const MAX_DEFLATE_STALLS: u32 = 64;

/// Whether a `compress` query-param value opts this connection into
/// server→client compressed frames. An unrecognised value (or none) leaves the
/// connection uncompressed, so a future client asking for a scheme this build
/// doesn't implement still works — it just gets plain frames.
pub fn negotiated(param: Option<&str>) -> bool {
    param == Some(COMPRESS_TOKEN)
}

/// Whether the opt-in bandwidth summary should be logged on socket close.
pub fn stats_enabled() -> bool {
    std::env::var(STATS_ENV).as_deref() == Ok("1")
}

// ── Frame compressor ────────────────────────────────────────────────

/// One connection's server→client deflate stream. Not `Clone` and not shared:
/// the single writer task owns it, which is what keeps the compressed message
/// order (and therefore the shared dictionary) in step with the peer.
pub struct FrameCompressor {
    deflate: Compress,
}

impl FrameCompressor {
    pub fn new() -> Self {
        // `zlib_header: false` → raw deflate, which is what the client's
        // fflate `Inflate` (see `FrameInflater` in `src/remote/transport.ts`)
        // expects — it is used instead of the platform's async
        // `DecompressionStream` precisely so decoding stays synchronous and
        // frame order can never be reordered by an await. The window is the full
        // 32 KiB: flate2's pure-Rust backend always uses that and only exposes
        // `new_with_window_bits` when built against a C zlib, which would add a
        // native dependency for no wire difference.
        Self {
            deflate: Compress::new(Compression::new(LEVEL), false),
        }
    }

    /// Encode one outbound frame per the module's wire contract. Text and
    /// binary frames at or above [`COMPRESS_MIN_BYTES`] become tagged binary
    /// wrappers; everything else (small frames, Close/Ping/Pong) is returned
    /// untouched.
    pub fn encode(&mut self, msg: Message) -> Message {
        match msg {
            Message::Text(text) if text.len() >= COMPRESS_MIN_BYTES => {
                Message::Binary(self.wrap(TAG_TEXT, text.as_bytes()))
            }
            Message::Binary(bytes) if bytes.len() >= COMPRESS_MIN_BYTES => {
                Message::Binary(self.wrap(TAG_BINARY, &bytes))
            }
            other => other,
        }
    }

    /// Build `[tag][u32 BE uncompressed_len][deflate-raw bytes]`.
    fn wrap(&mut self, tag: u8, input: &[u8]) -> Vec<u8> {
        // Compressed JSON/PTY output typically lands well under half the input;
        // the deflate loop grows the buffer if it doesn't.
        let mut out = Vec::with_capacity(1 + 4 + input.len() / 2 + GROW_BYTES);
        out.push(tag);
        out.extend_from_slice(&(input.len() as u32).to_be_bytes());
        self.deflate_sync(input, &mut out);
        out
    }

    /// Append `input`, compressed, to `out`, ending at a `Z_SYNC_FLUSH`
    /// boundary.
    ///
    /// `compress_vec` only ever writes into the vector's **spare capacity**, so
    /// a call that fills the buffer has neither consumed all the input nor
    /// finished draining the flush — zlib requires calling again, with more
    /// room and the same flush mode. The single success condition is therefore
    /// *both*: every input byte consumed AND the last call left room to spare
    /// (`avail_out > 0`). Stopping at "all input consumed" alone is the classic
    /// bug: it truncates the trailing `00 00 FF FF` sync marker and
    /// desynchronises the peer's inflate context for the rest of the
    /// connection. Returning with input still pending is the same bug wearing a
    /// different hat — it emits a short frame the peer inflates to fewer bytes
    /// than the header declares — so there is no such escape hatch here. A
    /// genuinely stuck encoder (no input taken, no output made, no room gained)
    /// is impossible for raw deflate over in-memory buffers; if it ever happens
    /// we panic, which drops this one connection instead of silently corrupting
    /// it.
    fn deflate_sync(&mut self, input: &[u8], out: &mut Vec<u8>) {
        let mut consumed = 0usize;
        let mut stalls = 0u32;
        loop {
            let capacity_before = out.capacity();
            if out.len() == out.capacity() {
                out.reserve(GROW_BYTES);
            }
            let before_in = self.deflate.total_in();
            let before_out = self.deflate.total_out();
            self.deflate
                .compress_vec(&input[consumed..], out, FlushCompress::Sync)
                // Raw deflate over in-memory buffers has no failure mode short
                // of invalid stream parameters, which are fixed constants here.
                .expect("web-remote deflate over in-memory buffers cannot fail");
            let took = (self.deflate.total_in() - before_in) as usize;
            consumed += took;
            let produced = self.deflate.total_out() - before_out;
            let has_room = out.len() < out.capacity();
            if has_room && consumed == input.len() {
                return;
            }
            if took == 0 && produced == 0 && out.capacity() == capacity_before {
                stalls += 1;
                assert!(
                    stalls < MAX_DEFLATE_STALLS,
                    "web-remote deflate made no progress in {MAX_DEFLATE_STALLS} consecutive \
                     calls with {} of {} input bytes consumed — refusing to emit a truncated \
                     frame that would desynchronise the peer's inflate context",
                    consumed,
                    input.len()
                );
            } else {
                stalls = 0;
            }
        }
    }
}

impl Default for FrameCompressor {
    fn default() -> Self {
        Self::new()
    }
}

// ── Outbound byte accounting ────────────────────────────────────────

/// Per-connection aggregate counters for frames the server sent. Counters
/// only — never any payload content. Read once, on socket close, and only
/// logged when [`STATS_ENV`] is set; the increments themselves are
/// unconditional and cost one relaxed atomic add each.
#[derive(Default)]
pub struct WireStats {
    text_frames: AtomicU64,
    text_bytes: AtomicU64,
    /// Uncompressed `[0x01]` channel frames (PTY output and friends).
    channel_frames: AtomicU64,
    channel_bytes: AtomicU64,
    /// Frames sent as a `0x02`/`0x03` wrapper, counted both ways.
    compressed_frames: AtomicU64,
    compressed_payload_bytes: AtomicU64,
    compressed_wire_bytes: AtomicU64,
}

impl WireStats {
    /// One line summarising everything this socket sent. Percentages are wire
    /// bytes as a fraction of payload bytes for the compressed bucket only.
    pub fn summary(&self) -> String {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let payload = load(&self.compressed_payload_bytes);
        let wire = load(&self.compressed_wire_bytes);
        let ratio = if payload == 0 {
            0.0
        } else {
            (wire as f64 / payload as f64) * 100.0
        };
        format!(
            "text {} frames {}B | channel-binary {} frames {}B | compressed {} frames {}B→{}B ({ratio:.1}%)",
            load(&self.text_frames),
            load(&self.text_bytes),
            load(&self.channel_frames),
            load(&self.channel_bytes),
            load(&self.compressed_frames),
            payload,
            wire,
        )
    }
}

/// Encode one outbound frame for the wire and account for it.
///
/// `compressor` is `None` when the client did not negotiate compression, in
/// which case the frame passes through untouched — but is still counted, so the
/// stats give a like-for-like baseline against a compressed connection.
pub fn encode_and_account(
    compressor: Option<&mut FrameCompressor>,
    stats: &WireStats,
    msg: Message,
) -> Message {
    let payload = frame_len(&msg);
    let was_text = matches!(msg, Message::Text(_));
    let encoded = match compressor {
        Some(c) => c.encode(msg),
        None => msg,
    };
    let wire = frame_len(&encoded);
    let bump = |counter: &AtomicU64, by: u64| {
        counter.fetch_add(by, Ordering::Relaxed);
    };
    if is_compressed(&encoded) {
        bump(&stats.compressed_frames, 1);
        bump(&stats.compressed_payload_bytes, payload as u64);
        bump(&stats.compressed_wire_bytes, wire as u64);
    } else if was_text {
        bump(&stats.text_frames, 1);
        bump(&stats.text_bytes, wire as u64);
    } else if matches!(encoded, Message::Binary(_)) {
        bump(&stats.channel_frames, 1);
        bump(&stats.channel_bytes, wire as u64);
    }
    encoded
}

/// Payload byte length of a data frame; control frames count as zero so they
/// stay out of the accounting entirely.
fn frame_len(msg: &Message) -> usize {
    match msg {
        Message::Text(s) => s.len(),
        Message::Binary(b) => b.len(),
        _ => 0,
    }
}

/// Whether a frame is one of our compressed wrappers.
fn is_compressed(msg: &Message) -> bool {
    match msg {
        Message::Binary(b) => matches!(b.first(), Some(&TAG_TEXT) | Some(&TAG_BINARY)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Decompress, FlushDecompress};

    /// A streaming raw-deflate inflater mirroring the client's: one context for
    /// the whole connection, fed each compressed message in order.
    struct Inflater {
        inflate: Decompress,
    }

    impl Inflater {
        fn new() -> Self {
            Self {
                inflate: Decompress::new(false),
            }
        }

        /// Unwrap one `0x02`/`0x03` frame back to its original bytes, asserting
        /// the declared length matches what inflating produced.
        fn feed(&mut self, frame: &[u8]) -> (u8, Vec<u8>) {
            let tag = frame[0];
            let declared = u32::from_be_bytes(frame[1..5].try_into().unwrap()) as usize;
            let body = &frame[5..];
            let mut out = Vec::with_capacity(declared);
            let mut consumed = 0usize;
            while consumed < body.len() || out.len() < declared {
                if out.len() == out.capacity() {
                    out.reserve(declared.max(64));
                }
                let before_in = self.inflate.total_in();
                let before_out = self.inflate.total_out();
                self.inflate
                    .decompress_vec(&body[consumed..], &mut out, FlushDecompress::Sync)
                    .expect("inflate");
                consumed += (self.inflate.total_in() - before_in) as usize;
                // A frame declaring more bytes than it actually inflates to
                // would otherwise spin here forever on empty input. Bail out and
                // let the length assertion below name the real problem.
                if self.inflate.total_in() == before_in && self.inflate.total_out() == before_out {
                    break;
                }
                if out.len() >= declared {
                    break;
                }
            }
            assert_eq!(out.len(), declared, "declared length must match inflated");
            (tag, out)
        }
    }

    fn text_of(len: usize) -> String {
        // Compressible, JSON-ish filler so the ratio is meaningful.
        GOLDEN_UNIT.repeat(len.div_ceil(GOLDEN_UNIT.len()))
    }

    // ── Golden fixture for the TypeScript suite ─────────────────────

    /// The one repeating unit both this module and `src/remote/transport.test.ts`
    /// build their golden payloads from. Compressible, JSON-ish filler.
    const GOLDEN_UNIT: &str =
        r#"{"t":"event","event":"app-state-changed","payload":{"workspaces":[]}}"#;
    /// Callback id / index carried by the golden `[0x01]` channel frame.
    const GOLDEN_CB: u32 = 77;
    const GOLDEN_IDX: u64 = 9;

    fn golden_text() -> String {
        GOLDEN_UNIT.repeat(3)
    }

    fn golden_pty_payload() -> Vec<u8> {
        b"\x1b[2J\x1b[H$ ls -la\r\ntotal 0\r\n".repeat(4)
    }

    /// `[0x01][u32 BE cb][u64 BE idx][payload]`, the frame a `0x03` wrapper
    /// inflates back to.
    fn golden_channel_frame() -> Vec<u8> {
        let mut frame = vec![0x01u8];
        frame.extend_from_slice(&GOLDEN_CB.to_be_bytes());
        frame.extend_from_slice(&GOLDEN_IDX.to_be_bytes());
        frame.extend_from_slice(&golden_pty_payload());
        frame
    }

    fn golden_large_text() -> String {
        GOLDEN_UNIT.repeat(1450)
    }

    /// The four frames, in order, of one connection's compressed stream.
    fn golden_frames() -> Vec<Vec<u8>> {
        let mut enc = FrameCompressor::new();
        let wrap = |enc: &mut FrameCompressor, msg: Message| match enc.encode(msg) {
            Message::Binary(b) => b,
            other => panic!("golden frames must all be wrappers, got {other:?}"),
        };
        vec![
            wrap(&mut enc, Message::Text(golden_text())),
            // Same text again: almost entirely back-references into frame 1.
            wrap(&mut enc, Message::Text(golden_text())),
            wrap(&mut enc, Message::Binary(golden_channel_frame())),
            wrap(&mut enc, Message::Text(golden_large_text())),
        ]
    }

    /// Regenerate the `GOLDEN_FRAMES` constants in
    /// `src/remote/transport.test.ts`, which pin the real flate2/miniz_oxide
    /// encoder against the client's fflate inflater (the only test that puts the
    /// two production implementations on the same stream).
    ///
    /// ```sh
    /// cargo test --manifest-path src-tauri/Cargo.toml --lib \
    ///   web_remote::compress::tests::golden_frames_for_the_client_suite \
    ///   -- --ignored --nocapture
    /// ```
    ///
    /// Kept `#[ignore]`d: it asserts nothing, it only prints. The payload
    /// builders above are mirrored literally in the TS test, so a change here
    /// means a change there.
    #[test]
    #[ignore = "fixture generator; run with --ignored --nocapture to print base64"]
    fn golden_frames_for_the_client_suite() {
        use base64::Engine as _;
        for (i, frame) in golden_frames().iter().enumerate() {
            println!(
                "frame {i} (tag {:#04x}, {}B on the wire):\n{}",
                frame[0],
                frame.len(),
                base64::engine::general_purpose::STANDARD.encode(frame)
            );
        }
    }

    #[test]
    fn golden_frames_round_trip_through_one_context() {
        // Guards the fixture itself: whatever the generator prints must inflate,
        // in order, through a single client-side context. If this fails the
        // embedded TS constants are stale — regenerate them.
        let frames = golden_frames();
        let expected: Vec<(u8, Vec<u8>)> = vec![
            (TAG_TEXT, golden_text().into_bytes()),
            (TAG_TEXT, golden_text().into_bytes()),
            (TAG_BINARY, golden_channel_frame()),
            (TAG_TEXT, golden_large_text().into_bytes()),
        ];
        assert!(
            frames[1].len() < frames[0].len() / 2,
            "the repeated text must collapse against the shared dictionary ({}B → {}B)",
            frames[0].len(),
            frames[1].len()
        );
        let mut inflater = Inflater::new();
        for (frame, (tag, want)) in frames.iter().zip(expected) {
            assert_eq!(inflater.feed(frame), (tag, want));
        }
    }

    #[test]
    fn compressed_text_frame_header_layout_and_roundtrip() {
        let mut enc = FrameCompressor::new();
        let original = text_of(400);
        let encoded = enc.encode(Message::Text(original.clone()));

        let bytes = match encoded {
            Message::Binary(b) => b,
            other => panic!("expected a binary wrapper, got {other:?}"),
        };
        // Offset-by-offset: tag, then the u32 BE uncompressed length.
        assert_eq!(bytes[0], TAG_TEXT);
        assert_eq!(&bytes[1..5], &(original.len() as u32).to_be_bytes());
        assert!(bytes.len() < original.len(), "repetitive JSON must shrink");
        // The sync-flush marker is part of the message, never stripped.
        assert_eq!(&bytes[bytes.len() - 4..], &[0x00, 0x00, 0xFF, 0xFF]);

        let (tag, inflated) = Inflater::new().feed(&bytes);
        assert_eq!(tag, TAG_TEXT);
        assert_eq!(inflated, original.as_bytes(), "text must round-trip exactly");
    }

    #[test]
    fn compressed_binary_channel_frame_roundtrips_byte_exact() {
        // A real `[0x01][u32 BE cb][u64 BE idx][payload]` channel frame wrapped
        // in a `0x03` message must inflate back to the identical bytes.
        let mut frame = vec![0x01u8];
        frame.extend_from_slice(&7u32.to_be_bytes());
        frame.extend_from_slice(&42u64.to_be_bytes());
        frame.extend_from_slice(b"\x1b[2J\x1b[H$ ls -la\r\ntotal 0\r\n".repeat(8).as_slice());

        let mut enc = FrameCompressor::new();
        let encoded = match enc.encode(Message::Binary(frame.clone())) {
            Message::Binary(b) => b,
            other => panic!("expected a binary wrapper, got {other:?}"),
        };
        assert_eq!(encoded[0], TAG_BINARY);
        assert_eq!(&encoded[1..5], &(frame.len() as u32).to_be_bytes());

        let (tag, inflated) = Inflater::new().feed(&encoded);
        assert_eq!(tag, TAG_BINARY);
        assert_eq!(inflated, frame, "binary frame must round-trip byte-exact");
    }

    #[test]
    fn context_takeover_makes_the_second_message_smaller() {
        // The whole point of one stream per connection: the second copy of a
        // payload costs almost nothing because the first is still in the
        // dictionary. Compare against a fresh stream compressing the same
        // message, which is what a per-message scheme would cost.
        let payload = text_of(600);
        let mut shared = FrameCompressor::new();
        let first = match shared.encode(Message::Text(payload.clone())) {
            Message::Binary(b) => b,
            other => panic!("expected wrapper, got {other:?}"),
        };
        let second = match shared.encode(Message::Text(payload.clone())) {
            Message::Binary(b) => b,
            other => panic!("expected wrapper, got {other:?}"),
        };
        let fresh = match FrameCompressor::new().encode(Message::Text(payload.clone())) {
            Message::Binary(b) => b,
            other => panic!("expected wrapper, got {other:?}"),
        };
        assert!(
            second.len() < fresh.len(),
            "second message ({}B) must beat a fresh stream ({}B) — shared context",
            second.len(),
            fresh.len()
        );

        // …and both still inflate, in order, through one client-side context.
        let mut inflater = Inflater::new();
        assert_eq!(inflater.feed(&first).1, payload.as_bytes());
        assert_eq!(inflater.feed(&second).1, payload.as_bytes());
    }

    #[test]
    fn frames_below_the_threshold_pass_through_unmodified() {
        let mut enc = FrameCompressor::new();
        let small = json_ok_frame();
        assert!(small.len() < COMPRESS_MIN_BYTES);
        match enc.encode(Message::Text(small.clone())) {
            Message::Text(s) => assert_eq!(s, small, "small text must be untouched"),
            other => panic!("expected passthrough text, got {other:?}"),
        }
        let tiny = vec![0x01u8, 0, 0, 0, 1];
        match enc.encode(Message::Binary(tiny.clone())) {
            Message::Binary(b) => assert_eq!(b, tiny, "small binary must be untouched"),
            other => panic!("expected passthrough binary, got {other:?}"),
        }

        // Skipping must not have advanced the context: a following large frame
        // still inflates through a fresh peer context.
        let big = text_of(300);
        let wrapped = match enc.encode(Message::Text(big.clone())) {
            Message::Binary(b) => b,
            other => panic!("expected wrapper, got {other:?}"),
        };
        assert_eq!(Inflater::new().feed(&wrapped).1, big.as_bytes());
    }

    #[test]
    fn control_frames_pass_through() {
        let mut enc = FrameCompressor::new();
        assert!(matches!(enc.encode(Message::Close(None)), Message::Close(None)));
        assert!(matches!(enc.encode(Message::Ping(Vec::new())), Message::Ping(_)));
        assert!(matches!(enc.encode(Message::Pong(Vec::new())), Message::Pong(_)));
    }

    #[test]
    fn large_frame_needs_repeated_deflate_calls_and_still_ends_at_a_flush() {
        // ~1 MiB of low-entropy output: the buffer-growth loop runs many times.
        // If it stopped early the trailing sync marker would be missing and the
        // inflate below would not produce the declared length.
        let payload = text_of(1024 * 1024);
        let mut enc = FrameCompressor::new();
        let encoded = match enc.encode(Message::Text(payload.clone())) {
            Message::Binary(b) => b,
            other => panic!("expected wrapper, got {other:?}"),
        };
        assert_eq!(&encoded[encoded.len() - 4..], &[0x00, 0x00, 0xFF, 0xFF]);
        assert_eq!(Inflater::new().feed(&encoded).1, payload.as_bytes());
    }

    fn json_ok_frame() -> String {
        r#"{"t":"ok","id":1,"data":null}"#.to_string()
    }

    // ── Negotiation ─────────────────────────────────────────────────

    #[test]
    fn only_the_deflate_token_negotiates_compression() {
        assert!(negotiated(Some("deflate")));
        assert!(!negotiated(None), "absent param must leave the socket plain");
        assert!(!negotiated(Some("")));
        assert!(!negotiated(Some("gzip")));
        assert!(!negotiated(Some("Deflate")), "token is case-sensitive");
        assert!(!negotiated(Some("deflate,gzip")));
    }

    // ── Stats ───────────────────────────────────────────────────────

    #[test]
    fn accounting_splits_text_channel_and_compressed_buckets() {
        let stats = WireStats::default();
        let mut enc = FrameCompressor::new();

        // Uncompressed connection: text + channel-binary buckets only.
        encode_and_account(None, &stats, Message::Text(text_of(200)));
        encode_and_account(None, &stats, Message::Binary(vec![0x01; 200]));
        encode_and_account(None, &stats, Message::Ping(Vec::new()));
        assert_eq!(stats.text_frames.load(Ordering::Relaxed), 1);
        assert_eq!(stats.channel_frames.load(Ordering::Relaxed), 1);
        assert_eq!(stats.channel_bytes.load(Ordering::Relaxed), 200);
        assert_eq!(stats.compressed_frames.load(Ordering::Relaxed), 0);

        // Compressed connection: the wrapper lands in the compressed bucket
        // with both the payload and the (smaller) wire size recorded.
        let big = text_of(400);
        encode_and_account(Some(&mut enc), &stats, Message::Text(big.clone()));
        assert_eq!(stats.compressed_frames.load(Ordering::Relaxed), 1);
        assert_eq!(
            stats.compressed_payload_bytes.load(Ordering::Relaxed),
            big.len() as u64
        );
        assert!(
            stats.compressed_wire_bytes.load(Ordering::Relaxed) < big.len() as u64,
            "wire bytes should be below payload bytes"
        );
        // A sub-threshold frame on a compressed connection still counts as text.
        encode_and_account(Some(&mut enc), &stats, Message::Text(json_ok_frame()));
        assert_eq!(stats.text_frames.load(Ordering::Relaxed), 2);

        assert!(stats.summary().contains("compressed 1 frames"));
    }
}
