import { describe, expect, it } from "vitest";
import {
  describeUpdateError,
  manualUpdateCommand,
  releasePageUrl,
} from "./update-release";

describe("releasePageUrl", () => {
  it("links the tag for a known version", () => {
    expect(releasePageUrl("0.24.0")).toBe(
      "https://github.com/Zeus-Deus/codemux/releases/tag/v0.24.0",
    );
  });

  it("falls back to the latest release without a version", () => {
    expect(releasePageUrl(null)).toBe(
      "https://github.com/Zeus-Deus/codemux/releases/latest",
    );
  });
});

describe("manualUpdateCommand", () => {
  it("names the AUR package for pacman installs", () => {
    expect(manualUpdateCommand("pacman")).toBe("yay -S codemux-bin");
  });

  it("has no command for formats it cannot name a package for", () => {
    expect(manualUpdateCommand("other")).toBeNull();
    expect(manualUpdateCommand("appimage")).toBeNull();
    expect(manualUpdateCommand(null)).toBeNull();
  });
});

describe("describeUpdateError", () => {
  it("reads Error messages and the plain strings Tauri rejects with", () => {
    expect(describeUpdateError(new Error("spawn failed"))).toBe("spawn failed");
    expect(describeUpdateError("404 Not Found")).toBe("404 Not Found");
  });

  it("never returns an empty reason", () => {
    expect(describeUpdateError("")).toBe(
      "The updater stopped without giving a reason.",
    );
    expect(describeUpdateError(undefined)).toBe(
      "The updater stopped without giving a reason.",
    );
  });
});
