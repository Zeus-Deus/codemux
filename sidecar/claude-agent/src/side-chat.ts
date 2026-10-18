// SideChat — one ephemeral `/btw` fork of a main Claude session.
//
// The fork resumes the main session's transcript with `forkSession: true`
// (a new session id, the source transcript untouched) and
// `persistSession: false` (no transcript of its own is written), so it can
// read everything the main thread said without adding anything back.
// Verified against Claude Code 2.1.299 / agent SDK 0.2.114: the fork keeps
// the context, writes no `.jsonl`, and the source file's bytes are unchanged.
//
// Follow-ups continue the same live streaming-input query. A fork that is
// not persisted cannot be resumed later, so the query is never rebuilt:
// when it ends, the side chat is over.
//
// Read-only enforcement is layered: only read/search tools are offered,
// edit/shell/agent tools are disallowed by name, hooks and setting sources
// are off, no MCP servers are loaded, and `canUseTool` denies anything that
// still asks.

import type {
  CanUseTool,
  Options,
  SDKMessage,
  SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";

import { AsyncPromptQueue } from "./async-queue.ts";
import type { EventEmitter, QueryFactory } from "./session.ts";

export interface SideChatStartInput {
  sideId: string;
  cwd: string;
  /** SDK session id of the main chat to fork. */
  resume: string;
  pathToClaudeCodeExecutable: string;
  model?: string;
  /** The main chat's reasoning effort, applied the way a main session
   *  applies it (`start-session`): `ultracode` runs as `xhigh` plus the
   *  `ultracode` setting, `ultrathink` prefixes each question. */
  effort?: string;
  /** The first side question. */
  text: string;
  appendSystemPrompt?: string;
}

/** Tools a side chat may use: reading and searching the workspace. */
export const SIDE_CHAT_TOOLS = ["Read", "Grep", "Glob"];

/** Never offered, even if a setting or plugin would add them back. */
export const SIDE_CHAT_DISALLOWED_TOOLS = [
  "Bash",
  "BashOutput",
  "KillShell",
  "Edit",
  "MultiEdit",
  "Write",
  "NotebookEdit",
  "Agent",
  "Task",
  "Workflow",
  "WebFetch",
  "WebSearch",
  "ExitPlanMode",
  "TodoWrite",
];

const READ_ONLY = new Set(SIDE_CHAT_TOOLS);

/** Deny everything outside the read-only set. Read tools are allowed
 *  without a prompt: the side pane has no approval UI. */
export const sideChatCanUseTool: CanUseTool = async (toolName, input) =>
  READ_ONLY.has(toolName)
    ? { behavior: "allow", updatedInput: input }
    : {
        behavior: "deny",
        message: `${toolName} is unavailable in a read-only side chat.`,
      };

export function buildSideChatOptions(input: SideChatStartInput): Options {
  const options: Partial<Options> = {
    cwd: input.cwd,
    pathToClaudeCodeExecutable: input.pathToClaudeCodeExecutable,
    resume: input.resume,
    forkSession: true,
    persistSession: false,
    includePartialMessages: true,
    tools: [...SIDE_CHAT_TOOLS],
    allowedTools: [...SIDE_CHAT_TOOLS],
    disallowedTools: [...SIDE_CHAT_DISALLOWED_TOOLS],
    permissionMode: "default",
    canUseTool: sideChatCanUseTool,
    // No user/project settings: their hooks, permission rules and plugins
    // could act. The fork still carries the main thread's context.
    settingSources: [],
    settings: {
      disableAllHooks: true,
      ...(input.effort === "ultracode" ? { ultracode: true } : {}),
    } as unknown as NonNullable<Options["settings"]>,
    mcpServers: {},
    strictMcpConfig: true,
    plugins: [],
    systemPrompt: {
      type: "preset",
      preset: "claude_code",
      ...(input.appendSystemPrompt ? { append: input.appendSystemPrompt } : {}),
    },
    env: process.env as Record<string, string | undefined>,
  };
  if (input.model) options.model = input.model;
  const effort = sideChatCliEffort(input.effort);
  if (effort) options.effort = effort as unknown as NonNullable<Options["effort"]>;
  return options as Options;
}

/** The `--effort` value for a stored effort level, matching
 *  `buildSdkOptions`: `ultracode` is not a CLI level and runs as `xhigh`
 *  (its setting is merged above); `ultrathink` is prompt-injected only. */
export function sideChatCliEffort(effort: string | undefined): string | undefined {
  if (!effort || effort === "ultrathink") return undefined;
  return effort === "ultracode" ? "xhigh" : effort;
}

/** Prefix a question the way a main session prefixes its turns when the
 *  chat runs at `ultrathink`. Idempotent. */
export function sideChatPrompt(text: string, effort: string | undefined): string {
  if (effort !== "ultrathink" || text.trimStart().startsWith("Ultrathink:")) return text;
  return `Ultrathink:\n${text}`;
}

function userMessage(text: string): SDKUserMessage {
  return {
    type: "user",
    message: { role: "user", content: text },
    parent_tool_use_id: null,
  } as SDKUserMessage;
}

/** Text of a streamed `content_block_delta`, or null. */
function deltaText(message: SDKMessage): string | null {
  const m = message as unknown as {
    type?: string;
    parent_tool_use_id?: string | null;
    event?: { type?: string; delta?: { type?: string; text?: string } };
  };
  if (m.type !== "stream_event" || m.parent_tool_use_id) return null;
  const delta = m.event?.type === "content_block_delta" ? m.event.delta : undefined;
  return delta?.type === "text_delta" && typeof delta.text === "string" ? delta.text : null;
}

export class SideChat {
  readonly sideId: string;
  private readonly effort: string | undefined;
  private readonly queue = new AsyncPromptQueue<SDKUserMessage>();
  private readonly query;
  private closed = false;
  private readonly done: Promise<void>;

  constructor(
    input: SideChatStartInput,
    private readonly emitter: EventEmitter,
    factory: QueryFactory,
  ) {
    this.sideId = input.sideId;
    this.effort = input.effort;
    this.query = factory({ prompt: this.queue, options: buildSideChatOptions(input) });
    this.queue.push(userMessage(sideChatPrompt(input.text, this.effort)));
    this.done = this.consume();
  }

  private emit(method: string, extra: Record<string, unknown> = {}): void {
    this.emitter.notification(method, { sideId: this.sideId, ...extra });
  }

  private async consume(): Promise<void> {
    try {
      for await (const message of this.query) {
        if (this.closed) break;
        const text = deltaText(message);
        if (text) {
          this.emit("side-chat-delta", { text });
          continue;
        }
        const m = message as unknown as {
          type?: string;
          subtype?: string;
          result?: unknown;
          errors?: unknown;
        };
        if (m.type !== "result") continue;
        if (m.subtype === "success") {
          this.emit("side-chat-turn-completed", {
            text: typeof m.result === "string" ? m.result : null,
          });
        } else {
          const errors = Array.isArray(m.errors) ? m.errors.filter((e) => typeof e === "string") : [];
          this.emit("side-chat-error", {
            text: errors.join("\n") || `The side chat stopped (${m.subtype ?? "error"}).`,
          });
        }
      }
    } catch (err) {
      if (!this.closed) {
        this.emit("side-chat-error", { text: err instanceof Error ? err.message : String(err) });
      }
    } finally {
      this.queue.close();
      this.emit("side-chat-ended");
    }
  }

  send(text: string): void {
    if (this.closed || this.queue.isClosed()) throw new Error("side chat has ended");
    this.queue.push(userMessage(sideChatPrompt(text, this.effort)));
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.queue.close();
    try {
      this.query.close();
    } catch {
      // already gone
    }
    await this.done.catch(() => {});
  }
}
