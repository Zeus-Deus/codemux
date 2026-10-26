import { describe, expect, it } from "vitest";
import { pluginKnownProjects } from "./known-projects";

describe("pluginKnownProjects", () => {
  it("lists every project of open and archived workspaces once, by the plugin project id", () => {
    const projects = pluginKnownProjects({
      workspaces: [
        { project_uid: "uid-app", project_root: "/code/app", cwd: "/code/app" },
        { project_uid: "uid-app", project_root: "/code/app", cwd: "/code/app-wt/feature" },
        { project_uid: null, project_root: null, cwd: "/home/u" },
      ],
      archived: [
        { project_uid: "uid-app", project_root: "/code/app", cwd: "/code/app-wt/old" },
        { project_uid: null, project_root: "/code/legacy", cwd: "/code/legacy" },
      ],
      homeDir: "/home/u",
    });
    expect(projects).toEqual([
      { id: "uid-app", title: "app", root: "/code/app", archivedOnly: false },
      { id: "/home/u", title: "Home", root: "/home/u", archivedOnly: false },
      { id: "/code/legacy", title: "legacy", root: "/code/legacy", archivedOnly: true },
    ]);
  });

  it("is never cut to a recent few", () => {
    const archived = Array.from({ length: 40 }, (_, index) => ({
      project_uid: null,
      project_root: `/code/p${index}`,
      cwd: `/code/p${index}`,
    }));
    expect(pluginKnownProjects({ workspaces: [], archived, homeDir: null })).toHaveLength(40);
  });
});
