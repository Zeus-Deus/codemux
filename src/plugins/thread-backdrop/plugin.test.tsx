import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";

vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), info: vi.fn(), success: vi.fn(), warning: vi.fn() },
}));
vi.mock("@/tauri/commands", () => ({ dbDeleteSetting: vi.fn(async () => {}) }));

import { activatePlugin, resetActivationForTests, unloadPlugin } from "@/contrib/activation";
import { registry } from "@/contrib/registry";
import { resetPluginsStoreForTests } from "@/contrib/plugins-store";
import { LandingBackdropHost } from "@/contrib/react/landing-backdrop";
import { bundledEntries } from "@/contrib/bundled";
import { LANDING_BACKDROP_AREA } from "@codemux/sdk";
import plugin from "./plugin";
import meta from "./meta";

const record = { id: "thread-backdrop", name: "Thread backdrop", kind: "bundled" as const, tier: "official" as const };

afterEach(() => {
  cleanup();
  resetActivationForTests();
  resetPluginsStoreForTests();
});

function Landing() {
  return (
    <LandingBackdropHost>
      <main data-testid="landing">
        <div data-backdrop-calm>
          <h1>What should we work on?</h1>
        </div>
      </main>
    </LandingBackdropHost>
  );
}

describe("thread-backdrop plugin", () => {
  it("is an Official bundled plugin that ships on", () => {
    const entry = bundledEntries().find((e) => e.meta.id === "thread-backdrop");
    expect(entry).toBeTruthy();
    expect(meta.defaultEnabled).toBe(true);
    expect(plugin.id).toBe(meta.id);
  });

  it("draws behind the landing while on and leaves no trace when off", () => {
    const { container } = render(<Landing />);
    expect(container.querySelector("[data-landing-backdrop-host]")).toBeNull();

    act(() => {
      expect(activatePlugin(plugin, record)).toBe(true);
    });
    expect(registry.getArea(LANDING_BACKDROP_AREA).map((c) => c.id)).toEqual(["thread-backdrop:backdrop"]);
    const layer = container.querySelector("[data-thread-backdrop]");
    expect(layer).not.toBeNull();
    expect(layer!.closest("[aria-hidden]")?.className).toContain("pointer-events-none");

    act(() => unloadPlugin("thread-backdrop"));
    expect(registry.getArea(LANDING_BACKDROP_AREA)).toEqual([]);
    expect(container.querySelector("[data-landing-backdrop-host]")).toBeNull();
    expect(container.querySelector("[data-thread-backdrop]")).toBeNull();
    expect(container.firstElementChild?.tagName).toBe("MAIN");
  });
});
