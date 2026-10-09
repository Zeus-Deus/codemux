import { createAgentPlatform, JsonlLocalAgentStore } from "@cursor/sdk/bundled";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { object } from "./contract";

/** Exercise the shipped native assets without creating an agent or model run. */
export async function probeCursorNatives(): Promise<void> {
  const owned = await mkdtemp(join(tmpdir(), "codemux-cursor-native-probe-"));
  let release: (() => Promise<void>) | undefined;
  try {
    const nativeName = `@cursor/sdk-${process.platform}-${process.arch}`;
    let nativeDir: string | undefined;
    // These are the same executable/argv ancestor roots used by SDK 1.0.37's
    // vendored-helper resolver. The probe's workspace is outside those roots.
    for (const entry of [process.argv[1], process.execPath]) {
      if (!entry) continue;
      let current = dirname(resolve(entry));
      while (current !== dirname(current)) {
        const candidate = join(current, "node_modules", nativeName);
        try {
          const metadata: unknown = JSON.parse(await readFile(join(candidate, "package.json"), "utf8"));
          if (!object(metadata) || metadata.name !== nativeName || metadata.version !== "1.0.37") {
            throw new Error("Cursor native package version mismatch");
          }
          nativeDir = candidate; break;
        } catch (error) {
          if (!object(error) || error.code !== "ENOENT") throw error;
        }
        current = dirname(current);
      }
      if (nativeDir) break;
    }
    if (!nativeDir) throw new Error("Bundled Cursor native package is unavailable");

    const rg = Bun.spawn([join(nativeDir, "bin", process.platform === "win32" ? "rg.exe" : "rg"), "--version"], { stdout: "pipe", stderr: "pipe" });
    const rgOutput = await new Response(rg.stdout).text();
    if (await rg.exited !== 0 || !rgOutput.startsWith("ripgrep ")) throw new Error("Bundled Cursor ripgrep failed");

    const require = createRequire(import.meta.url);
    const Parser: unknown = require(join(nativeDir, "vendor", "tree-sitter", "index.js"));
    const bash: unknown = require(join(nativeDir, "vendor", "tree-sitter-bash", "index.js"));
    if (typeof Parser !== "function") throw new Error("Bundled Cursor parser constructor is unavailable");
    const parser: unknown = Reflect.construct(Parser, []);
    if (!object(parser) || typeof parser.setLanguage !== "function" || typeof parser.parse !== "function") {
      throw new Error("Bundled Cursor parser interface is unavailable");
    }
    parser.setLanguage(bash);
    const tree: unknown = parser.parse("printf 'native readiness only'\n");
    if (!object(tree) || !object(tree.rootNode) || tree.rootNode.type !== "program") throw new Error("Bundled Cursor native parsing failed");

    const platform = await createAgentPlatform({ localStore: new JsonlLocalAgentStore(join(owned, "store")), workspaceRef: owned });
    release = await platform.prewarmLocalWorkspace({
      // Explicitly empty; prewarm does not use the SDK's saved-auth fallback.
      apiKey: "", tools: ["mcp"], disallowedTools: ["task"], agents: {}, mcpServers: {},
      local: { cwd: owned, settingSources: [], enableAgentRetries: false },
    });
    process.stdout.write(JSON.stringify({ protocolVersion: 1, cursorAdapterVersion: "cursor-sdk-1.0.37",
      nativePackageVersion: "1.0.37", ripgrepReady: true, parserReady: true, workspaceReady: true,
      modelTurns: 0 }) + "\n");
  } finally {
    await release?.();
    await rm(owned, { recursive: true, force: true });
  }
}
