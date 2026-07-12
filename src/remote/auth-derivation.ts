/**
 * Client-side Codemux account credential derivation.
 *
 * A byte-for-byte mirror of the shared `codemux-api-*` zero-knowledge protocol
 * implemented in Rust at `src-tauri/src/auth/derivation.rs` (and in Vexis).
 * Given the user's raw password + email it produces the **AuthSecret** — the
 * stretched, base64 value Codemux sends to Better Auth in place of the raw
 * password. The raw password is stretched locally and never leaves the browser.
 *
 * Web-remote account mode (Stage A) uses this so a browser can prove it owns
 * the desktop's Codemux account without the raw password ever crossing the
 * wire: the browser derives the AuthSecret here and POSTs `{email, auth_secret}`
 * to the desktop's own `POST /api/pair-account`, which forwards it to
 * `api.codemux.org` server-side (the browser can't call the auth API directly —
 * its origin isn't on the fixed CORS allowlist).
 *
 * The derivation is:
 *
 *   salt        = SHA256("codemux-api-master-v1\0" || normalize_email(email))
 *   master      = Argon2id(password, salt, m=64MiB, t=3, p=4, L=32, v=0x13)
 *   auth_secret = base64_no_pad( HKDF-Expand-SHA256(master, "codemux-api-auth-secret-v1", 32) )
 *
 * Every constant below MUST stay identical to the Rust peer; the golden-value
 * test (`auth-derivation.test.ts`) pins the output so any drift fails CI. Note
 * that `HKDF-Expand` here takes `master` directly as the PRK (no extract step),
 * matching Rust's `Hkdf::from_prk(master).expand(...)`.
 */

import { argon2idAsync } from "@noble/hashes/argon2";
import { expand } from "@noble/hashes/hkdf";
import { sha256 } from "@noble/hashes/sha2";

const encoder = new TextEncoder();

// Domain-separation prefix for the Argon2id salt. The trailing NUL is
// load-bearing — it matches Rust's `b"codemux-api-master-v1\0"` byte-for-byte.
const MASTER_SALT_DOMAIN = "codemux-api-master-v1\0";
// HKDF `info` label for the server-visible auth secret.
const AUTH_SECRET_INFO = "codemux-api-auth-secret-v1";

// Argon2id parameters — must equal the Rust `ARGON2_*` constants.
const ARGON2_M_COST_KIB = 65_536; // 64 MiB
const ARGON2_T_COST = 3;
const ARGON2_P_COST = 4;
const ARGON2_VERSION = 0x13; // v19 (RFC 9106)
const AUTH_SECRET_LEN = 32;

/**
 * Canonicalize an email for use as a derivation salt input: trim surrounding
 * whitespace and lowercase the whole address, so `"  Foo@Bar.COM  "` and
 * `"foo@bar.com"` derive identical credentials. Matches Rust `normalize_email`.
 */
export function normalizeEmail(email: string): string {
  return email.trim().toLowerCase();
}

/** Standard-alphabet base64 with the trailing `=` padding stripped, matching
 *  Rust's `base64::engine::general_purpose::STANDARD_NO_PAD`. */
function base64StandardNoPad(bytes: Uint8Array): string {
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/=+$/, "");
}

/**
 * Derive the server-visible AuthSecret from `(password, email)`. Async so the
 * ~300–500ms Argon2id stretch yields to the event loop instead of freezing the
 * login UI. Deterministic: the same inputs always produce the same output, here
 * and in every other `codemux-api` client.
 */
export async function deriveAuthSecret(
  password: string,
  email: string,
): Promise<string> {
  const domain = encoder.encode(MASTER_SALT_DOMAIN);
  const emailBytes = encoder.encode(normalizeEmail(email));
  const saltInput = new Uint8Array(domain.length + emailBytes.length);
  saltInput.set(domain, 0);
  saltInput.set(emailBytes, domain.length);
  const salt = sha256(saltInput);

  const master = await argon2idAsync(encoder.encode(password), salt, {
    t: ARGON2_T_COST,
    m: ARGON2_M_COST_KIB,
    p: ARGON2_P_COST,
    dkLen: AUTH_SECRET_LEN,
    version: ARGON2_VERSION,
  });

  const authSecret = expand(
    sha256,
    master,
    encoder.encode(AUTH_SECRET_INFO),
    AUTH_SECRET_LEN,
  );
  return base64StandardNoPad(authSecret);
}
