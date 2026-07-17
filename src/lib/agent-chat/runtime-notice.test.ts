import { describe, expect, it } from "vitest";

import { runtimeNoticeFromWarning } from "./runtime-notice";

describe("runtimeNoticeFromWarning", () => {
  it("promotes a rejected rate-limit event to a usage-limit notice", () => {
    expect(
      runtimeNoticeFromWarning("rate limit event", {
        rate_limit_info: { status: "rejected" },
      }),
    ).toBe("Usage limit reached — the provider stopped the run.");
  });

  it("ignores an informational (non-rejected) rate-limit event", () => {
    expect(
      runtimeNoticeFromWarning("rate limit event", {
        rate_limit_info: { status: "allowed" },
      }),
    ).toBeNull();
    // Missing / malformed payload also stays console-only.
    expect(runtimeNoticeFromWarning("rate limit event", null)).toBeNull();
    expect(runtimeNoticeFromWarning("rate limit event", {})).toBeNull();
    expect(
      runtimeNoticeFromWarning("rate limit event", { rate_limit_info: 42 }),
    ).toBeNull();
  });

  it("maps an enumerated assistant error to a provider-error notice", () => {
    expect(
      runtimeNoticeFromWarning("assistant error: rate_limit", null),
    ).toBe("Provider error: rate_limit");
    expect(
      runtimeNoticeFromWarning("assistant error: overloaded_error", {}),
    ).toBe("Provider error: overloaded_error");
  });

  it("promotes a resume-fallback warning to its inline notice text", () => {
    const text =
      "Previous session context couldn't be restored, so this turn continues in a fresh session. Your chat history is preserved.";
    expect(runtimeNoticeFromWarning("resume-fallback: " + text, null)).toBe(
      text,
    );
  });

  it("keeps SDK debug chatter console-only", () => {
    expect(
      runtimeNoticeFromWarning("stream_event message_start", {
        type: "stream_event",
      }),
    ).toBeNull();
    expect(runtimeNoticeFromWarning("unknown sdk variant", {})).toBeNull();
  });
});
