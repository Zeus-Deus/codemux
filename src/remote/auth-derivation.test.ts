import { describe, expect, it, vi } from "vitest";

import { deriveAuthSecret, normalizeEmail } from "./auth-derivation";

// Argon2id (m=64MiB, t=3, p=4) is deliberately slow (~0.3–0.5s), and balloons
// to several seconds under the full suite's parallel CPU contention. Tests that
// derive more than once can exceed the 5s default, so widen the file timeout.
vi.setConfig({ testTimeout: 30_000 });

// These golden AuthSecret strings are the SAME pins the Rust suite asserts on
// (`auth_secret_matches_vexis_*` in `src-tauri/src/auth/derivation.rs`). If this
// file drifts from the Rust/Vexis derivation, cross-product + web-remote account
// login breaks — so the pins are duplicated here on purpose: they lock the
// browser derivation to the shared `codemux-api-*` protocol byte-for-byte.
describe("deriveAuthSecret — cross-implementation golden pins", () => {
  const CASES: Array<[password: string, email: string, expected: string]> = [
    // The canonical golden value, matching Rust's
    // `auth_secret_matches_vexis_for_known_input`.
    [
      "golden-test-password",
      "golden-test@example.com",
      "9FxAbaiRLQfRmjpB6x4d3FuamAUojg9bh9dVfPYRfyI",
    ],
    [
      "correct horse battery staple",
      "alice@example.com",
      "xmOSQbORXwp4R4j8AvUUe6enq9tYr1pwUYE4WJaSt3g",
    ],
    // Empty password — weak but must not throw and must match the peer.
    [
      "",
      "empty-password@example.com",
      "bdAIS62ZaOzVMViPKdr9vxoD0Y4UrYuyTAmx5T3/EsQ",
    ],
    // Unicode password.
    [
      "password-with-spaces and café ☕",
      "unicode-test@example.com",
      "h3jjvVHu+73yfCpRP+WxO/x9ECYjxL0QKlYWndRK7us",
    ],
    // Email normalization: mixed case + surrounding whitespace must collapse.
    [
      "password",
      "  Mixed@Case.COM  ",
      "RPEpsBQM4Y+q6CXLL34UF/R6Ul+8cM4+nwGznP1lDK0",
    ],
    // Minimal-length inputs.
    ["p", "a@b.co", "XqzDeX5p6BZ8Ws8ykB67QRUFEppODc7fsl/ddjEMruU"],
  ];

  for (const [password, email, expected] of CASES) {
    it(`matches the pinned AuthSecret for (${JSON.stringify(password).slice(0, 24)}, ${email})`, async () => {
      const got = await deriveAuthSecret(password, email);
      expect(got).toBe(expected);
    });
  }
});

describe("deriveAuthSecret — protocol properties", () => {
  it("is deterministic for the same input", async () => {
    const a = await deriveAuthSecret("hunter2", "user@example.com");
    const b = await deriveAuthSecret("hunter2", "user@example.com");
    expect(a).toBe(b);
  });

  // Input sensitivity (password/email → distinct output) is already proven by
  // the six distinct golden pins above; the Rust suite covers it exhaustively.
  // Here we keep only the properties the golden pins don't demonstrate.

  it("is case- and whitespace-insensitive in the email", async () => {
    const a = await deriveAuthSecret("hunter2", "  Alice@Example.COM ");
    const b = await deriveAuthSecret("hunter2", "alice@example.com");
    expect(a).toBe(b);
  });

  it("decodes to 32 bytes and never leaks the raw password", async () => {
    const password = "super-distinctive-plaintext-password-12345";
    const secret = await deriveAuthSecret(password, "leak@example.com");
    expect(secret).not.toContain(password);
    const decoded = atob(secret);
    expect(decoded.length).toBe(32);
  });
});

describe("normalizeEmail", () => {
  it("lowercases and trims", () => {
    expect(normalizeEmail("  FOO@BAR.COM  ")).toBe("foo@bar.com");
    expect(normalizeEmail("\tfoo@bar.com\n")).toBe("foo@bar.com");
  });
});
