import type { UserMessageItem } from "@/lib/agent-chat/types";

export function UserMessage({ item }: { item: UserMessageItem }) {
  return (
    <div className="flex justify-end">
      <div className="inline-block max-w-full rounded-2xl bg-muted px-3.5 py-2 text-sm text-foreground whitespace-pre-wrap break-words">
        {item.text}
      </div>
    </div>
  );
}
