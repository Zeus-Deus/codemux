// One-shot read of the claude.ai plan rate limits, for the Usage page.
//
// Opens a transient `query()` that never sends a prompt (the lifecycle
// `list-models.ts` uses), then asks the CLI for its structured `/usage`
// data through the `get_usage` control request. The CLI answers from
// its own login, so the sidecar never touches a credential or calls an
// Anthropic endpoint itself.
//
// The pinned SDK predates the typed `usage_EXPERIMENTAL_*()` wrapper,
// but its `Query` already routes arbitrary control requests through
// `request()`; the deployed CLI is what implements `get_usage`. A CLI
// too old to know the subtype rejects it, and the caller treats that as
// "no limits available" rather than as an empty quota.

import { query, type SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";

export interface GetUsageInput {
  cwd: string;
  pathToClaudeCodeExecutable: string;
}

/** The control-request surface `Query` exposes at runtime. */
interface ControlRequester {
  request(request: {
    subtype: string;
    skip_behaviors?: boolean;
  }): Promise<{ response?: unknown }>;
}

function hasControlRequest(handle: unknown): handle is ControlRequester {
  return (
    typeof handle === "object" &&
    handle !== null &&
    typeof (handle as { request?: unknown }).request === "function"
  );
}

/** Yields nothing until aborted, so the CLI initializes but never runs a turn. */
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

/** Returns the raw `get_usage` response; Rust maps it to quota windows. */
export async function getUsage(input: GetUsageInput): Promise<unknown> {
  const controller = new AbortController();
  const handle = query({
    prompt: emptyPromptStream(controller.signal),
    options: {
      cwd: input.cwd,
      pathToClaudeCodeExecutable: input.pathToClaudeCodeExecutable,
      env: process.env as Record<string, string | undefined>,
      settingSources: [],
      includePartialMessages: false,
    },
  });
  try {
    if (!hasControlRequest(handle)) {
      throw new Error("this Claude Agent SDK cannot send control requests");
    }
    // `skip_behaviors` skips the CLI's scan of the last week of
    // transcripts; a usage meter only needs the plan windows.
    const reply = await handle.request({
      subtype: "get_usage",
      skip_behaviors: true,
    });
    return reply.response ?? null;
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
