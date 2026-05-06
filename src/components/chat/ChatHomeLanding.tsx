import type { ReactNode } from "react";

interface Props {
  composer: ReactNode;
}

export function ChatHomeLanding({ composer }: Props) {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-6 px-4">
      <h1 className="text-2xl font-medium text-foreground">
        What should we do today?
      </h1>
      <div className="w-full max-w-2xl">{composer}</div>
    </div>
  );
}
