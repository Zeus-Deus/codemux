import type { CommLogEntry } from "@/tauri/types";
import { getRoleColor, getRoleBgColor, formatRole } from "@/lib/openflow-utils";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";

interface CommLogMessageProps {
  entry: CommLogEntry;
  isDelivered?: boolean;
}

export function CommLogMessage({ entry, isDelivered }: CommLogMessageProps) {
  const role = entry.role.toLowerCase();
  const isUser = role === "user/inject" || role.startsWith("user");
  const isSystem = role === "system";
  const isAssign = entry.message.trimStart().startsWith("ASSIGN ");
  const isDone =
    entry.message.trimStart().startsWith("DONE:") ||
    entry.message.trimStart().startsWith("RUN COMPLETE:");

  return (
    <div
      className={cn(
        "px-3 py-1.5 text-xs border-l-2",
        isAssign && "border-l-amber-400/60 bg-amber-400/5",
        isDone && "border-l-emerald-400/60 bg-emerald-400/5",
        !isAssign && !isDone && "border-l-transparent",
        isSystem && "opacity-60 italic",
      )}
    >
      <div className="flex items-center gap-1.5 mb-0.5">
        <span
          className={cn(
            "inline-flex items-center rounded px-1 py-0.5 text-[10px] font-semibold uppercase tracking-wider",
            getRoleColor(entry.role),
            getRoleBgColor(entry.role),
          )}
        >
          {formatRole(entry.role)}
        </span>
        <span className="text-[10px] text-muted-foreground/60 tabular-nums">
          {entry.timestamp.split(" ")[1] ?? entry.timestamp}
        </span>
        {isUser && isDelivered && (
          <Badge
            variant="outline"
            className="h-3.5 px-1 text-[9px] border-emerald-400/30 text-emerald-400"
          >
            Delivered
          </Badge>
        )}
      </div>
      <pre className="whitespace-pre-wrap break-words font-mono text-xs text-foreground/90 leading-relaxed">
        {entry.message}
      </pre>
    </div>
  );
}
