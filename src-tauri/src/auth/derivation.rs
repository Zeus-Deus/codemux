// Zero-knowledge auth credential derivation. Matches the Bitwarden-
// style client-side password split shared across every product
// authenticating to api.codemux.org.
//
// What this does: takes the user's raw password + email and produces
// two halves derived via Argon2id + HKDF-SHA256:
//   * `AuthSecret` — a deterministic, high-entropy, base64-encoded
//     32-byte value Codemux sends to Better Auth in place of the raw
//     password, so the server only ever bcrypt-hashes this stretched
//     value and never sees the user's actual password.
//   * `EncryptionKey` — a 32-byte raw symmetric key used by E2E
//     features (Step 10 skills sync). Never sent to the server,
//     never persisted in plaintext on disk, never logged.
//
// Multi-device / cross-product contract: as long as every client
// (Codemux, Vexis, any future app in the ecosystem) uses these exact
// same constants and algorithm, `derive_login_credentials(password,
// email)` produces byte-identical output everywhere. That's how one
// Better Auth account works across multiple products — the server
// compares bcrypt hashes, and every client hands it the same
// pre-hash input. The shared E2E key likewise lets two devices that
// share the same Better Auth account decrypt each other's synced
// state without any key ever crossing the network.
//
// Implementation reference: src-tauri/src/encryption/manager.rs in
// the Vexis repo. Any change to the protocol MUST be made in lockstep
// across every product, bumping the `vN` suffix in the constants
// below and in every peer implementation at the same time. The two
// cross-product hex pins below (`auth_secret_matches_vexis_*` and
// `encryption_key_matches_vexis_for_known_input`) are the canary —
// if either fires in CI, the products have drifted.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

// Argon2id parameters — interactive-class on modern hardware
// (~300-500ms), well above OWASP 2023's floor (m≥19MiB, t≥2). A
// stolen server-side bcrypt hash of AuthSecret still forces a full
// Argon2id derivation per password candidate, so weak passwords are
// dictionary-attackable but slowly; strong passwords are infeasible
// to crack.
const ARGON2_M_COST_KIB: u32 = 65_536; // 64 MiB
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 4;
const AUTH_SECRET_LEN: usize = 32;
const ENCRYPTION_KEY_LEN: usize = 32;

/// Domain-separation prefix for the Argon2id salt. The
/// `codemux-api-` prefix identifies this as a SHARED protocol
/// constant for any client authenticating to `api.codemux.org` —
/// not specific to Codemux specifically. Same byte sequence lives
/// in Vexis's `src-tauri/src/encryption/manager.rs` at
/// `MASTER_SALT_DOMAIN`. Changing this byte-for-byte invalidates
/// every existing auth hash across every product in the ecosystem
/// and forces a global password reset — any rotation MUST be
/// coordinated with every peer implementation.
const MASTER_SALT_DOMAIN: &[u8] = b"codemux-api-master-v1\0";

/// HKDF `info` label for the server-visible auth secret. Shared
/// across every product talking to api.codemux.org — keeping this
/// byte-identical with Vexis (and any future product) is what makes
/// one Better Auth account work across all of them.
const AUTH_SECRET_INFO: &[u8] = b"codemux-api-auth-secret-v1";

/// HKDF `info` label for the client-side encryption key. Shared
/// across every product that ships E2E features against the
/// codemux-api ecosystem (Codemux skills sync, Vexis voice_*).
/// Keeping this byte-identical with Vexis is what lets two devices
/// signed into the same Better Auth account decrypt each other's
/// synced data without ever transmitting the key.
const ENCRYPTION_KEY_INFO: &[u8] = b"codemux-api-encryption-key-v1";

/// A base64-encoded 32-byte authentication secret derived from the
/// user's password + email. This is what Codemux sends to Better
/// Auth in place of the raw password — so the server only ever sees
/// this high-entropy value, never the user's actual password.
///
/// **Why a newtype?** Compile-time enforcement that the auth-API
/// callsites cannot be called with a raw password `String`. The
/// inner string is only constructible inside this module (via
/// `derive_auth_secret`), so the type system catches any attempt to
/// bypass the derivation at build time.
///
/// The `Debug` impl deliberately redacts the value — if this struct
/// ever ends up in a log line, panic message, or crash report, the
/// actual secret won't leak.
#[derive(Clone)]
pub struct AuthSecret(String);

impl AuthSecret {
    /// Borrow the base64 secret for transmission to the auth API.
    /// `pub(crate)` so only code inside the codemux crate can read
    /// it; external callers can't accidentally log or leak it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Wrap an `AuthSecret` that was **already derived on the client** and
    /// arrived over the web-remote account-pairing channel
    /// (`POST /api/pair-account`). The browser runs the identical
    /// `codemux-api-*` derivation ([`derive_auth_secret`]) locally and sends
    /// only the stretched base64 secret — never the raw password — to the
    /// desktop, which forwards it to Better Auth. This is still an `AuthSecret`
    /// in the type sense (a stretched value, never a raw password), so the
    /// typed boundary that keeps raw passwords off the wire is preserved: the
    /// value that reaches [`crate::auth::login_email_api`] is a derived secret
    /// by construction. `pub(crate)` and verbosely named so the (single) call
    /// site stays greppable and no external code can forge one.
    pub(crate) fn from_web_remote_derived(secret: String) -> Self {
        AuthSecret(secret)
    }

    /// Public escape hatch for integration test binaries (e.g. the
    /// `skills_smoke` example) that need to forward the secret to
    /// the live `/api/auth/desktop/signin` endpoint. Verbose name
    /// keeps the call sites greppable.
    pub fn expose_for_external_signin(&self) -> &str {
        &self.0
    }
}

/// A 32-byte raw symmetric key derived alongside `AuthSecret` for
/// products that ship client-side end-to-end encryption (Step 10
/// skills sync, Vexis voice_*). Never sent to the server, never
/// persisted in plaintext on disk, never logged.
///
/// `Drop` zeroes the bytes via `write_volatile` so a stack frame
/// holding a transient `EncryptionKey` doesn't leave key material
/// in memory after it goes out of scope. Not as airtight as the
/// `zeroize` crate but sufficient for our threat model — brief
/// in-stack lifetime, never crosses FFI.
pub struct EncryptionKey([u8; ENCRYPTION_KEY_LEN]);

impl EncryptionKey {
    /// Borrow the raw key bytes for use with an AEAD cipher.
    /// `pub(crate)` so the bytes don't leak past crate boundaries;
    /// the public escape hatch below is for the smoke-test binary.
    #[allow(dead_code)] // used by tests + Stage 2 sync layer (forthcoming)
    pub(crate) fn as_bytes(&self) -> &[u8; ENCRYPTION_KEY_LEN] {
        &self.0
    }

    /// Public escape hatch for the Stage 1 `skills_smoke` example
    /// binary, which needs the raw bytes to feed
    /// `crate::encryption::encrypt`. Verbose name keeps the
    /// callsites greppable.
    pub fn expose_for_smoke_test(&self) -> &[u8; ENCRYPTION_KEY_LEN] {
        &self.0
    }
}

impl Drop for EncryptionKey {
    fn drop(&mut self) {
        for byte in self.0.iter_mut() {
            // Safety: writing to our own slot, no aliasing.
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
    }
}

impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EncryptionKey(***)")
    }
}

impl std::fmt::Debug for AuthSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the actual secret. A stray `println!("{:?}",
        // secret)` or `eprintln!("got: {:?}", …)` would otherwise
        // leak the auth credential into logs.
        write!(f, "AuthSecret(***)")
    }
}

/// Derive the server-visible `AuthSecret` from the user's password +
/// email. **This is the only entry point that ingests the raw
/// password** — every other call site receives an `AuthSecret`
/// (to send to the server).
///
/// The derivation is:
///
/// ```text
/// master      = Argon2id(
///     password,
///     salt   = SHA256("codemux-api-master-v1\0" || normalize_email(email)),
///     m=64MiB, t=3, p=4, L=32,
/// )
/// auth_secret = base64_no_pad( HKDF-Expand(master, "codemux-api-auth-secret-v1", 32) )
/// ```
///
/// **Security properties:**
///   1. The server never sees the raw password — only `auth_secret`,
///      which is indistinguishable from random bytes to anyone
///      without the password.
///   2. Argon2id cost (~300-500ms on modern hardware) forces any
///      attacker with a stolen server-side bcrypt hash to do a full
///      Argon2id derivation per candidate password.
///   3. Deterministic — same `(password, email)` produces identical
///      output on every device and across every product that
///      implements the protocol, enabling one-account-many-products
///      login without ever transmitting a key.
///   4. Email is normalized (lowercase + trim) so capitalization and
///      stray whitespace don't break cross-device/cross-product
///      derivation.
///
/// **Empty password is allowed** (returns Ok with a weak but valid
/// value) so this function never panics on edge inputs. The login
/// UI is responsible for enforcing length/strength policy before
/// calling this.
pub fn derive_auth_secret(password: &str, email: &str) -> Result<AuthSecret, String> {
    let master = derive_master_material(password, email)?;
    let hk = Hkdf::<Sha256>::from_prk(&master)
        .map_err(|e| format!("hkdf from_prk: {e}"))?;
    let mut bytes = [0u8; AUTH_SECRET_LEN];
    hk.expand(AUTH_SECRET_INFO, &mut bytes)
        .map_err(|e| format!("hkdf expand auth: {e}"))?;
    let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes);
    Ok(AuthSecret(encoded))
}

/// Derive both halves of the login credentials from a single
/// Argon2id stretch of `(password, email)`. Mirrors Vexis's
/// `derive_login_credentials` and produces byte-identical output
/// for the same inputs, which is what makes the shared Better Auth
/// account work across both apps and what lets two devices sharing
/// an account decrypt each other's E2E-synced state.
///
/// ```text
/// master         = Argon2id(
///     password,
///     salt   = SHA256("codemux-api-master-v1\0" || normalize_email(email)),
///     m=64MiB, t=3, p=4, L=32,
/// )
/// auth_secret    = base64_no_pad( HKDF-Expand(master, "codemux-api-auth-secret-v1",    32) )
/// encryption_key =                HKDF-Expand(master, "codemux-api-encryption-key-v1", 32)
/// ```
///
/// HKDF domain separation: observing one half tells an attacker
/// nothing about the other. Server-stored AuthSecret cannot be used
/// to derive any user's EncryptionKey.
pub fn derive_login_credentials(
    password: &str,
    email: &str,
) -> Result<(AuthSecret, EncryptionKey), String> {
    let master = derive_master_material(password, email)?;
    let hk = Hkdf::<Sha256>::from_prk(&master)
        .map_err(|e| format!("hkdf from_prk: {e}"))?;

    let mut auth_bytes = [0u8; AUTH_SECRET_LEN];
    hk.expand(AUTH_SECRET_INFO, &mut auth_bytes)
        .map_err(|e| format!("hkdf expand auth: {e}"))?;
    let auth_secret = AuthSecret(
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(auth_bytes),
    );

    let mut key_bytes = [0u8; ENCRYPTION_KEY_LEN];
    hk.expand(ENCRYPTION_KEY_INFO, &mut key_bytes)
        .map_err(|e| format!("hkdf expand encryption-key: {e}"))?;
    let encryption_key = EncryptionKey(key_bytes);

    Ok((auth_secret, encryption_key))
}

/// Expensive Argon2id stretch of `(password, normalize(email))`.
/// Produces 32 bytes of master key material that feeds the HKDF
/// expansion. The salt is SHA-256 of a domain-separation prefix
/// concatenated with the normalized email — hashing through SHA-256
/// normalizes every email to a uniform 32-byte salt regardless of
/// length, clearing Argon2's 8-byte minimum and OWASP's 16-byte
/// recommendation for any email format.
fn derive_master_material(password: &str, email: &str) -> Result<[u8; AUTH_SECRET_LEN], String> {
    let normalized = normalize_email(email);

    let mut hasher = Sha256::new();
    hasher.update(MASTER_SALT_DOMAIN);
    hasher.update(normalized.as_bytes());
    let salt: [u8; 32] = hasher.finalize().into();

    let params = Params::new(
        ARGON2_M_COST_KIB,
        ARGON2_T_COST,
        ARGON2_P_COST,
        Some(AUTH_SECRET_LEN),
    )
    .map_err(|e| format!("invalid argon2 params: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; AUTH_SECRET_LEN];
    argon
        .hash_password_into(password.as_bytes(), &salt, &mut out)
        .map_err(|e| format!("argon2 derive: {e}"))?;
    Ok(out)
}

/// Canonicalize an email for use as a derivation salt input. Trims
/// surrounding whitespace and lowercases the whole address so that
/// `"  Foo@Bar.COM  "` and `"foo@bar.com"` produce identical master
/// material on every device and across every product.
///
/// We deliberately do NOT perform Unicode NFKC/IDN normalization
/// here. Better Auth stores emails as submitted; any normalization
/// mismatch between client and server would break auth, but as long
/// as every client in the codemux-api ecosystem uses this exact
/// function, multi-device/multi-product determinism is preserved
/// regardless of what Better Auth does server-side.
pub(crate) fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    use std::time::Instant;

    const PASS: &str = "correct horse battery staple";
    const PASS_WRONG: &str = "Tr0ub4dor&3";
    const EMAIL: &str = "alice@example.com";
    const EMAIL_OTHER: &str = "bob@example.com";

    // ----------------------------------------------------------------
    // Shape
    // ----------------------------------------------------------------

    #[test]
    fn auth_secret_is_valid_base64_of_32_bytes() {
        let secret = derive_auth_secret(PASS, EMAIL).expect("derive");
        let decoded = STANDARD_NO_PAD
            .decode(secret.as_str())
            .expect("auth_secret must be valid base64");
        assert_eq!(decoded.len(), 32, "auth_secret must decode to 32 bytes");
    }

    #[test]
    fn auth_secret_bytes_are_not_all_zero() {
        let secret = derive_auth_secret(PASS, EMAIL).expect("derive");
        let decoded = STANDARD_NO_PAD.decode(secret.as_str()).unwrap();
        assert!(
            decoded.iter().any(|b| *b != 0),
            "auth_secret must not be degenerate (all zeros)"
        );
    }

    // ----------------------------------------------------------------
    // Multi-device / cross-product contract — determinism
    // ----------------------------------------------------------------

    #[test]
    fn same_password_same_email_yields_identical_auth_secret() {
        // THE multi-device contract: two devices signing in with the
        // same password + email must derive byte-identical
        // auth_secrets so Better Auth's bcrypt match succeeds.
        let a = derive_auth_secret(PASS, EMAIL).unwrap();
        let b = derive_auth_secret(PASS, EMAIL).unwrap();
        assert_eq!(a.as_str(), b.as_str(), "auth_secret must be deterministic");
    }

    // ----------------------------------------------------------------
    // Input sensitivity (avalanche / user isolation)
    // ----------------------------------------------------------------

    #[test]
    fn different_password_yields_different_auth_secret() {
        let a = derive_auth_secret(PASS, EMAIL).unwrap();
        let b = derive_auth_secret(PASS_WRONG, EMAIL).unwrap();
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn different_email_yields_different_auth_secret() {
        // User isolation at the auth layer.
        let a = derive_auth_secret(PASS, EMAIL).unwrap();
        let b = derive_auth_secret(PASS, EMAIL_OTHER).unwrap();
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn single_character_password_change_flips_output() {
        let a = derive_auth_secret("password-A", EMAIL).unwrap();
        let b = derive_auth_secret("password-B", EMAIL).unwrap();
        assert_ne!(a.as_str(), b.as_str());
    }

    // ----------------------------------------------------------------
    // Email normalization
    // ----------------------------------------------------------------

    #[test]
    fn normalize_email_lowercases_ascii() {
        assert_eq!(normalize_email("Foo@Bar.COM"), "foo@bar.com");
    }

    #[test]
    fn normalize_email_trims_whitespace() {
        assert_eq!(normalize_email("  foo@bar.com  "), "foo@bar.com");
        assert_eq!(normalize_email("\tfoo@bar.com\n"), "foo@bar.com");
    }

    #[test]
    fn normalize_email_combines_lowercase_and_trim() {
        assert_eq!(normalize_email("  FOO@BAR.COM  "), "foo@bar.com");
    }

    #[test]
    fn email_case_is_insensitive_for_derivation() {
        // Device A signs up as "Alice@Example.COM"; device B signs
        // in as "alice@example.com". They MUST derive the same
        // credentials or cross-device login breaks.
        let a = derive_auth_secret(PASS, "Alice@Example.COM").unwrap();
        let b = derive_auth_secret(PASS, "alice@example.com").unwrap();
        assert_eq!(a.as_str(), b.as_str());
    }

    #[test]
    fn email_whitespace_is_ignored_for_derivation() {
        // A user who accidentally pastes " alice@example.com " with
        // trailing spaces must still derive the same credentials as
        // the canonical form.
        let a = derive_auth_secret(PASS, "  alice@example.com  ").unwrap();
        let b = derive_auth_secret(PASS, "alice@example.com").unwrap();
        assert_eq!(a.as_str(), b.as_str());
    }

    #[test]
    fn mixed_case_plus_whitespace_email_normalizes_correctly() {
        let a = derive_auth_secret(PASS, "\t  ALICE@example.COM\n").unwrap();
        let b = derive_auth_secret(PASS, "alice@example.com").unwrap();
        assert_eq!(a.as_str(), b.as_str());
    }

    // ----------------------------------------------------------------
    // Edge inputs — must not panic
    // ----------------------------------------------------------------

    #[test]
    fn empty_password_returns_ok() {
        // Empty password is weak but must not panic. The login UI
        // is responsible for rejecting it before calling this.
        let secret = derive_auth_secret("", EMAIL).expect("derive empty password");
        let decoded = STANDARD_NO_PAD.decode(secret.as_str()).unwrap();
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn empty_email_returns_ok() {
        let secret = derive_auth_secret(PASS, "").expect("derive empty email");
        let decoded = STANDARD_NO_PAD.decode(secret.as_str()).unwrap();
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn unicode_password_handled() {
        let a = derive_auth_secret("pässwörd 🔑", EMAIL).expect("derive");
        let b = derive_auth_secret("pässwörd 🔑", EMAIL).expect("derive");
        assert_eq!(a.as_str(), b.as_str());
        // Single codepoint change flips the output.
        let c = derive_auth_secret("pässwörd 🔒", EMAIL).expect("derive");
        assert_ne!(a.as_str(), c.as_str());
    }

    #[test]
    fn unicode_email_local_part_handled() {
        // RFC 6531 allows Unicode in the local part. We don't care
        // whether the server accepts it — we just care that
        // derivation is deterministic for the same input.
        let a = derive_auth_secret(PASS, "用户@example.com").unwrap();
        let b = derive_auth_secret(PASS, "用户@example.com").unwrap();
        assert_eq!(a.as_str(), b.as_str());
    }

    #[test]
    fn very_long_password_handled() {
        let long = "x".repeat(10_000);
        let _ = derive_auth_secret(&long, EMAIL).expect("derive 10k-char password");
    }

    #[test]
    fn very_long_email_handled() {
        // Real emails max at ~254 chars per RFC 5321. Push past
        // that to catch any length-dependent bugs in the salt
        // construction.
        let long = format!("{}@example.com", "a".repeat(2000));
        let _ = derive_auth_secret(PASS, &long).expect("derive long email");
    }

    // ----------------------------------------------------------------
    // Security properties
    // ----------------------------------------------------------------

    #[test]
    fn auth_secret_does_not_contain_password_substring() {
        // Structural leak check: the base64 auth_secret must not
        // literally contain the raw password anywhere. HKDF output
        // is indistinguishable from random, so the probability of
        // this failing by chance is negligible — if it ever fires,
        // the derivation is broken.
        let password = "super-distinctive-plaintext-password-12345";
        let secret = derive_auth_secret(password, EMAIL).unwrap();
        assert!(
            !secret.as_str().contains(password),
            "auth_secret string must not leak password substring"
        );
        let decoded = STANDARD_NO_PAD.decode(secret.as_str()).unwrap();
        let password_bytes = password.as_bytes();
        let contains_substring = decoded
            .windows(password_bytes.len())
            .any(|w| w == password_bytes);
        assert!(
            !contains_substring,
            "auth_secret bytes must not leak password substring"
        );
    }

    #[test]
    fn derivation_timing_at_least_100ms() {
        // Proof that Argon2id cost parameters haven't been
        // weakened. An attacker with a stolen bcrypt-of-auth_secret
        // hash must do a full Argon2id derivation per password
        // candidate. If this drops below ~100ms, dictionary attacks
        // on weak passwords become feasible in minutes.
        let start = Instant::now();
        let _ = derive_auth_secret(PASS, EMAIL).expect("derive");
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() >= 100,
            "argon2id derivation should take ≥ 100ms (got {}ms) — params weakened?",
            elapsed.as_millis()
        );
        assert!(
            elapsed.as_secs() < 30,
            "argon2id derivation should finish < 30s (got {:?}) — params too strong?",
            elapsed
        );
    }

    #[test]
    fn auth_secret_debug_format_does_not_leak_secret() {
        // If an AuthSecret ever ends up in a log line
        // (eprintln!("{:?}"), panic message, crash report, ...) the
        // value MUST be redacted. Pin the Debug impl.
        let secret = derive_auth_secret(PASS, EMAIL).unwrap();
        let rendered = format!("{:?}", secret);
        assert!(rendered.contains("***"));
        assert!(
            !rendered.contains(secret.as_str()),
            "Debug impl must not include the raw base64 secret"
        );
    }

    #[test]
    fn auth_secret_clone_preserves_value() {
        let secret = derive_auth_secret(PASS, EMAIL).unwrap();
        let cloned = secret.clone();
        assert_eq!(secret.as_str(), cloned.as_str());
    }

    // ----------------------------------------------------------------
    // Regression / cross-product compatibility
    // ----------------------------------------------------------------

    /// **Cross-product compatibility pin.** This locks the wire-
    /// format compatibility with Vexis (and any future product
    /// authenticating to api.codemux.org). The expected value comes
    /// from Vexis's `pinned_golden_values_codemux_api_v1` test in
    /// `/home/zeus/projects/vexis/src-tauri/src/encryption/manager.rs`.
    ///
    /// If this assertion ever fails, Codemux and Vexis have drifted
    /// — cross-product email+password login (the whole point of the
    /// shared Better Auth account) is broken until they re-converge.
    /// Any intentional rotation must bump the `vN` suffix in
    /// `MASTER_SALT_DOMAIN` / `AUTH_SECRET_INFO` in every product at
    /// the same time.
    #[test]
    fn auth_secret_matches_vexis_for_known_input() {
        let secret = derive_auth_secret(
            "golden-test-password",
            "golden-test@example.com",
        )
        .unwrap();
        assert_eq!(
            secret.as_str(),
            "9FxAbaiRLQfRmjpB6x4d3FuamAUojg9bh9dVfPYRfyI",
            "Codemux derivation diverged from Vexis — cross-product login is broken"
        );
    }

    /// Multi-input cross-product pin. Each `(password, email,
    /// expected_auth_secret)` triple below was captured by running
    /// Vexis's production `derive_login_credentials` against that
    /// input and copying the resulting `AuthSecret` string verbatim.
    /// Vexis was **NOT** modified — a temporary test was added,
    /// executed once, and reverted via `git checkout`.
    ///
    /// If any row fails, Codemux and Vexis have drifted — the
    /// derivations are no longer producing identical output for the
    /// same `(password, email)`, and cross-product login will fail
    /// for users whose credentials hash to the diverged branch.
    ///
    /// The set is chosen to exercise the edge cases most likely to
    /// drift silently: common ASCII, empty password, Unicode,
    /// email-normalization (mixed case + surrounding whitespace),
    /// and minimal-length inputs.
    #[test]
    fn auth_secret_matches_vexis_across_multiple_inputs() {
        const CASES: &[(&str, &str, &str)] = &[
            (
                "correct horse battery staple",
                "alice@example.com",
                "xmOSQbORXwp4R4j8AvUUe6enq9tYr1pwUYE4WJaSt3g",
            ),
            (
                "",
                "empty-password@example.com",
                "bdAIS62ZaOzVMViPKdr9vxoD0Y4UrYuyTAmx5T3/EsQ",
            ),
            (
                "password-with-spaces and café ☕",
                "unicode-test@example.com",
                "h3jjvVHu+73yfCpRP+WxO/x9ECYjxL0QKlYWndRK7us",
            ),
            // Email normalization: mixed-case + surrounding
            // whitespace must collapse to the same derivation.
            (
                "password",
                "  Mixed@Case.COM  ",
                "RPEpsBQM4Y+q6CXLL34UF/R6Ul+8cM4+nwGznP1lDK0",
            ),
            (
                "p",
                "a@b.co",
                "XqzDeX5p6BZ8Ws8ykB67QRUFEppODc7fsl/ddjEMruU",
            ),
        ];

        for (password, email, expected) in CASES {
            let got = derive_auth_secret(password, email)
                .expect("derive")
                .as_str()
                .to_string();
            assert_eq!(
                &got, expected,
                "Codemux derivation diverged from Vexis for (password={password:?}, email={email:?})"
            );
        }
    }

    #[test]
    fn derivation_is_stable_within_run() {
        // Belt-and-braces: two back-to-back derivations in the same
        // test process produce identical output. Pins that there's
        // no accidental randomness in the derivation path.
        let a = derive_auth_secret("pinned", "pinned@example.com").unwrap();
        let b = derive_auth_secret("pinned", "pinned@example.com").unwrap();
        assert_eq!(a.as_str(), b.as_str());
    }

    // ----------------------------------------------------------------
    // Encryption-key half (Step 10 — Skills Sync E2E)
    // ----------------------------------------------------------------

    #[test]
    fn encryption_key_is_32_raw_bytes() {
        let (_auth, key) = derive_login_credentials(PASS, EMAIL).unwrap();
        assert_eq!(key.as_bytes().len(), 32);
    }

    #[test]
    fn encryption_key_is_deterministic() {
        let (_a_auth, a_key) = derive_login_credentials(PASS, EMAIL).unwrap();
        let (_b_auth, b_key) = derive_login_credentials(PASS, EMAIL).unwrap();
        assert_eq!(a_key.as_bytes(), b_key.as_bytes());
    }

    #[test]
    fn encryption_key_differs_from_auth_secret() {
        // HKDF domain separation: the two halves come from different
        // info labels and must produce different bytes. If they ever
        // coincide, observing the server-visible AuthSecret would
        // leak the client-only EncryptionKey — catastrophic.
        let (auth, key) = derive_login_credentials(PASS, EMAIL).unwrap();
        let auth_decoded = STANDARD_NO_PAD.decode(auth.as_str()).unwrap();
        assert_ne!(auth_decoded.as_slice(), key.as_bytes().as_slice());
    }

    #[test]
    fn encryption_key_changes_with_password() {
        let (_a, key_a) = derive_login_credentials(PASS, EMAIL).unwrap();
        let (_b, key_b) = derive_login_credentials(PASS_WRONG, EMAIL).unwrap();
        assert_ne!(key_a.as_bytes(), key_b.as_bytes());
    }

    #[test]
    fn encryption_key_changes_with_email() {
        let (_a, key_a) = derive_login_credentials(PASS, EMAIL).unwrap();
        let (_b, key_b) = derive_login_credentials(PASS, EMAIL_OTHER).unwrap();
        assert_ne!(key_a.as_bytes(), key_b.as_bytes());
    }

    #[test]
    fn encryption_key_email_normalization_matches() {
        let (_a, key_a) = derive_login_credentials(PASS, "Alice@Example.COM").unwrap();
        let (_b, key_b) = derive_login_credentials(PASS, "alice@example.com").unwrap();
        assert_eq!(key_a.as_bytes(), key_b.as_bytes());
    }

    #[test]
    fn encryption_key_debug_redacts() {
        let (_auth, key) = derive_login_credentials(PASS, EMAIL).unwrap();
        let rendered = format!("{:?}", key);
        assert!(rendered.contains("***"));
    }

    /// **Cross-product encryption-key pin.** Must match Vexis
    /// byte-for-byte. Expected hex from `codemux-api-infrastructure`
    /// skill: this is the canary that catches any drift between
    /// Codemux's and Vexis's encryption_key derivation.
    #[test]
    fn encryption_key_matches_vexis_for_known_input() {
        let (_auth, key) = derive_login_credentials(
            "golden-test-password",
            "golden-test@example.com",
        )
        .unwrap();
        let hex_str: String = key
            .as_bytes()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        assert_eq!(
            hex_str,
            "0b03b2541a15d39e7a422df4a6e6ade56b371453d5b41c1ec117efa62db54534",
            "Codemux encryption_key derivation diverged from Vexis — cross-product E2E sync is broken"
        );
    }

    #[test]
    fn derive_login_credentials_auth_half_matches_derive_auth_secret() {
        // `derive_login_credentials` returns the same AuthSecret bytes
        // as the convenience `derive_auth_secret` wrapper for the same
        // input. If they diverge, callers that picked one entry point
        // and not the other would produce mismatched server hashes.
        let (a_auth, _key) = derive_login_credentials(PASS, EMAIL).unwrap();
        let b_auth = derive_auth_secret(PASS, EMAIL).unwrap();
        assert_eq!(a_auth.as_str(), b_auth.as_str());
    }
}
