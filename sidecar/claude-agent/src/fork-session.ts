// forkSession — copy a Claude session into a new, persistent session for a
// chat fork (`fork-session`).
//
// The agent SDK's own `forkSession` does the copy (fresh message uuids, the
// source file untouched); `upToMessageId` cuts it at an earlier turn. Codemux
// addresses that turn by its user prompt, so the cut is found here: the
// first prompt the fork leaves out is located in the SDK transcript, by
// count when the counts agree and by text otherwise, and the fork keeps
// everything before it. A prompt that can't be found refuses the fork
// rather than cutting somewhere else.
//
// The SDK writes the fork next to the source transcript. Claude only
// resumes a session from the project directory of the folder it runs in,
// so a fork that will run in another folder (a new worktree) is moved
// there.

import {
  forkSession as sdkForkSession,
  getSessionMessages as sdkGetSessionMessages,
  type SessionMessage,
} from "@anthropic-ai/claude-agent-sdk";
import { copyFile, mkdir, readdir, realpath, rename, unlink } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

/** Keep `keepPrompts` user prompts; the next one reads `nextPrompt`.
 *  `occurrence` counts the kept prompts with exactly that text. */
export interface ForkCut {
  keepPrompts: number;
  nextPrompt: string;
  occurrence: number;
}

export interface ForkSessionInput {
  sessionId: string;
  /** Folder of the source chat: where its transcript is found. */
  dir: string;
  /** Folder the fork will run in. */
  targetDir: string;
  /** Omitted: fork the whole session. */
  cut?: ForkCut;
}

/** SDK and filesystem seams, swapped in tests. */
export interface ForkSessionDeps {
  forkSession: typeof sdkForkSession;
  getSessionMessages: typeof sdkGetSessionMessages;
  configDir: () => string;
}

const defaultDeps: ForkSessionDeps = {
  forkSession: sdkForkSession,
  getSessionMessages: sdkGetSessionMessages,
  configDir: () =>
    (process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), ".claude")).normalize("NFC"),
};

export async function forkClaudeSession(
  input: ForkSessionInput,
  deps: ForkSessionDeps = defaultDeps,
): Promise<{ sessionId: string }> {
  let upToMessageId: string | undefined;
  if (input.cut) {
    const messages = await deps.getSessionMessages(input.sessionId, { dir: input.dir });
    upToMessageId = findCut(messages, input.cut);
  }
  const { sessionId } = await deps.forkSession(input.sessionId, {
    dir: input.dir,
    ...(upToMessageId ? { upToMessageId } : {}),
  });
  if (input.targetDir !== input.dir) {
    await moveToProject(deps.configDir(), sessionId, input.targetDir);
  }
  return { sessionId };
}

/** A user message that is a prompt: typed text or images, not a tool
 *  result, and not the CLI's own "[Request interrupted by user]" marker. */
function promptText(message: SessionMessage): string | null {
  if (message.type !== "user") return null;
  const content = (message.message as { content?: unknown } | null)?.content;
  let text: string;
  if (typeof content === "string") {
    text = content;
  } else if (Array.isArray(content)) {
    const blocks = content as Array<{ type?: unknown; text?: unknown }>;
    if (blocks.some((block) => block?.type === "tool_result")) return null;
    text = blocks
      .filter((block) => block?.type === "text" && typeof block.text === "string")
      .map((block) => block.text as string)
      .join("\n");
  } else {
    return null;
  }
  if (/^\s*\[Request interrupted by user/.test(text)) return null;
  return text;
}

const normalize = (text: string) => text.replace(/\s+/g, " ").trim();

/** The uuid of the last message the fork keeps: the one just before the
 *  first prompt it leaves out. Codemux may wrap a prompt (skills, attached
 *  context), so a prompt matches when it contains the typed text. */
export function findCut(messages: SessionMessage[], cut: ForkCut): string {
  const prompts = messages.flatMap((message, index) => {
    const text = promptText(message);
    return text === null ? [] : [{ index, text: normalize(text) }];
  });
  const wanted = normalize(cut.nextPrompt);
  const matches = (text: string) => (wanted ? text.includes(wanted) : text === "");
  let target = prompts[cut.keepPrompts];
  if (!target || !matches(target.text)) {
    // The counts disagree (a prompt the SDK never saw, such as one sent
    // before a provider handoff): fall back to the prompt's own text.
    target = wanted ? prompts.filter((prompt) => matches(prompt.text))[cut.occurrence] : undefined;
  }
  const kept = target ? messages[target.index - 1] : undefined;
  if (!kept) throw new Error("fork_point_not_found: that turn isn't in this Claude session");
  return kept.uuid;
}

/** Claude's project directory name for a folder, as the CLI derives it:
 *  every non-alphanumeric character becomes `-`, and long paths keep a
 *  200-character prefix plus a hash. */
export function projectDirName(path: string): string {
  const name = path.replace(/[^a-zA-Z0-9]/g, "-");
  if (name.length <= 200) return name;
  let hash = 0;
  for (let i = 0; i < path.length; i++) hash = ((hash << 5) - hash + path.charCodeAt(i)) | 0;
  return `${name.slice(0, 200)}-${Math.abs(hash).toString(36)}`;
}

async function moveToProject(configDir: string, sessionId: string, targetDir: string) {
  const projects = join(configDir, "projects");
  let target: string;
  try {
    target = (await realpath(targetDir)).normalize("NFC");
  } catch {
    target = targetDir.normalize("NFC");
  }
  const destinationDir = join(projects, projectDirName(target));
  const file = `${sessionId}.jsonl`;
  let source: string | undefined;
  for (const entry of await readdir(projects, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const dir = join(projects, entry.name);
    if ((await readdir(dir)).includes(file)) {
      source = dir;
      break;
    }
  }
  if (!source) throw new Error(`the forked session ${sessionId} wasn't written`);
  if (source === destinationDir) return;
  await mkdir(destinationDir, { recursive: true });
  try {
    await rename(join(source, file), join(destinationDir, file));
  } catch {
    await copyFile(join(source, file), join(destinationDir, file));
    await unlink(join(source, file));
  }
}
