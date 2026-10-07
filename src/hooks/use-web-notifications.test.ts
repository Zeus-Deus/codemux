import { describe, it, expect, vi } from "vitest";

const { toastInfo } = vi.hoisted(() => ({ toastInfo: vi.fn() }));
const openNotificationTarget = vi.fn();
vi.mock("@/lib/toast", () => ({ toast: { info: toastInfo } }));
vi.mock("@/lib/open-notification-target", () => ({
  openNotificationTarget: (target: unknown) => openNotificationTarget(target),
}));

import { chooseNotificationDelivery, showToast } from "./use-web-notifications";

describe("chooseNotificationDelivery", () => {
  it("uses a toast when the Web Notifications API is unavailable", () => {
    // Insecure origin / old browser: no `Notification` at all.
    expect(
      chooseNotificationDelivery({
        apiAvailable: false,
        permission: null,
        pageHidden: true,
      }),
    ).toBe("toast");
    // Even a nominally-"granted" permission can't win without the API.
    expect(
      chooseNotificationDelivery({
        apiAvailable: false,
        permission: "granted",
        pageHidden: true,
      }),
    ).toBe("toast");
  });

  it("raises an OS notification only when granted AND the tab is hidden", () => {
    expect(
      chooseNotificationDelivery({
        apiAvailable: true,
        permission: "granted",
        pageHidden: true,
      }),
    ).toBe("web");
  });

  it("uses a toast when granted but the tab is visible", () => {
    // User is already looking at the tab — a system notification would be
    // redundant, so a toast is the right, quieter signal.
    expect(
      chooseNotificationDelivery({
        apiAvailable: true,
        permission: "granted",
        pageHidden: false,
      }),
    ).toBe("toast");
  });

  it("uses a toast when permission is denied", () => {
    expect(
      chooseNotificationDelivery({
        apiAvailable: true,
        permission: "denied",
        pageHidden: true,
      }),
    ).toBe("toast");
  });

  it("uses a toast when permission is still default (not yet granted)", () => {
    expect(
      chooseNotificationDelivery({
        apiAvailable: true,
        permission: "default",
        pageHidden: true,
      }),
    ).toBe("toast");
  });
});

describe("showToast", () => {
  it("offers an Open action that goes to the agent's pane", () => {
    const payload = {
      title: "Agent finished — My Project",
      body: "Codemux is waiting for your review.",
      workspace_title: "My Project",
      workspace_id: "ws-1",
      pane_id: "pane-7",
    };
    showToast(payload);

    const [title, opts] = toastInfo.mock.calls[0];
    expect(title).toBe(payload.title);
    expect(opts.description).toBe(payload.body);
    expect(opts.action.label).toBe("Open");
    opts.action.onClick();
    expect(openNotificationTarget).toHaveBeenCalledWith(payload);
  });
});
