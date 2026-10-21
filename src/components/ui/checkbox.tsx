import { CheckIcon } from "lucide-react";
import { Checkbox as CheckboxPrimitive } from "radix-ui";
import * as React from "react";

import { cn } from "@/lib/utils";

/** shadcn/ui Checkbox (radix-nova), on app tokens. Draws in the theme's own
 *  colours instead of the operating system's native control. */
function Checkbox({
  className,
  ...props
}: React.ComponentProps<typeof CheckboxPrimitive.Root>) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        "peer relative flex size-4 shrink-0 items-center justify-center rounded-[calc(var(--radius-sm)-2px)] border border-input bg-background shadow-xs/5 inset-shadow-bevel transition-[color,background-color,border-color,box-shadow] duration-100 outline-none after:absolute after:-inset-x-3 after:-inset-y-2 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-64 disabled:shadow-none disabled:inset-shadow-none aria-invalid:border-destructive/36 dark:bg-input/32 data-[state=checked]:border-primary data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground data-[state=checked]:shadow-none data-[state=checked]:inset-shadow-highlight dark:data-[state=checked]:bg-primary",
        className,
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator
        data-slot="checkbox-indicator"
        className="grid place-content-center text-current [&>svg]:size-3 [&>svg]:stroke-3"
      >
        <CheckIcon />
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  );
}

export { Checkbox };
