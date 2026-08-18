import type { AgentChatSessionMention } from "@/tauri/commands";

/** Human provider label with a graceful fallback for adapters added after the
 *  current frontend was built. */
export function sessionProviderLabel(provider: string): string {
  switch (provider.toLowerCase()) {
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "cursor":
      return "Cursor";
    case "opencode":
      return "OpenCode";
    default:
      return provider.length > 0
        ? provider.charAt(0).toUpperCase() + provider.slice(1)
        : "Agent";
  }
}

export function sessionMentionTitle(session: AgentChatSessionMention): string {
  const title = session.title?.trim();
  return title || `Chat ${session.thread_id.slice(-6)}`;
}

/** Stable, compact and somewhat readable token. The title portion helps the
 *  draft make sense at a glance; the id suffix prevents same-title collisions
 *  and survives a later rename. */
export function sessionMentionToken(session: AgentChatSessionMention): string {
  const slug = sessionMentionTitle(session)
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 22)
    .replace(/-+$/g, "");
  const suffix = session.thread_id.replace(/[^A-Za-z0-9]/g, "").slice(-6);
  return `${slug || "chat"}-${suffix || "session"}`;
}

export function compactSessionPreview(value: string, maxChars = 120): string {
  const compact = value.replace(/\s+/g, " ").trim();
  if (compact.length <= maxChars) return compact;
  return `${compact.slice(0, Math.max(1, maxChars - 1)).trimEnd()}…`;
}

/** Remove a staged session's inline source token when its strip chip is
 *  dismissed. Prefer consuming one following plain space so prose around the
 *  chip remains naturally separated without normalising intentional layout. */
export function removeSessionMentionToken(
  draft: string,
  mentionToken: string,
): string {
  const literal = `@session:${mentionToken}`;
  return draft.split(`${literal} `).join("").split(literal).join("");
}
