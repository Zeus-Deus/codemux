import { describe, it, expect } from "vitest";

import {
  buildPermissionUpdate,
  formatPermissionRule,
  suggestPermissionRule,
} from "./permission-rules";

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

  it("'session' scope keeps the rule for the provider session only", () => {
    expect(
      buildPermissionUpdate("session", { toolName: "Bash", ruleContent: "npm test:*" }),
    ).toEqual([
      {
        type: "addRules",
        rules: [{ toolName: "Bash", ruleContent: "npm test:*" }],
        behavior: "allow",
        destination: "session",
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
      buildPermissionUpdate("bogus" as unknown as "once", { toolName: "Bash" }),
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

describe("formatPermissionRule", () => {
  it("prints the rule the way settings store it", () => {
    expect(formatPermissionRule({ toolName: "Bash" })).toBe("Bash");
    expect(
      formatPermissionRule({ toolName: "Bash", ruleContent: "git status:*" }),
    ).toBe("Bash(git status:*)");
  });
});

describe("suggestPermissionRule", () => {
  const bash = (command: string) =>
    suggestPermissionRule("Bash", { command });

  it("scopes a Bash command to its program and subcommands", () => {
    expect(bash("git status")).toEqual({ toolName: "Bash", ruleContent: "git status:*" });
    expect(bash("npm run build --watch")).toEqual({
      toolName: "Bash",
      ruleContent: "npm run build:*",
    });
    expect(bash("cargo check -j 2")).toEqual({ toolName: "Bash", ruleContent: "cargo check:*" });
    expect(bash("ls -la")).toEqual({ toolName: "Bash", ruleContent: "ls:*" });
    expect(bash("cat README.md")).toEqual({ toolName: "Bash", ruleContent: "cat:*" });
    expect(bash("uv tool list")).toEqual({ toolName: "Bash", ruleContent: "uv tool list:*" });
  });

  it("offers no scoped rule for chained, redirected or env-prefixed commands", () => {
    expect(bash("cd src && rm -rf build")).toBeNull();
    expect(bash("ls | head")).toBeNull();
    expect(bash("echo hi > out.txt")).toBeNull();
    expect(bash("echo $(whoami)")).toBeNull();
    expect(bash("FOO=1 npm test")).toBeNull();
    expect(bash("   ")).toBeNull();
  });

  it("offers no scoped rule for shells, interpreters and wrappers, which run anything", () => {
    expect(bash("bash -c 'rm -rf /'")).toBeNull();
    expect(bash("sh scripts/x.sh")).toBeNull();
    expect(bash("python manage.py migrate")).toBeNull();
    expect(bash("python3.12 -m pytest")).toBeNull();
    expect(bash("/usr/bin/node build.js")).toBeNull();
    expect(bash("sudo apt install jq")).toBeNull();
    expect(bash("env npm test")).toBeNull();
    expect(bash("xargs rm")).toBeNull();
    expect(bash("timeout 10 cargo test")).toBeNull();
    // Package runners fetch and run any package, flags first or not.
    expect(bash("npx -y create-vite")).toBeNull();
    expect(bash("bunx cowsay@latest")).toBeNull();
    expect(bash("uvx ruff check")).toBeNull();
    expect(bash("uv tool run ruff")).toBeNull();
    expect(bash("/usr/bin/uv tool run ruff check")).toBeNull();
    expect(bash("npm exec -- tsc")).toBeNull();
    expect(bash("pnpm dlx @scope/pkg")).toBeNull();
    expect(bash("npm --prefix app exec cowsay")).toBeNull();
    expect(bash("uv --directory app tool run ruff")).toBeNull();
    expect(bash("uv tool --quiet run ruff")).toBeNull();
    expect(bash("uv tool --directory app run ruff")).toBeNull();
    expect(bash("uv tool")).toBeNull();
    expect(bash("yarn")).toBeNull();
    // A program that merely starts with an interpreter's name is fine.
    expect(bash("shellcheck x.sh")).toEqual({ toolName: "Bash", ruleContent: "shellcheck:*" });
  });

  it("offers no rule whose content would break the Tool(content) syntax", () => {
    expect(bash("(cd src)")).toBeNull();
    expect(
      suggestPermissionRule("Edit", { file_path: "/home/u/my (copy)/a.ts" }),
    ).toBeNull();
  });

  it("scopes file tools to the file's directory, using Edit rules for every editor", () => {
    expect(suggestPermissionRule("Edit", { file_path: "/repo/src/a.ts" })).toEqual({
      toolName: "Edit",
      ruleContent: "//repo/src/**",
    });
    expect(suggestPermissionRule("Write", { file_path: "/repo/b.ts" })).toEqual({
      toolName: "Edit",
      ruleContent: "//repo/**",
    });
    expect(suggestPermissionRule("Read", { file_path: "/etc/hosts" })).toEqual({
      toolName: "Read",
      ruleContent: "//etc/**",
    });
    expect(suggestPermissionRule("Edit", { file_path: "relative/a.ts" })).toBeNull();
    expect(suggestPermissionRule("Edit", { file_path: "/a.ts" })).toBeNull();
  });

  it("scopes WebFetch to the URL's domain", () => {
    expect(
      suggestPermissionRule("WebFetch", { url: "https://docs.rs/serde/latest" }),
    ).toEqual({ toolName: "WebFetch", ruleContent: "domain:docs.rs" });
    expect(suggestPermissionRule("WebFetch", { url: "not a url" })).toBeNull();
  });

  it("returns null for tools without a narrower rule", () => {
    expect(suggestPermissionRule("Glob", { pattern: "**/*.ts" })).toBeNull();
    expect(suggestPermissionRule("Bash", null)).toBeNull();
  });
});
