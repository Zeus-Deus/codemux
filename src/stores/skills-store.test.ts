import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Skill } from "@/tauri/commands";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listSkills: vi.fn(),
  };
});

import { listSkills } from "@/tauri/commands";
import { TTL_MS, useSkillsStore } from "./skills-store";

const listSkillsMock = listSkills as unknown as ReturnType<typeof vi.fn>;

function resetStore() {
  useSkillsStore.setState({
    skills: [],
    loaded: false,
    loading: false,
    error: null,
    loadedAt: 0,
    includePlugins: true,
  });
}

function makeSkill(name: string): Skill {
  return {
    id: `id-${name}`,
    name,
    description: `desc-${name}`,
    provider: "claude",
    scope: "user",
    skillDir: `/skills/${name}`,
    filePath: `/skills/${name}/SKILL.md`,
    body: "",
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
  };
}

describe("skills-store", () => {
  beforeEach(() => {
    resetStore();
    listSkillsMock.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts empty with includePlugins=true", () => {
    const s = useSkillsStore.getState();
    expect(s.skills).toEqual([]);
    expect(s.loaded).toBe(false);
    expect(s.loading).toBe(false);
    expect(s.error).toBeNull();
    expect(s.includePlugins).toBe(true);
    expect(s.loadedAt).toBe(0);
  });

  it("loadSkills populates the store and stamps loadedAt", async () => {
    listSkillsMock.mockResolvedValue([makeSkill("a"), makeSkill("b")]);
    const before = Date.now();

    await useSkillsStore.getState().loadSkills(null);

    const s = useSkillsStore.getState();
    expect(s.skills.map((sk) => sk.name)).toEqual(["a", "b"]);
    expect(s.loaded).toBe(true);
    expect(s.loading).toBe(false);
    expect(s.loadedAt).toBeGreaterThanOrEqual(before);
    expect(listSkillsMock).toHaveBeenCalledWith(null, true);
  });

  it("subsequent loadSkills within TTL is a no-op", async () => {
    listSkillsMock.mockResolvedValue([makeSkill("a")]);
    await useSkillsStore.getState().loadSkills(null);
    await useSkillsStore.getState().loadSkills(null);
    await useSkillsStore.getState().loadSkills("/some/project");
    expect(listSkillsMock).toHaveBeenCalledTimes(1);
  });

  it("force=true bypasses TTL", async () => {
    listSkillsMock.mockResolvedValue([makeSkill("a")]);
    await useSkillsStore.getState().loadSkills(null);
    await useSkillsStore.getState().loadSkills(null, true);
    expect(listSkillsMock).toHaveBeenCalledTimes(2);
  });

  it("re-fetches once the TTL has expired", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(0));
    listSkillsMock.mockResolvedValue([makeSkill("a")]);

    await useSkillsStore.getState().loadSkills(null);
    expect(listSkillsMock).toHaveBeenCalledTimes(1);

    vi.setSystemTime(new Date(TTL_MS + 1));
    await useSkillsStore.getState().loadSkills(null);
    expect(listSkillsMock).toHaveBeenCalledTimes(2);
  });

  it("concurrent loadSkills calls dedupe (loading guard)", async () => {
    let resolveFirst: (skills: Skill[]) => void = () => {};
    listSkillsMock.mockReturnValueOnce(
      new Promise<Skill[]>((resolve) => {
        resolveFirst = resolve;
      }),
    );
    listSkillsMock.mockResolvedValueOnce([makeSkill("late")]);

    const first = useSkillsStore.getState().loadSkills(null);
    const second = useSkillsStore.getState().loadSkills(null);

    expect(useSkillsStore.getState().loading).toBe(true);
    resolveFirst([makeSkill("a")]);
    await Promise.all([first, second]);

    // Second call returned without invoking listSkills again because
    // loading was true.
    expect(listSkillsMock).toHaveBeenCalledTimes(1);
    expect(useSkillsStore.getState().skills.map((s) => s.name)).toEqual(["a"]);
  });

  it("setIncludePlugins(false) invalidates the cache", async () => {
    listSkillsMock.mockResolvedValue([makeSkill("a")]);
    await useSkillsStore.getState().loadSkills(null);
    expect(useSkillsStore.getState().loaded).toBe(true);

    useSkillsStore.getState().setIncludePlugins(false);
    expect(useSkillsStore.getState().includePlugins).toBe(false);
    expect(useSkillsStore.getState().loaded).toBe(false);
    expect(useSkillsStore.getState().loadedAt).toBe(0);

    listSkillsMock.mockResolvedValue([makeSkill("b")]);
    await useSkillsStore.getState().loadSkills(null);
    expect(listSkillsMock).toHaveBeenLastCalledWith(null, false);
  });

  it("setIncludePlugins to the same value is a no-op", async () => {
    listSkillsMock.mockResolvedValue([makeSkill("a")]);
    await useSkillsStore.getState().loadSkills(null);
    const loadedAt = useSkillsStore.getState().loadedAt;

    useSkillsStore.getState().setIncludePlugins(true); // unchanged
    expect(useSkillsStore.getState().loaded).toBe(true);
    expect(useSkillsStore.getState().loadedAt).toBe(loadedAt);
  });

  it("invalidate clears cache without re-fetching", async () => {
    listSkillsMock.mockResolvedValue([makeSkill("a")]);
    await useSkillsStore.getState().loadSkills(null);

    useSkillsStore.getState().invalidate();
    const s = useSkillsStore.getState();
    expect(s.loaded).toBe(false);
    expect(s.loadedAt).toBe(0);
    // Skills array intentionally preserved so the popup still shows
    // results until the next load completes.
    expect(s.skills.map((sk) => sk.name)).toEqual(["a"]);
    expect(listSkillsMock).toHaveBeenCalledTimes(1);
  });

  it("error path: skills preserved, error set, loading false", async () => {
    listSkillsMock.mockResolvedValueOnce([makeSkill("a")]);
    await useSkillsStore.getState().loadSkills(null);

    useSkillsStore.getState().invalidate();
    listSkillsMock.mockRejectedValueOnce(new Error("boom"));
    await useSkillsStore.getState().loadSkills(null, true);

    const s = useSkillsStore.getState();
    expect(s.loading).toBe(false);
    expect(s.error).toBe("boom");
    expect(s.loaded).toBe(false);
    // Previously-loaded skills survive a failed re-fetch — UX continuity.
    expect(s.skills.map((sk) => sk.name)).toEqual(["a"]);
  });

  it("error path normalises non-Error throws to a string", async () => {
    listSkillsMock.mockRejectedValueOnce("a string error");
    await useSkillsStore.getState().loadSkills(null);
    expect(useSkillsStore.getState().error).toBe("a string error");
  });

  it("error is cleared when a subsequent load succeeds", async () => {
    listSkillsMock.mockRejectedValueOnce(new Error("first failure"));
    await useSkillsStore.getState().loadSkills(null);
    expect(useSkillsStore.getState().error).toBe("first failure");

    listSkillsMock.mockResolvedValueOnce([makeSkill("a")]);
    await useSkillsStore.getState().loadSkills(null, true);
    expect(useSkillsStore.getState().error).toBeNull();
  });
});
