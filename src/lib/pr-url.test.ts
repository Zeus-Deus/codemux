import { beforeEach, describe, expect, it, vi } from "vitest";

import { _resetPrLinkIndex, parsePrUrl, publishPrLinkIndex } from "./pr-url";
import type { PrRow } from "./pr-overview";

const openUrl = vi.hoisted(() => vi.fn(() => Promise.resolve()));
const toastInfo = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));
vi.mock("@/lib/toast", () => ({
  toast: { info: toastInfo, error: toastError, success: vi.fn(), warning: vi.fn() },
}));

import { openExternalUrl } from "./open-url";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";

const row = (over: Partial<PrRow> = {}): PrRow => ({
  number: 285,
  title: "Fix the installer",
  author: "mock-dev",
  head_branch: "fix/installer",
  is_draft: false,
  additions: 10,
  deletions: 2,
  review_decision: null,
  checks: "passing",
  review_requested_from: [],
  updated_at: null,
  url: "https://github.com/example/codemux/pull/285",
  projectRoot: "/home/dev/projects/codemux",
  repo: "example/codemux",
  providerKind: "github",
  ...over,
});

function setCustomHosts(hosts: Record<string, string>, inBrowser = false) {
  useSyncedSettingsStore.setState((s) => ({
    settings: {
      ...s.settings,
      source_control: { custom_hosts: hosts, open_pr_links_in_browser: inBrowser },
    },
  }));
}

describe("parsePrUrl", () => {
  it("reads a GitHub pull request", () => {
    expect(parsePrUrl("https://github.com/example/codemux/pull/285")).toEqual({
      kind: "github",
      host: "github.com",
      slug: "example/codemux",
      number: 285,
    });
  });

  it("tolerates a sub-page and a fragment", () => {
    expect(parsePrUrl("https://github.com/example/codemux/pull/285/files#r12")?.number).toBe(
      285,
    );
  });

  it("reads a merge request on a declared self-hosted instance", () => {
    expect(
      parsePrUrl("https://git.acme.internal/acme/group/vexis/-/merge_requests/88", {
        "git.acme.internal": "gitlab",
      }),
    ).toEqual({
      kind: "gitlab",
      host: "git.acme.internal",
      // Subgroups are real; the last two segments identify the project.
      slug: "group/vexis",
      number: 88,
    });
  });

  it("ignores a host nobody has declared", () => {
    expect(
      parsePrUrl("https://git.unknown.example/acme/vexis/-/merge_requests/88"),
    ).toBeNull();
  });

  it("ignores URLs on a known host that aren't pull requests", () => {
    expect(parsePrUrl("https://github.com/example/codemux/issues/285")).toBeNull();
    expect(parsePrUrl("https://github.com/example/codemux")).toBeNull();
    expect(parsePrUrl("not a url")).toBeNull();
  });
});

describe("openExternalUrl", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    _resetPrLinkIndex();
    setCustomHosts({});
    useUIStore.setState({ showPullRequests: false, pendingPrSelection: null });
  });

  it("routes a pull request in an open project to the page", async () => {
    publishPrLinkIndex([row()]);

    const outcome = await openExternalUrl("https://github.com/example/codemux/pull/285");

    expect(outcome).toBe("in-app");
    expect(openUrl).not.toHaveBeenCalled();
    expect(useUIStore.getState().showPullRequests).toBe(true);
    expect(useUIStore.getState().pendingPrSelection).toEqual({
      projectRoot: "/home/dev/projects/codemux",
      number: 285,
    });
  });

  it("routes a merge request on a custom host", async () => {
    setCustomHosts({ "gitlab.example.com": "gitlab" });
    publishPrLinkIndex([
      row({
        number: 88,
        repo: "acme/vexis",
        providerKind: "gitlab",
        projectRoot: "/home/dev/projects/vexis",
        url: "https://gitlab.example.com/acme/vexis/-/merge_requests/88",
      }),
    ]);

    const outcome = await openExternalUrl(
      "https://gitlab.example.com/acme/vexis/-/merge_requests/88",
    );

    expect(outcome).toBe("in-app");
    expect(useUIStore.getState().pendingPrSelection).toEqual({
      projectRoot: "/home/dev/projects/vexis",
      number: 88,
    });
  });

  it("shift-click keeps the browser", async () => {
    publishPrLinkIndex([row()]);

    const outcome = await openExternalUrl(
      "https://github.com/example/codemux/pull/285",
      { event: { shiftKey: true } },
    );

    expect(outcome).toBe("browser");
    expect(openUrl).toHaveBeenCalledWith("https://github.com/example/codemux/pull/285");
    expect(useUIStore.getState().showPullRequests).toBe(false);
  });

  it("the setting turns interception off entirely", async () => {
    setCustomHosts({}, true);
    publishPrLinkIndex([row()]);

    const outcome = await openExternalUrl("https://github.com/example/codemux/pull/285");

    expect(outcome).toBe("browser");
    expect(openUrl).toHaveBeenCalled();
    expect(useUIStore.getState().showPullRequests).toBe(false);
  });

  it("falls back to the browser, and says why, for a repository nobody has open", async () => {
    publishPrLinkIndex([row()]);

    const outcome = await openExternalUrl("https://github.com/other/thing/pull/9");

    expect(outcome).toBe("browser");
    expect(openUrl).toHaveBeenCalledWith("https://github.com/other/thing/pull/9");
    expect(toastInfo).toHaveBeenCalledWith(
      "Opening this pull request in the browser",
      expect.objectContaining({
        description: expect.stringContaining("other/thing"),
      }),
    );
  });

  it("leaves an ordinary link alone", async () => {
    publishPrLinkIndex([row()]);

    const outcome = await openExternalUrl("https://docs.codemux.org/installation");

    expect(outcome).toBe("browser");
    expect(toastInfo).not.toHaveBeenCalled();
    expect(openUrl).toHaveBeenCalledWith("https://docs.codemux.org/installation");
  });
});
