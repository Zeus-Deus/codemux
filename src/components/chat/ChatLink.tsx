import { useEffect, useState, type ComponentProps } from "react";
import { MessageSquareText } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { agentChatGetSession } from "@/tauri/commands";
import { EMPTY_APPEARANCE, useProjectAppearanceStore } from "@/stores/project-appearance-store";
import { basename } from "@/lib/path";
import { useAppStore } from "@/stores/app-store";
import type { AgentChatSessionRecord } from "@/tauri/commands";
import { cn } from "@/lib/utils";
import { openDelegatedChat } from "./delegation-actions";

/** A chat destination is opened by the app; the webview never follows it. */
export function ChatLink({ threadId, children, className, onClick, onAuxClick, ...props }: ComponentProps<"a"> & { threadId: string }) {
  const [shell, setShell] = useState<AgentChatSessionRecord | null | undefined>();
  const workspace = useAppStore((s) => s.appState?.workspaces.find((w) => w.workspace_id === shell?.workspace_id));
  const projectPath = workspace?.project_root ?? workspace?.cwd ?? "";
  const appearance = useProjectAppearanceStore((s) => s.byPath[projectPath] ?? EMPTY_APPEARANCE);
  const loadAppearance = useProjectAppearanceStore((s) => s.load);
  useEffect(() => { if (projectPath) void loadAppearance(projectPath); }, [projectPath, loadAppearance]);
  useEffect(() => {
    let live = true;
    const refresh = () => { void agentChatGetSession(threadId).then((record) => { if (live) setShell(record); }).catch(() => { if (live) setShell(null); }); };
    refresh();
    const subscription = listen<{ thread_ids: string[] }>("chat-shell-changed", (event) => { if (event.payload.thread_ids.includes(threadId)) refresh(); });
    return () => { live = false; void subscription.then((unlisten) => unlisten()); };
  }, [threadId]);
  const activate = () => { if (shell) void openDelegatedChat(threadId); };
  return (
    <a {...props} href={undefined} data-chat-link={threadId} data-streamdown="link"
      title={shell === null ? "Chat no longer available" : (projectPath ? basename(projectPath) : "Open chat")}
      role="link" tabIndex={shell === null ? -1 : 0} aria-disabled={shell === null || undefined}
      className={cn("inline-flex max-w-full items-baseline gap-1 font-medium [&]:text-primary [&]:no-underline hover:decoration-dotted hover:[&]:underline focus-visible:[&]:underline", shell === null ? "cursor-default opacity-60" : "cursor-pointer", className)}
      onClick={(event) => { onClick?.(event); const handled = event.defaultPrevented; event.preventDefault(); if (!handled) activate(); }}
      onAuxClick={(event) => { onAuxClick?.(event); const handled = event.defaultPrevented; event.preventDefault(); if (!handled && event.button === 1) activate(); }}
      onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); activate(); } }}
      onCopy={(event) => { event.preventDefault(); event.clipboardData.setData("text/plain", `[${shell?.title ?? (typeof children === "string" ? children : threadId)}](codemux-chat://v1/${encodeURIComponent(threadId)})`); }}>
      {appearance.imageUrl ? <ProjectAvatar name={basename(projectPath)} imageUrl={appearance.imageUrl} cacheBust={appearance.imageVersion} size="sm" className="inline-flex self-center" /> : <MessageSquareText aria-hidden="true" className="inline size-3.5 self-center" />}
      <span>{shell?.title ?? children}</span>
    </a>
  );
}
