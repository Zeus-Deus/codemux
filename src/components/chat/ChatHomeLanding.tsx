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
    <div className="flex h-full w-full flex-col items-center justify-center gap-8 px-4 pb-12">
      <h1 className="text-3xl font-medium tracking-tight text-foreground text-center">
        What should we do today?
      </h1>
      {/* Match the composer's own 760px column (design D10/D12) so the
          landing card lines up with the mid-conversation composer. */}
      <div className="w-full max-w-[760px]">{composer}</div>
    </div>
  );
}
