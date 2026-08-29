import { describe, expect, it } from "vitest";

import {
  adoptableSessionGroup,
  buildAdoptableSessionItems,
  buildWidenScopeItem,
  externalSessionFolderLabel,
  externalSessionRowId,
  RESUME_GROUP_CHECKOUT,
  RESUME_GROUP_EXISTING,
  RESUME_GROUP_OTHER,
  RESUME_WIDEN_SCOPE_ITEM_ID,
  toExternalAgentSession,
} from "./external-sessions";
import type { AdoptableAgentSession } from "@/tauri/commands";

function makeSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return {
    session_id: "sess-1",
    title: "Refactor the splitter",
    cwd: "/projects/codemux",
    git_branch: "main",
    last_modified: "2026-04-24T12:00:00.000Z",
    created_at: "2026-04-24T10:00:00.000Z",
    file_size: 4096,
    title_source: "summary",
    existing_thread_id: null,
    same_repo: true,
    ...overrides,
  };
}

describe("adoptableSessionGroup", () => {
  it("files a same-repo session under the current checkout", () => {
    expect(adoptableSessionGroup(makeSession())).toBe(RESUME_GROUP_CHECKOUT);
  });

  it("files an unrelated project separately so it is never a silent jump", () => {
    expect(adoptableSessionGroup(makeSession({ same_repo: false }))).toBe(
      RESUME_GROUP_OTHER,
    );
  });

  it("prefers 'already in Codemux' over locality", () => {
    expect(
      adoptableSessionGroup(
        makeSession({ existing_thread_id: "thread-9", same_repo: false }),
      ),
    ).toBe(RESUME_GROUP_EXISTING);
  });
});

describe("buildAdoptableSessionItems", () => {
  it("orders checkout rows first, then adopted, then other projects", () => {
    const items = buildAdoptableSessionItems({
      sessions: [
        makeSession({ session_id: "other", same_repo: false }),
        makeSession({ session_id: "adopted", existing_thread_id: "t-1" }),
        makeSession({ session_id: "here" }),
      ],
    });
    expect(items.map((item) => item.group)).toEqual([
      RESUME_GROUP_CHECKOUT,
      RESUME_GROUP_EXISTING,
      RESUME_GROUP_OTHER,
    ]);
    expect(items[0].id).toBe(externalSessionRowId("here"));
  });

  it("sorts within a group by most recently touched", () => {
    const items = buildAdoptableSessionItems({
      sessions: [
        makeSession({
          session_id: "older",
          last_modified: "2026-04-20T09:00:00.000Z",
        }),
        makeSession({
          session_id: "newer",
          last_modified: "2026-04-24T09:00:00.000Z",
        }),
      ],
    });
    expect(items.map((item) => item.id)).toEqual([
      externalSessionRowId("newer"),
      externalSessionRowId("older"),
    ]);
  });

  it("marks an already-adopted row as a switch, never a second adoption", () => {
    const [item] = buildAdoptableSessionItems({
      sessions: [makeSession({ existing_thread_id: "thread-9" })],
    });
    expect(item.command).toBe("switch");
    expect(item.description).toMatch(/already in codemux/i);
  });

  it("spells out the directory an unrelated project would open in", () => {
    const [item] = buildAdoptableSessionItems({
      sessions: [makeSession({ same_repo: false, cwd: "/projects/ledger" })],
    });
    expect(item.command).toBe("other project");
    expect(item.description).toContain("/projects/ledger");
  });

  it("renders a relative timestamp against an injected clock", () => {
    const [item] = buildAdoptableSessionItems({
      sessions: [makeSession({ last_modified: "2026-04-24T11:00:00.000Z" })],
      now: new Date("2026-04-24T12:00:00.000Z"),
    });
    // The adornment is a React element; assert on its rendered strings.
    expect(JSON.stringify(item.rightAdornment)).toContain("1 hour ago");
  });
});

describe("buildWidenScopeItem", () => {
  it("is a scope control, not a discovered session", () => {
    const item = buildWidenScopeItem();
    expect(item.id).toBe(RESUME_WIDEN_SCOPE_ITEM_ID);
    expect(item.id.startsWith("external-session:")).toBe(true);
    expect(externalSessionRowId("sess-1")).not.toBe(item.id);
  });
});

describe("toExternalAgentSession", () => {
  it("drops the Codemux-side decorations before the adopt call", () => {
    const payload = toExternalAgentSession(
      makeSession({ existing_thread_id: "t-1", same_repo: false }),
    );
    expect(payload).not.toHaveProperty("existing_thread_id");
    expect(payload).not.toHaveProperty("same_repo");
    expect(payload.session_id).toBe("sess-1");
    expect(payload.cwd).toBe("/projects/codemux");
  });
});

describe("externalSessionFolderLabel", () => {
  it("takes the last segment regardless of separator or trailing slash", () => {
    expect(externalSessionFolderLabel("/projects/codemux/")).toBe("codemux");
    expect(externalSessionFolderLabel("C:\\work\\ledger")).toBe("ledger");
  });
});
