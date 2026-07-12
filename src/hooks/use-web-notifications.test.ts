import { describe, it, expect } from "vitest";

import { chooseNotificationDelivery } from "./use-web-notifications";

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
