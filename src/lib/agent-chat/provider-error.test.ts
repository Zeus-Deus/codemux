import { describe, expect, it } from "vitest";

import { formatProviderError } from "./provider-error";

describe("formatProviderError", () => {
  it("renders not_installed with the provider label and hint", () => {
    const raw = JSON.stringify({
      kind: "not_installed",
      provider: "claude",
      hint: "claude-agent sidecar not found at /usr/lib/codemux/sidecar",
    });
    expect(formatProviderError(raw)).toBe(
      "Claude CLI is not installed. claude-agent sidecar not found at /usr/lib/codemux/sidecar",
    );
  });

  it("renders not_authenticated with the remediation hint", () => {
    const raw = JSON.stringify({
      kind: "not_authenticated",
      provider: "codex",
      hint: "Run `codex login` and try again.",
    });
    expect(formatProviderError(raw)).toBe(
      "Codex CLI is not authenticated. Run `codex login` and try again.",
    );
  });

  it("renders process_error with its source detail", () => {
    const raw = JSON.stringify({
      kind: "process_error",
      message: "failed to spawn claude-agent sidecar",
      source: "Permission denied (os error 13)",
    });
    expect(formatProviderError(raw)).toBe(
      "failed to spawn claude-agent sidecar (Permission denied (os error 13))",
    );
  });

  it("renders timeout in seconds", () => {
    const raw = JSON.stringify({
      kind: "timeout",
      operation: "start-session",
      elapsed_ms: 30000,
    });
    expect(formatProviderError(raw)).toBe("start-session timed out after 30s.");
  });

  it("passes through non-JSON rejections verbatim", () => {
    expect(formatProviderError("provider_not_configured: Claude")).toBe(
      "provider_not_configured: Claude",
    );
    expect(formatProviderError(new Error("plain failure"))).toBe(
      "plain failure",
    );
  });

  it("passes through unknown JSON kinds verbatim", () => {
    const raw = JSON.stringify({ kind: "mystery", detail: "??" });
    expect(formatProviderError(raw)).toBe(raw);
  });
});
