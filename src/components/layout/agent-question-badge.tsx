import { MessageCircleQuestion } from "lucide-react";
import { useQuestionAttention } from "@/stores/async-question-attention-store";

/** Question attention remains visible alongside execution and approval status. */
export function AgentQuestionBadge({ workspaceId }: { workspaceId: string }) {
  const count = useQuestionAttention(
    (state) => state.workspaces[workspaceId] ?? 0,
  );
  if (!count) return null;
  const label = `${count} unanswered agent question${count === 1 ? "" : "s"}`;
  return (
    <span
      title={label}
      aria-label={label}
      className="flex shrink-0 items-center gap-0.5 text-[10px] text-primary"
    >
      <MessageCircleQuestion className="size-3" />
      {count}
    </span>
  );
}
