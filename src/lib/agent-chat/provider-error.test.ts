import { describe, expect, it } from "vitest";

import {
  formatProviderError,
  grokModelChangeRequiresRestart,
  parsePaneAlreadyBound,
} from "./provider-error";

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

  it("uses the Grok provider label", () => {
    const raw = JSON.stringify({
      kind: "not_authenticated",
      provider: "grok",
      hint: "Run `grok login` and try again.",
    });
    expect(formatProviderError(raw)).toBe(
      "Grok CLI is not authenticated. Run `grok login` and try again.",
    );
  });

  it("recognizes and formats Grok model-family restart errors", () => {
    const raw = JSON.stringify({
      kind: "validation_error",
      message:
        "grok_model_restart_required: Grok cannot switch model families in this session.",
    });
    expect(grokModelChangeRequiresRestart(raw)).toBe(true);
    expect(formatProviderError(raw)).toBe(
      "Grok cannot switch model families in this session.",
    );
    expect(grokModelChangeRequiresRestart("some other failure")).toBe(false);
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

describe("parsePaneAlreadyBound", () => {
  const raw = JSON.stringify({
    kind: "pane_already_bound",
    pane_id: "pane-7",
    thread_id: "thread-winner",
    provider: "codex",
  });

  it("parses the winning binding out of a rejected claim", () => {
    expect(parsePaneAlreadyBound(raw)).toEqual({
      paneId: "pane-7",
      threadId: "thread-winner",
      provider: "codex",
    });
  });

  it("accepts the rejection as an Error whose message is the JSON", () => {
    // Tauri surfaces command rejections as strings, but anything that
    // round-trips through a `catch` may arrive wrapped.
    expect(parsePaneAlreadyBound(new Error(raw))).toEqual({
      paneId: "pane-7",
      threadId: "thread-winner",
      provider: "codex",
    });
  });

  it("reports a null provider when the kind is unknown to the UI", () => {
    const claim = parsePaneAlreadyBound(
      JSON.stringify({
        kind: "pane_already_bound",
        pane_id: "pane-7",
        thread_id: "thread-winner",
        provider: "some-future-cli",
      }),
    );
    expect(claim).toEqual({
      paneId: "pane-7",
      threadId: "thread-winner",
      provider: null,
    });
  });

  it("returns null without the winning thread id — nothing to adopt", () => {
    expect(
      parsePaneAlreadyBound(
        JSON.stringify({ kind: "pane_already_bound", pane_id: "pane-7" }),
      ),
    ).toBeNull();
  });

  it("returns null for other provider errors and for plain strings", () => {
    expect(
      parsePaneAlreadyBound(
        JSON.stringify({ kind: "not_installed", provider: "claude" }),
      ),
    ).toBeNull();
    expect(parsePaneAlreadyBound("provider_not_configured: Claude")).toBeNull();
    expect(parsePaneAlreadyBound(null)).toBeNull();
  });
});

describe("formatProviderError — pane claim rejections", () => {
  it("turns the bare pane_not_found string into a sentence", () => {
    // The backend returns `pane_not_found: <pane id>` (not a wire error)
    // when the claim runs against a pane that was closed mid-start.
    expect(formatProviderError("pane_not_found: pane-7")).toBe(
      "This chat pane was closed before the session could start.",
    );
  });

  it("explains a lost claim for callers that cannot adopt the binding", () => {
    expect(
      formatProviderError(
        JSON.stringify({
          kind: "pane_already_bound",
          pane_id: "pane-7",
          thread_id: "thread-winner",
          provider: "codex",
        }),
      ),
    ).toBe("This chat pane is already running a session started elsewhere.");
  });
});
