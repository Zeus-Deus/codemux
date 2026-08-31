import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorInfo } from "@/tauri/types";

const detectEditorsMock = vi.fn();

vi.mock("@/tauri/commands", () => ({
  detectEditors: (...args: unknown[]) => detectEditorsMock(...args),
}));

import {
  _resetEditorDiscoveryForTests,
  ensureEditorsDetected,
  useEditorDiscoveryStore,
} from "./editor-discovery-store";

const EDITORS: EditorInfo[] = [
  { id: "code", name: "VS Code", command: "code" },
];

beforeEach(() => {
  _resetEditorDiscoveryForTests();
  detectEditorsMock.mockReset();
});

describe("editor discovery", () => {
  it("single-flights concurrent consumers and caches the result", async () => {
    let resolve!: (editors: EditorInfo[]) => void;
    detectEditorsMock.mockReturnValue(
      new Promise<EditorInfo[]>((done) => {
        resolve = done;
      }),
    );

    const first = ensureEditorsDetected();
    const second = ensureEditorsDetected();
    expect(first).toBe(second);
    expect(detectEditorsMock).toHaveBeenCalledTimes(1);

    resolve(EDITORS);
    await expect(first).resolves.toEqual(EDITORS);
    await expect(ensureEditorsDetected()).resolves.toEqual(EDITORS);
    expect(detectEditorsMock).toHaveBeenCalledTimes(1);
  });

  it("supports one explicit forced refresh", async () => {
    detectEditorsMock.mockResolvedValueOnce(EDITORS).mockResolvedValueOnce([]);

    await ensureEditorsDetected();
    await expect(ensureEditorsDetected({ force: true })).resolves.toEqual([]);

    expect(detectEditorsMock).toHaveBeenCalledTimes(2);
  });

  it("lets a later consumer retry after a transient failed attempt", async () => {
    detectEditorsMock
      .mockRejectedValueOnce(new Error("not available"))
      .mockResolvedValueOnce(EDITORS);

    await expect(ensureEditorsDetected()).resolves.toEqual([]);
    expect(useEditorDiscoveryStore.getState().editors).toBeNull();
    await expect(ensureEditorsDetected()).resolves.toEqual(EDITORS);

    expect(detectEditorsMock).toHaveBeenCalledTimes(2);
  });
});
