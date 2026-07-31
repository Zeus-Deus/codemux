import type { ReactNode } from "react";

interface Props {
  composer: ReactNode;
}

/**
 * Empty-state landing for the chat surface. Per the chat-ui skill,
 * this is the sole place inside the chat feature that may exceed
 * prose size — one marquee headline above the composer, no grid of
 * example prompts, no marketing copy. The composer is the invitation.
 */
export function ChatHomeLanding({ composer }: Props) {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-8 pb-12">
      <h1 className="px-4 text-3xl font-medium tracking-tight text-foreground text-center">
        What should we do today?
      </h1>
      {/* The composer carries the shared column rails itself (see
          chat-column.ts), so the landing card lines up with the
          mid-conversation composer at every pane width. */}
      <div className="w-full">{composer}</div>
    </div>
  );
}
