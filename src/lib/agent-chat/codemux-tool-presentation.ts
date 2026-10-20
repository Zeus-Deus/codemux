import { Bot, CircleStop, List, MessageSquareText, Search, Send, Timer, type LucideIcon } from "lucide-react";
import type { ToolCallItem } from "./types";

type Presentation = { present: string; progressive: string; past: string; icon: LucideIcon };
const tools: Record<string, Presentation> = {
  orchestrator_capabilities: { present: "Discover agent capabilities", progressive: "Discovering agent capabilities", past: "Discovered agent capabilities", icon: List },
  delegate_task: { present: "Delegate a child task", progressive: "Delegating a child task", past: "Delegated a child task", icon: Bot },
  task_status: { present: "Read delegated task", progressive: "Reading delegated task", past: "Read delegated task", icon: Bot },
  task_cancel: { present: "Cancel delegated task", progressive: "Cancelling delegated task", past: "Requested cancellation of delegated task", icon: CircleStop },
  thread_launch: { present: "Start chat", progressive: "Starting chat", past: "Started chat", icon: MessageSquareText },
  create_threads: { present: "Start chats", progressive: "Starting chats", past: "Started chats", icon: MessageSquareText },
  thread_send: { present: "Send message to chat", progressive: "Sending message to chat", past: "Sent message to chat", icon: Send },
  thread_read: { present: "Read chat", progressive: "Reading chat", past: "Read chat", icon: MessageSquareText },
  thread_wait: { present: "Wait for chat", progressive: "Waiting for chat", past: "Waited for chat", icon: Timer },
  thread_list: { present: "List chats", progressive: "Listing chats", past: "Listed chats", icon: List },
  thread_search: { present: "Search chats", progressive: "Searching chats", past: "Searched chats", icon: Search },
  thread_interrupt: { present: "Interrupt chat run", progressive: "Interrupting chat run", past: "Requested chat run interruption", icon: CircleStop },
};
export function codemuxToolPresentation(name: string): Presentation | null {
  return tools[name.split("__").pop() ?? ""] ?? null;
}
export function orchestrationSummary(item: ToolCallItem): { verb: string; target: string } | null {
  const presentation = codemuxToolPresentation(item.tool_name);
  if (!presentation) return null;
  const input = item.input && typeof item.input === "object" ? item.input as Record<string, unknown> : {};
  const target = [input.title, input.query].find((v): v is string => typeof v === "string") ?? "";
  return { verb: item.status === "running" ? presentation.progressive : item.status === "done" ? presentation.past : presentation.present, target };
}

export function launchedChatId(content: unknown): string | null {
  if (typeof content === "string") { try { return launchedChatId(JSON.parse(content)); } catch { return null; } }
  if (Array.isArray(content)) { for (const block of content) { const id = launchedChatId(block); if (id) return id; } return null; }
  if (content === null || typeof content !== "object") return null;
  const record = content as Record<string, unknown>;
  if (record.isError === true || record.success === false || record.state === "failed") return null;
  if (typeof record.thread_id === "string") return record.thread_id;
  return launchedChatId(record.content ?? record.contentItems ?? record.text ?? record.result);
}
