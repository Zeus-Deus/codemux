import { setRemoteViewHost } from "./client-view";
import { beforeEach, describe, expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => true,
}));
import {
  enablePush,
  disablePush,
  defaultCategories,
  pushUnavailableReason,
  decodeApplicationKey,
} from "./push";
const unsubscribe = vi.fn();
const requestPermission = vi.fn();
const subscribe = vi.fn();
const subscription = {
  toJSON: () => ({ endpoint: "https://web.push.apple.com/test" }),
  unsubscribe,
};
beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal("isSecureContext", true);
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  vi.stubGlobal("PushManager", function () {});
  vi.stubGlobal("Notification", { permission: "default", requestPermission });
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: {
      register: vi.fn().mockResolvedValue({
        active: {},
        pushManager: { getSubscription: async () => null, subscribe },
      }),
      getRegistration: vi.fn().mockResolvedValue({
        pushManager: { getSubscription: async () => subscription },
      }),
    },
  });
  invoke.mockResolvedValue({ public_key: "BA" });
  requestPermission.mockResolvedValue("granted");
  subscribe.mockResolvedValue(subscription);
  unsubscribe.mockResolvedValue(true);
});
describe("push opt-in", () => {
  it("requests permission from the user gesture before any asynchronous registration or host request", async () => {
    const promise = enablePush(defaultCategories);
    expect(requestPermission).toHaveBeenCalledOnce();
    expect(invoke).not.toHaveBeenCalled();
    await promise;
    expect(subscribe).toHaveBeenCalledWith(
      expect.objectContaining({ userVisibleOnly: true }),
    );
    expect(invoke).toHaveBeenCalledWith(
      "web_push_subscribe",
      expect.objectContaining({ categories: defaultCategories }),
    );
  });
  it("does not subscribe when permission is denied", async () => {
    requestPermission.mockResolvedValue("denied");
    await expect(enablePush(defaultCategories)).rejects.toThrow("blocked");
    expect(invoke).not.toHaveBeenCalled();
  });
  it("rolls back a new browser subscription if registration on the host fails", async () => {
    invoke
      .mockResolvedValueOnce({ public_key: "BA" })
      .mockRejectedValueOnce(new Error("offline"));
    await expect(enablePush(defaultCategories)).rejects.toThrow("offline");
    expect(unsubscribe).toHaveBeenCalledOnce();
  });
  it("does not claim to disable delivery while the host is unreachable", async () => {
    invoke.mockRejectedValueOnce(new Error("offline"));
    await expect(disablePush()).rejects.toThrow("offline");
    expect(unsubscribe).not.toHaveBeenCalled();
  });
  it("explains insecure origins without asking permission", () => {
    vi.stubGlobal("isSecureContext", false);
    expect(pushUnavailableReason()).toContain("HTTPS");
    expect(requestPermission).not.toHaveBeenCalled();
  });
  it("decodes URL-safe application keys", () => {
    expect([...decodeApplicationKey("BA")]).toEqual([4]);
  });
});

it("uses a valid service-worker scope for LAN origins as well as hosted device IDs", async () => {
  setRemoteViewHost("https://desktop.example:4379");
  await enablePush(defaultCategories);
  const options = vi.mocked(navigator.serviceWorker.register).mock.calls[0][1];
  expect(options?.scope).toMatch(/^\/push\/[A-Za-z0-9_-]+\/$/);
  expect(options?.scope).not.toMatch(/%2f|%5c/i);
});
it("requires an installed Home Screen app on iPhone, while allowing supported Android browsers", () => {
  const userAgent = vi.spyOn(navigator, "userAgent", "get");
  vi.stubGlobal("matchMedia", () => ({ matches: false }));
  userAgent.mockReturnValue(
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)",
  );
  expect(pushUnavailableReason()).toContain("Home Screen");
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  expect(pushUnavailableReason()).toBeNull();
  userAgent.mockReturnValue(
    "Mozilla/5.0 (Linux; Android 15) Chrome/130.0 Mobile",
  );
  vi.stubGlobal("matchMedia", () => ({ matches: false }));
  expect(pushUnavailableReason()).toBeNull();
  userAgent.mockRestore();
});
