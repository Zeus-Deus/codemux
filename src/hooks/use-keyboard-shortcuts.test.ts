import { describe, it, expect, beforeEach } from "vitest";
import { dispatch } from "./use-keyboard-shortcuts";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";

// A fake KeyboardEvent — dispatch only uses the second arg when it needs to
// call preventDefault via the caller, not inside dispatch itself, so a stub
// is fine for closeOverlay-path testing.
const FAKE_EVENT = new KeyboardEvent("keydown", { key: "Escape" });

beforeEach(() => {
  useAppStore.setState({ appState: null });
  useUIStore.setState({
    onboardingProjectDir: null,
    hasSeenOnboarding: false,
    showSettings: false,
    showFileSearch: false,
    showContentSearch: false,
    showCommandPalette: false,
  });
  window.localStorage.clear();
});

describe("use-keyboard-shortcuts dispatch — closeOverlay precedence", () => {
  it("returns false when no overlay is open", () => {
    const handled = dispatch("closeOverlay", FAKE_EVENT);
    expect(handled).toBe(false);
  });

  it("returns false for an unknown actionId", () => {
    // Sanity check — dispatch should not swallow unrelated actions even when
    // state that closeOverlay cares about is set.
    useUIStore.setState({ showSettings: true });
    const handled = dispatch("someOtherAction", FAKE_EVENT);
    expect(handled).toBe(false);
    // And settings stays open.
    expect(useUIStore.getState().showSettings).toBe(true);
  });

  describe("onboarding priority", () => {
    it("clears onboardingProjectDir when it's set (the escape-hatch fix)", () => {
      useUIStore.setState({ onboardingProjectDir: "/home/user/myproj" });

      const handled = dispatch("closeOverlay", FAKE_EVENT);

      expect(handled).toBe(true);
      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBeNull();
      // And Escape counts as "onboarding seen" so Escape-dismissal doesn't
      // re-arm on the next project open.
      expect(s.hasSeenOnboarding).toBe(true);
    });

    it("onboarding takes precedence over settings", () => {
      useUIStore.setState({
        onboardingProjectDir: "/home/user/myproj",
        showSettings: true,
      });

      dispatch("closeOverlay", FAKE_EVENT);

      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBeNull();
      // Settings stays open — only one overlay closes per Escape press.
      expect(s.showSettings).toBe(true);
    });

    it("onboarding takes precedence over all other overlays simultaneously", () => {
      useUIStore.setState({
        onboardingProjectDir: "/home/user/myproj",
        showSettings: true,
        showFileSearch: true,
        showContentSearch: true,
        showCommandPalette: true,
      });

      dispatch("closeOverlay", FAKE_EVENT);

      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBeNull();
      expect(s.showSettings).toBe(true);
      expect(s.showFileSearch).toBe(true);
      expect(s.showContentSearch).toBe(true);
      expect(s.showCommandPalette).toBe(true);
    });
  });

  describe("other overlays — ordering preserved", () => {
    it("closes settings when onboarding is not active", () => {
      useUIStore.setState({ showSettings: true });
      const handled = dispatch("closeOverlay", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showSettings).toBe(false);
    });

    it("closes fileSearch before contentSearch", () => {
      useUIStore.setState({ showFileSearch: true, showContentSearch: true });
      dispatch("closeOverlay", FAKE_EVENT);
      const s = useUIStore.getState();
      expect(s.showFileSearch).toBe(false);
      expect(s.showContentSearch).toBe(true);
    });

    it("closes contentSearch before commandPalette", () => {
      useUIStore.setState({
        showContentSearch: true,
        showCommandPalette: true,
      });
      dispatch("closeOverlay", FAKE_EVENT);
      const s = useUIStore.getState();
      expect(s.showContentSearch).toBe(false);
      expect(s.showCommandPalette).toBe(true);
    });

    it("closes commandPalette when it's the only overlay open", () => {
      useUIStore.setState({ showCommandPalette: true });
      const handled = dispatch("closeOverlay", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showCommandPalette).toBe(false);
    });
  });

  describe("other dispatch actions still work", () => {
    it("commandPalette toggles on", () => {
      expect(useUIStore.getState().showCommandPalette).toBe(false);
      const handled = dispatch("commandPalette", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showCommandPalette).toBe(true);
    });

    it("openSettings opens settings", () => {
      const handled = dispatch("openSettings", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showSettings).toBe(true);
    });
  });
});
