import { beforeEach, describe, expect, it } from "vitest";
import {
  DEFAULT_FOOTER_PINS,
  FOOTER_STORAGE_KEY,
  useFooterPinsStore,
  validateFooterPins,
} from "./footer-pins-store";
import { FOOTER_ACTIONS, isFooterActionAvailable } from "@/lib/footer-actions";
import { buildNavGroups } from "@/lib/settings-sections";

beforeEach(() => {
  localStorage.clear();
  useFooterPinsStore.getState().reset();
});
describe("footer preferences", () => {
  it("validates stale IDs, duplicates, malformed entries and icon IDs without sorting", () => {
    expect(
      validateFooterPins([
        { id: "codemux.ports.open", iconId: "star", script: "untrusted" },
        { id: "codemux.settings.appearance", iconId: "__proto__" },
        { id: "codemux.ports.open" },
        { id: "removed.action" },
        null,
        42,
      ]),
    ).toEqual([
      { id: "codemux.ports.open", iconId: "star" },
      { id: "codemux.settings.appearance" },
    ]);
    expect(validateFooterPins(null)).toEqual(DEFAULT_FOOTER_PINS);
    expect(validateFooterPins([])).toEqual([]);
  });
  it("persists ordered IDs and allowlisted icon overrides, and restores defaults", async () => {
    const store = useFooterPinsStore.getState();
    store.togglePin("codemux.settings.appearance");
    store.movePin("codemux.settings.appearance", -1);
    store.setIcon("codemux.settings.appearance", "star");
    const saved = JSON.parse(localStorage.getItem(FOOTER_STORAGE_KEY)!);
    expect(saved.version).toBe(1);
    expect(saved.state.pins[3]).toEqual({
      id: "codemux.settings.appearance",
      iconId: "star",
    });
    useFooterPinsStore.setState({ pins: [] });
    localStorage.setItem(FOOTER_STORAGE_KEY, JSON.stringify(saved));
    await useFooterPinsStore.persist.rehydrate();
    expect(useFooterPinsStore.getState().pins).toEqual(saved.state.pins);
    store.reset();
    expect(useFooterPinsStore.getState().pins).toEqual(DEFAULT_FOOTER_PINS);
  });
  it("recovers malformed and unsupported-version storage without losing store actions", async () => {
    for (const raw of [
      "{broken",
      '{"version":1,"state":{"pins":null}}',
      '{"version":99,"state":{"pins":[]}}',
    ]) {
      localStorage.setItem(FOOTER_STORAGE_KEY, raw);
      await useFooterPinsStore.persist.rehydrate();
      expect(useFooterPinsStore.getState().pins).toEqual(DEFAULT_FOOTER_PINS);
      expect(useFooterPinsStore.getState().togglePin).toBeTypeOf("function");
    }
  });
  it("offers every visible Settings section and retains gated pins in their saved order", () => {
    const skills = FOOTER_ACTIONS.find(
      (action) => action.id === "codemux.settings.skills",
    )!;
    useFooterPinsStore.getState().togglePin(skills.id);
    const pins = useFooterPinsStore.getState().pins;
    expect(isFooterActionAvailable(skills, false, true)).toBe(false);
    expect(isFooterActionAvailable(skills, true, true)).toBe(true);
    expect(useFooterPinsStore.getState().pins).toBe(pins);
    for (const enabled of [false, true]) {
      const offered = FOOTER_ACTIONS.filter(
        (action) =>
          action.section && isFooterActionAvailable(action, enabled, false),
      ).map((action) => action.section);
      expect(offered).toEqual(
        buildNavGroups(enabled).flatMap((group) =>
          group.items.map((item) => item.id),
        ),
      );
    }
  });
});
