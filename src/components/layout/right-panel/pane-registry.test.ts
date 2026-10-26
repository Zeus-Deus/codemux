import { describe, expect, it } from "vitest";

import {
  CONDITIONAL_PANES,
  PANE_REGISTRY,
  baseName,
  docPaneId,
  docPanePath,
  isCorePane,
  isPluginPane,
  paneMeta,
} from "./pane-registry";

describe("pane registry", () => {
  it("declares every pane the deck can host, with unique ids", () => {
    const ids = PANE_REGISTRY.map((meta) => meta.id);
    expect(new Set(ids).size).toBe(ids.length);
    // The four original right-panel tabs must all still be panes.
    expect(ids).toEqual(expect.arrayContaining(["files", "changes", "review", "tasks"]));
  });

  // The browser is a deck pane, not a jump-out to a workspace pane: it
  // must be in the registry so the `+` menu builds it like any other, and
  // it must NOT be conditional (there is always a browser to open).
  it("declares the browser as an always-available pane", () => {
    expect(PANE_REGISTRY.map((meta) => meta.id)).toContain("browser");
    expect(paneMeta("browser")?.label).toBe("Browser");
    expect(paneMeta("browser")?.conditional).toBeUndefined();
  });

  it("keeps dynamic workflows independent of native orchestration", () => {
    expect(paneMeta("workflows")?.label).toBe("Workflows");
    expect(paneMeta("workflows")?.conditional).toBeUndefined();
    expect(paneMeta("orchestration")?.conditional).toBe(true);
  });

  it("lists exactly the availability-gated panes as conditional", () => {
    expect([...CONDITIONAL_PANES]).toEqual(["tasks", "subagents", "side-chat", "orchestration"]);
  });

  it("resolves metadata by id and returns null for an unknown one", () => {
    expect(paneMeta("changes")?.label).toBe("Changes");
    expect(paneMeta("nope" as never)).toBeNull();
  });
});

describe("doc pane ids", () => {
  it("round-trips a file path through the pane id", () => {
    const id = docPaneId("/p/src/AGENTS.md");
    expect(docPanePath(id)).toBe("/p/src/AGENTS.md");
    expect(isCorePane(id)).toBe(false);
  });

  // Paths containing a colon must survive — only the leading `doc:` is
  // the marker, so the payload is taken by prefix length, not by split.
  it("keeps colons inside the path intact", () => {
    const id = docPaneId("/p/weird:name.md");
    expect(docPanePath(id)).toBe("/p/weird:name.md");
  });

  it("treats a fixed id as a core pane with no path", () => {
    expect(docPanePath("changes")).toBeNull();
    expect(isCorePane("changes")).toBe(true);
  });
});

describe("breadcrumb paths", () => {
  it("takes the last segment for a tab label", () => {
    expect(baseName("/p/src/lib/utils.ts")).toBe("utils.ts");
    expect(baseName("/p/")).toBe("p");
  });
});

describe("plugin pane IDs", () => {
  it("accepts full isolated installation ids without widening renderer plugin ids", () => {
    const installationId = `plugin-${"a".repeat(64)}`;
    const pane = `plugin:@${installationId}:view.inspector` as const;
    expect(isPluginPane(pane)).toBe(true);
    expect(isCorePane(pane)).toBe(false);
    expect(isPluginPane(`plugin:${installationId}:view.inspector`)).toBe(false);
    expect(isPluginPane("plugin:@../escape:view.inspector")).toBe(false);
  });

  it("does not classify plugin panes, empty sentinels or unknown IDs as core", () => {
    expect(isPluginPane("plugin:project-brief:brief")).toBe(true);
    expect(isCorePane("plugin:project-brief:brief")).toBe(false);
    expect(isCorePane("unknown" as never)).toBe(false);
    expect(isPluginPane("plugin:../escape:brief")).toBe(false);
  });
});
