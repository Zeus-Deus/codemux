import * as React from "react";

import { cn } from "@/lib/utils";

/** shadcn/ui Kbd (radix-nova), on app tokens: one keycap style for every
 *  shortcut hint. A filled chip rather than an outline, sans figures. The
 *  fill is neutral ink (`surface-3`), not `muted`, which tinted themes
 *  shift toward their accent. */
function Kbd({ className, ...props }: React.ComponentProps<"kbd">) {
  return (
    <kbd
      data-slot="kbd"
      className={cn(
        "pointer-events-none inline-flex h-5 w-fit min-w-5 shrink-0 items-center justify-center gap-1 rounded-sm bg-surface-3 px-1 font-sans text-caption font-medium text-muted-foreground select-none [&_svg:not([class*='size-'])]:size-3",
        className,
      )}
      {...props}
    />
  );
}

function KbdGroup({ className, ...props }: React.ComponentProps<"kbd">) {
  return (
    <kbd
      data-slot="kbd-group"
      className={cn("inline-flex items-center gap-1", className)}
      {...props}
    />
  );
}

/** `Ctrl+Shift+P` → `["Ctrl", "Shift", "P"]`; a trailing `+` key survives
 *  (`Ctrl++` → `["Ctrl", "+"]`). */
export function splitKeyCombo(keys: string): string[] {
  if (keys.length <= 1 || !keys.includes("+")) return [keys];
  const plusKey = keys.endsWith("++");
  const parts = (plusKey ? keys.slice(0, -2) : keys).split("+").filter(Boolean);
  return plusKey ? [...parts, "+"] : parts;
}

/** Draws `Ctrl+Shift+P` as one cap per key. The `+` between caps stays in
 *  the text, visually hidden, so screen readers and copied text still read
 *  "Ctrl+Shift+P". */
function KbdKeys({ keys, className }: { keys: string; className?: string }) {
  const parts = splitKeyCombo(keys);
  return (
    <KbdGroup className={className}>
      {parts.map((part, index) => (
        <span key={index} className="contents">
          {index > 0 ? <span className="sr-only">+</span> : null}
          <Kbd>{part}</Kbd>
        </span>
      ))}
    </KbdGroup>
  );
}

export { Kbd, KbdGroup, KbdKeys };
