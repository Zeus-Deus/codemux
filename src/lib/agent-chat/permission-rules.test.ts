import { describe, it, expect } from "vitest";

import { buildPermissionUpdate } from "./permission-rules";

describe("buildPermissionUpdate", () => {
  it("returns undefined for the 'once' scope (single-shot allow)", () => {
    expect(
      buildPermissionUpdate("once", { toolName: "Bash" }),
    ).toBeUndefined();
  });

  it("'project' scope produces an addRules entry targeting localSettings", () => {
    expect(buildPermissionUpdate("project", { toolName: "Bash" })).toEqual([
      {
        type: "addRules",
        rules: [{ toolName: "Bash" }],
        behavior: "allow",
        destination: "localSettings",
      },
    ]);
  });

  it("'user' scope produces an addRules entry targeting userSettings", () => {
    expect(buildPermissionUpdate("user", { toolName: "Read" })).toEqual([
      {
        type: "addRules",
        rules: [{ toolName: "Read" }],
        behavior: "allow",
        destination: "userSettings",
      },
    ]);
  });

  it("omits ruleContent when not provided (matches any input for the tool)", () => {
    const update = buildPermissionUpdate("project", { toolName: "Bash" });
    const rule = (update as Array<{ rules: Array<Record<string, unknown>> }>)[0]
      .rules[0];
    expect(rule).not.toHaveProperty("ruleContent");
  });

  it("includes ruleContent when provided (Stage 7 command-specific rules)", () => {
    const update = buildPermissionUpdate("project", {
      toolName: "Bash",
      ruleContent: "git status",
    });
    expect(update).toEqual([
      {
        type: "addRules",
        rules: [{ toolName: "Bash", ruleContent: "git status" }],
        behavior: "allow",
        destination: "localSettings",
      },
    ]);
  });

  it("does not include the rule even when ruleContent is provided in 'once' scope", () => {
    expect(
      buildPermissionUpdate("once", {
        toolName: "Bash",
        ruleContent: "git status",
      }),
    ).toBeUndefined();
  });

  it("returns undefined for an unknown scope (defensive — does NOT fall through to userSettings)", () => {
    // Cast through `as any` to simulate a value squeaking past the
    // type system at a JSON boundary (e.g. a typo in a future
    // caller, or a stale persisted value). Locks the explicit-map
    // contract — the helper must NOT silently target user-wide
    // settings on an unrecognized scope.
    expect(
      buildPermissionUpdate("session" as unknown as "once", { toolName: "Bash" }),
    ).toBeUndefined();
    expect(
      buildPermissionUpdate("" as unknown as "once", { toolName: "Bash" }),
    ).toBeUndefined();
  });

  it("serializes empty toolName verbatim — UI is responsible for guarding (helper is dumb)", () => {
    // The SDK would treat this as a wildcard-ish match; the helper
    // intentionally doesn't second-guess the caller. Documents the
    // separation of concerns: if a guard is needed, it lives in the
    // UI layer where the tool name is sourced.
    const update = buildPermissionUpdate("project", { toolName: "" });
    expect(update).toEqual([
      {
        type: "addRules",
        rules: [{ toolName: "" }],
        behavior: "allow",
        destination: "localSettings",
      },
    ]);
  });

  it("preserves exotic tool names (special chars, brackets) verbatim in the rule", () => {
    // Stage 7 will produce names like "Bash(git status)" once
    // command-specific rules land; the helper must not mangle them.
    const update = buildPermissionUpdate("user", {
      toolName: "Bash(git status)",
    });
    expect(
      (update as Array<{ rules: Array<{ toolName: string }> }>)[0].rules[0]
        .toolName,
    ).toBe("Bash(git status)");
  });
});
