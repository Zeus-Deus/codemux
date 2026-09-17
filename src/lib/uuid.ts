/**
 * Secure-context-independent UUID v4 minting.
 *
 * Why this exists: `crypto.randomUUID()` is only exposed in secure
 * contexts (https, or localhost). The remote web client is served over
 * plain `http://<lan-ip>:<port>`, which is *not* a secure context, so
 * `crypto.randomUUID` is `undefined` there and every bare call site
 * throws `TypeError: crypto.randomUUID is not a function` — killing the
 * click handler it ran in. `crypto.getRandomValues()` has no such
 * restriction, so we derive the same value from it instead.
 *
 * The output must stay a canonical RFC 4122 v4 string: the backend
 * parses some of these ids as real UUIDs, so an ad-hoc token like
 * `id-${Date.now()}` would be rejected further down the line.
 */

const HEX: string[] = Array.from({ length: 256 }, (_, i) =>
  i.toString(16).padStart(2, "0"),
);

/** Fill `bytes` with random data, preferring the Web Crypto RNG.
 *  `Math.random` is a last-resort fallback for environments that expose
 *  neither `randomUUID` nor `getRandomValues`; the id shape is identical
 *  either way, only the entropy quality differs. */
function fillRandomBytes(bytes: Uint8Array): void {
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    crypto.getRandomValues(bytes);
    return;
  }
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = Math.floor(Math.random() * 256);
  }
}

/**
 * Returns a canonical RFC 4122 version-4 UUID.
 *
 * Uses the native `crypto.randomUUID()` when the platform exposes it,
 * and otherwise builds the same string from 16 random bytes with the
 * version (`4`) and variant (`10xx`) bits set by hand.
 */
export function randomUUID(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }

  const bytes = new Uint8Array(16);
  fillRandomBytes(bytes);
  // Version 4 in the high nibble of byte 6, variant 10xx in byte 8.
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;

  const h = HEX;
  return (
    h[bytes[0]] + h[bytes[1]] + h[bytes[2]] + h[bytes[3]] +
    "-" +
    h[bytes[4]] + h[bytes[5]] +
    "-" +
    h[bytes[6]] + h[bytes[7]] +
    "-" +
    h[bytes[8]] + h[bytes[9]] +
    "-" +
    h[bytes[10]] + h[bytes[11]] + h[bytes[12]] + h[bytes[13]] + h[bytes[14]] + h[bytes[15]]
  );
}
