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
import {
  TTL_MS,
  migrateDisabledSkillIds,
  selectActiveSkills,
  useSkillsStore,
} from "./skills-store";

const listSkillsMock = listSkills as unknown as ReturnType<typeof vi.fn>;

function resetStore() {
  useSkillsStore.setState({
    skills: [],
    loaded: false,
    loading: false,
    error: null,
    loadedAt: 0,
    includePlugins: true,
    disabledIds: [],
    adapterErrors: [],
    inventoryCache: {},
    activeContextKey: null,
    inFlightContexts: {},
    nextRequestId: 1,
    cacheGeneration: 0,
  });
  // Persist middleware writes to localStorage; clear so a leftover
  // entry from another test doesn't bleed into the next.
  if (typeof window !== "undefined") {
    try {
      window.localStorage.removeItem("codemux:skills:v1");
    } catch {
      // jsdom should always expose localStorage; ignore otherwise.
    }
  }
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
    expect(listSkillsMock).toHaveBeenCalledWith(null, true, false);
  });

  it("caches inventories per project root", async () => {
    listSkillsMock
      .mockResolvedValueOnce([makeSkill("home")])
      .mockResolvedValueOnce([makeSkill("project")]);
    await useSkillsStore.getState().loadSkills(null);
    await useSkillsStore.getState().loadSkills(null);
    await useSkillsStore.getState().loadSkills("/some/project");
    expect(listSkillsMock).toHaveBeenCalledTimes(2);
    expect(useSkillsStore.getState().skills.map((skill) => skill.name)).toEqual([
      "project",
    ]);

    await useSkillsStore.getState().loadSkills(null);
    expect(listSkillsMock).toHaveBeenCalledTimes(2);
    expect(useSkillsStore.getState().skills.map((skill) => skill.name)).toEqual([
      "home",
    ]);
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

  it("does not let a slower previous project overwrite the active project", async () => {
    let resolveFirst: (skills: Skill[]) => void = () => {};
    listSkillsMock
      .mockReturnValueOnce(
        new Promise<Skill[]>((resolve) => {
          resolveFirst = resolve;
        }),
      )
      .mockResolvedValueOnce([makeSkill("project-b")]);

    const first = useSkillsStore.getState().loadSkills("/project-a");
    const second = useSkillsStore.getState().loadSkills("/project-b");
    await second;
    expect(useSkillsStore.getState().skills.map((skill) => skill.name)).toEqual([
      "project-b",
    ]);

    resolveFirst([makeSkill("project-a")]);
    await first;
    expect(useSkillsStore.getState().skills.map((skill) => skill.name)).toEqual([
      "project-b",
    ]);

    await useSkillsStore.getState().loadSkills("/project-a");
    expect(listSkillsMock).toHaveBeenCalledTimes(2);
    expect(useSkillsStore.getState().skills.map((skill) => skill.name)).toEqual([
      "project-a",
    ]);
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
    expect(listSkillsMock).toHaveBeenLastCalledWith(null, false, false);
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

describe("skills-store · disable toggle (Stage 5)", () => {
  beforeEach(() => {
    resetStore();
    listSkillsMock.mockReset();
  });

  it("disabledIds defaults to an empty array", () => {
    expect(useSkillsStore.getState().disabledIds).toEqual([]);
  });

  it("toggleSkillDisabled adds the id and keeps the list sorted", () => {
    useSkillsStore.getState().toggleSkillDisabled("zeta");
    useSkillsStore.getState().toggleSkillDisabled("alpha");
    useSkillsStore.getState().toggleSkillDisabled("mid");
    expect(useSkillsStore.getState().disabledIds).toEqual([
      "alpha",
      "mid",
      "zeta",
    ]);
  });

  it("toggleSkillDisabled flips back when called twice on the same id", () => {
    useSkillsStore.getState().toggleSkillDisabled("foo");
    expect(useSkillsStore.getState().disabledIds).toEqual(["foo"]);
    useSkillsStore.getState().toggleSkillDisabled("foo");
    expect(useSkillsStore.getState().disabledIds).toEqual([]);
  });

  it("selectActiveSkills returns all skills when nothing is disabled", () => {
    useSkillsStore.setState({
      skills: [makeSkill("a"), makeSkill("b")],
      disabledIds: [],
    });
    const active = selectActiveSkills(useSkillsStore.getState());
    expect(active.map((s) => s.name)).toEqual(["a", "b"]);
  });

  it("selectActiveSkills filters out disabled ids", () => {
    useSkillsStore.setState({
      skills: [makeSkill("a"), makeSkill("b"), makeSkill("c")],
      disabledIds: ["id-a", "id-c"],
    });
    const active = selectActiveSkills(useSkillsStore.getState());
    expect(active.map((s) => s.name)).toEqual(["b"]);
  });

  it("selectActiveSkills uses worktree-stable preference ids", () => {
    useSkillsStore.setState({
      skills: [{ ...makeSkill("foo"), id: "runtime-worktree-id", preferenceId: "repo-skill-id" }],
      disabledIds: ["repo-skill-id"],
    });
    expect(selectActiveSkills(useSkillsStore.getState())).toEqual([]);
  });

  it("migrates a disabled path id to the canonical preference id on discovery", async () => {
    const skill = {
      ...makeSkill("project-skill"),
      id: "legacy-path-hash",
      preferenceId: "canonical-repo-hash",
    };
    useSkillsStore.setState({ disabledIds: ["legacy-path-hash"] });
    listSkillsMock.mockResolvedValueOnce([skill]);

    await useSkillsStore.getState().loadSkills("/repo", true);

    expect(useSkillsStore.getState().disabledIds).toEqual([
      "canonical-repo-hash",
    ]);
    expect(selectActiveSkills(useSkillsStore.getState())).toEqual([]);
  });

  it("leaves unrelated disabled ids byte-for-byte unchanged", () => {
    const ids = ["unrelated"];
    expect(migrateDisabledSkillIds(ids, [makeSkill("project-skill")])).toBe(ids);
  });

  it("disabled state persists to localStorage and rehydrates on a fresh import", async () => {
    useSkillsStore.getState().toggleSkillDisabled("foo");
    useSkillsStore.getState().setIncludePlugins(false);

    // Simulate a reload by reading the persisted JSON directly. The
    // persist middleware writes synchronously after the set call so
    // localStorage is up to date by the time we read.
    const raw = window.localStorage.getItem("codemux:skills:v1");
    expect(raw).not.toBeNull();
    const parsed = JSON.parse(raw!);
    expect(parsed.state).toMatchObject({
      disabledIds: ["foo"],
      includePlugins: false,
    });
    // Loading/loaded/skills are intentionally NOT persisted.
    expect(parsed.state.skills).toBeUndefined();
    expect(parsed.state.loadedAt).toBeUndefined();
  });
});
