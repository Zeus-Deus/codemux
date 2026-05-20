import { describe, it, expect, vi } from "vitest";

// The hook module imports Tauri plugins at the top level. Stub them so the
// pure helpers under test can be imported without a Tauri runtime present.
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("@/tauri/commands", () => ({ getPackageFormat: vi.fn() }));

import { canAutoUpdateFormat, isVersionDismissed } from "./use-update-checker";

describe("canAutoUpdateFormat", () => {
  it("treats AppImage installs as auto-updatable", () => {
    expect(canAutoUpdateFormat("appimage")).toBe(true);
  });

  // Regression guard: before this fix `get_package_format()` returned
  // "other" on Windows, so `canAutoUpdate` was false and every Windows
  // user was downgraded to a manual download-and-reinstall flow despite
  // the NSIS updater fully supporting in-app updates.
  it("treats Windows NSIS installs as auto-updatable", () => {
    expect(canAutoUpdateFormat("nsis")).toBe(true);
  });

  it("does not auto-update deb/rpm/other installs", () => {
    expect(canAutoUpdateFormat("other")).toBe(false);
    expect(canAutoUpdateFormat("deb")).toBe(false);
    expect(canAutoUpdateFormat("rpm")).toBe(false);
  });

  it("rejects empty and unknown formats", () => {
    expect(canAutoUpdateFormat("")).toBe(false);
    expect(canAutoUpdateFormat("snap")).toBe(false);
  });
});

describe("isVersionDismissed", () => {
  // The dismissed version is held in an in-memory ref that starts as null
  // on every launch — so a fresh launch always shows the toast again.
  it("shows the toast on a fresh launch (nothing dismissed yet)", () => {
    expect(isVersionDismissed(null, "0.5.1")).toBe(false);
  });

  it("hides the toast for a version dismissed this session", () => {
    expect(isVersionDismissed("0.5.1", "0.5.1")).toBe(true);
  });

  // If a newer version is published while the app is running, the toast
  // must reappear even though an older version was dismissed.
  it("re-shows the toast when a newer version is published", () => {
    expect(isVersionDismissed("0.5.0", "0.5.1")).toBe(false);
  });
});
