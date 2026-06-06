// One-shot list of the models the deployed Claude Code SDK reports
// for the current user.
//
// Opens a transient `query()` solely so the SDK can run its
// initialization handshake, awaits `supportedModels()`, then aborts
// the prompt stream and closes the query. The transient query never
// consumes any prompt — `prompt` is an async iterator that yields
// nothing until the abort fires.
//
// Why this exists: Anthropic's public `/v1/models` endpoint requires
// an API key, which Claude Code subscription / OAuth users don't
// have. The Agent SDK's `supportedModels()` works for everyone using
// Claude Code regardless of auth, and returns the live model list the
// *deployed* CLI actually accepts — so new effort levels or models
// the deployed CLI is ahead of the bundled SDK's static types still
// surface in Codemux's picker.

import {
  query,
  type SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";

export interface ListModelsInput {
  cwd: string;
  pathToClaudeCodeExecutable: string;
}

export interface ListModelsResult {
  /** Raw SDK `ModelInfo[]` — forwarded verbatim. Rust does the
   *  Codemux-specific mapping (merging with hand-maintained metadata
   *  for context windows and prompt-injected effort levels). */
  models: unknown[];
}

/** Yields nothing until aborted. Keeps the query's prompt stream open
 *  just long enough for the SDK to run its handshake and answer
 *  `supportedModels()`; the `finally` block aborts the controller so
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

export async function listModels(
  input: ListModelsInput,
): Promise<ListModelsResult> {
  const controller = new AbortController();
  const handle = query({
    prompt: emptyPromptStream(controller.signal),
    options: {
      cwd: input.cwd,
      pathToClaudeCodeExecutable: input.pathToClaudeCodeExecutable,
      env: process.env as Record<string, string | undefined>,
      // Minimal init — no project / user setting sources, no plugins,
      // no streaming chrome. Plenty for `supportedModels()` to resolve.
      settingSources: [],
      includePartialMessages: false,
    },
  });
  try {
    const models = await handle.supportedModels();
    return { models };
  } finally {
    controller.abort();
    try {
      // `Query.close()` is the documented shutdown path.
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
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
