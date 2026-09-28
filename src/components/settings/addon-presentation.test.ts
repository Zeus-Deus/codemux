import { expect, it } from "vitest";
import { addonProblem } from "./addon-presentation";

const rejection = (code: string, message: string) => ({
  message,
  data: { code },
});

// Every catalog, link and device rejection the host words distinctly.
it.each([
  [
    "INCOMPATIBLE_API",
    "Version 1.3.0 of example.hello is not available for Linux x64",
    "Not available for this device",
  ],
  [
    "INCOMPATIBLE_API",
    "No release of example.hello is available for Windows x64",
    "Not available for this device",
  ],
  [
    "INCOMPATIBLE_API",
    "Add-ons are not supported on this platform",
    "Not available for this device",
  ],
  [
    "INCOMPATIBLE_API",
    "Add-ons are unsupported on this platform",
    "Not available for this device",
  ],
  [
    "INCOMPATIBLE_API",
    "This platform is not supported by add-ons",
    "Not available for this device",
  ],
  [
    "INCOMPATIBLE_API",
    "Version 2.0.0 of example.hello requires add-on API ^2.0; this CodeMux provides 1.0.0",
    "Incompatible add-on API",
  ],
  [
    "INCOMPATIBLE_API",
    "No release of example.hello supports add-on API 1.0.0; version 2.0.0 requires ^2.0",
    "Incompatible add-on API",
  ],
  [
    "INCOMPATIBLE_API",
    "Requires an unsupported plugin API",
    "Incompatible add-on API",
  ],
  ["INCOMPATIBLE_API", "Incompatible protocol", "Incompatible release"],
  [
    "PERMISSION_DENIED",
    "This release is blocked: Revoked build",
    "Release blocked",
  ],
  [
    "INVALID_MESSAGE",
    "example.hello is not listed in the add-on catalog",
    "Not in the catalog",
  ],
  [
    "INVALID_MESSAGE",
    "Version 9.9.9 of example.hello is not listed in the add-on catalog",
    "Not in the catalog",
  ],
  [
    "INVALID_MESSAGE",
    "example.hello has no releases in the add-on catalog",
    "Not in the catalog",
  ],
  [
    "INVALID_MESSAGE",
    "Hello 1.0.0 is already installed",
    "Already installed",
  ],
  [
    "INVALID_MESSAGE",
    "Downgrades require the recorded rollback action",
    "Older release",
  ],
  [
    "INVALID_MESSAGE",
    "Use a catalog ID or a codemux.org add-on install link",
    "Not a valid install link or ID",
  ],
  [
    "INVALID_MESSAGE",
    "The install link version is not a semantic version",
    "Not a valid install link or ID",
  ],
  [
    "INVALID_MESSAGE",
    "The add-on catalog lists only stable release versions",
    "Not a valid install link or ID",
  ],
  ["NETWORK_DENIED", "Add-on download failed", "Network unavailable"],
  ["TIMEOUT", "Add-on download timed out", "Network unavailable"],
])("titles %s %j as %j", (code, message, title) => {
  expect(addonProblem(rejection(code, message))).toEqual({ title, message });
});

it("keeps the host message without a title for other rejections", () => {
  expect(
    addonProblem(rejection("STORAGE_UNAVAILABLE", "Not enough disk space")),
  ).toEqual({ title: null, message: "Not enough disk space" });
  expect(addonProblem(new Error("Unexpected failure"))).toEqual({
    title: null,
    message: "Unexpected failure",
  });
});
