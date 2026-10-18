import { describe, expect, it } from "vitest";

import { isRestartResumeText } from "./restart-resume";

describe("isRestartResumeText", () => {
  it("matches only the backend's restart note", () => {
    expect(
      isRestartResumeText(
        "[Resumed automatically after an app restart. Codemux quit while you were working on this task.]",
      ),
    ).toBe(true);
    expect(isRestartResumeText("Continue")).toBe(false);
    expect(isRestartResumeText(" [Resumed automatically after an app restart")).toBe(false);
    expect(
      isRestartResumeText("[Resumed automatically after a provider usage limit reset."),
    ).toBe(false);
  });
});
