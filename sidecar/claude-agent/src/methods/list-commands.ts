// One-shot list of the slash commands the deployed Claude Code SDK
// reports for a given working directory.
//
// Opens a transient `query()` solely so the SDK can run its
// initialization handshake, awaits `supportedCommands()`, then aborts
// the prompt stream and closes the query — the exact lifecycle
// `list-models.ts` uses. The transient query never consumes any
// prompt.
//
// Unlike the models probe, this one loads the full setting sources
// (`user`, `project`, `local`) so custom commands the user installed
// under `~/.claude/commands` or `<cwd>/.claude/commands` surface
// alongside the CLI's built-ins (`/compact`, `/init`, `/review`, …).
// That's also why `cwd` matters here: project-scoped commands are
// resolved relative to it.

import {
  query,
  type SDKUserMessage,
  type SlashCommand,
} from "@anthropic-ai/claude-agent-sdk";

export interface ListCommandsInput {
  cwd: string;
  pathToClaudeCodeExecutable: string;
}

export interface ListCommandsResult {
  /** Raw SDK `SlashCommand[]` — forwarded verbatim. Rust does the
   *  Codemux-specific mapping (dedupe + reserved-name filtering). */
  commands: SlashCommand[];
}

/** Yields nothing until aborted. Keeps the query's prompt stream open
 *  just long enough for the SDK to run its handshake and answer
 *  `supportedCommands()`; the `finally` block aborts the controller so
 *  this resolves and the iterator closes. */
function emptyPromptStream(
  signal: AbortSignal,
): AsyncIterable<SDKUserMessage> {
  return {
    async *[Symbol.asyncIterator]() {
      await new Promise<void>((resolve) => {
        if (signal.aborted) {
          resolve();
          return;
        }
        signal.addEventListener("abort", () => resolve(), { once: true });
      });
    },
  };
}

export async function listCommands(
  input: ListCommandsInput,
): Promise<ListCommandsResult> {
  const controller = new AbortController();
  const handle = query({
    prompt: emptyPromptStream(controller.signal),
    options: {
      cwd: input.cwd,
      pathToClaudeCodeExecutable: input.pathToClaudeCodeExecutable,
      env: process.env as Record<string, string | undefined>,
      // Full setting sources — custom user/project/local commands are
      // the whole point of a live probe.
      settingSources: ["user", "project", "local"],
      includePartialMessages: false,
    },
  });
  try {
    const commands = await handle.supportedCommands();
    return { commands };
  } finally {
    controller.abort();
    try {
      const maybeClose = (handle as unknown as { close?: () => unknown })
        .close;
      if (typeof maybeClose === "function") {
        await Promise.resolve(maybeClose.call(handle));
      }
    } catch (_err) {
      // Already closed or in shutdown — ignore.
    }
  }
}
