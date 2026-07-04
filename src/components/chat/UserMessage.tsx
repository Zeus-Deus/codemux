import { memo } from "react";

import type { UserMessageItem } from "@/lib/agent-chat/types";

/**
 * Right-aligned user bubble (design D3). Card fill, soft border, the
 * asymmetric `14px 14px 5px 14px` radius that tucks the bottom-right
 * corner toward the composer, and pre-wrapped text so pasted snippets
 * keep their line breaks. Width is capped so long turns don't span the
 * whole 760px column.
 */
export const UserMessage = memo(function UserMessage({
  item,
}: {
  item: UserMessageItem;
}) {
  return (
    <div className="flex justify-end">
      <div className="max-w-[82%] whitespace-pre-wrap break-words rounded-[14px_14px_5px_14px] border border-border/60 bg-card px-[15px] py-[11px] text-[13.5px] leading-[1.55] text-foreground">
        {item.text}
      </div>
    </div>
  );
});
