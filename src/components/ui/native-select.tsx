import { ChevronDownIcon } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

type NativeSelectProps = Omit<React.ComponentProps<"select">, "size"> & {
  size?: "sm" | "default";
  /** Classes for the `<select>` itself; `className` styles the wrapper. */
  selectClassName?: string;
};

/**
 * shadcn/ui NativeSelect (radix-nova), on app tokens. The platform picker
 * stays underneath — keyboard, screen readers and the option list behave as
 * they always have — but the closed field is drawn like the app's Input,
 * with its own chevron instead of the browser's arrow. Every prop except
 * `className` lands on the `<select>`, so labels and test ids keep working.
 */
function NativeSelect({
  className,
  selectClassName,
  size = "default",
  ...props
}: NativeSelectProps) {
  return (
    <div
      className={cn(
        "group/native-select relative w-full has-[select:disabled]:opacity-50",
        className,
      )}
      data-slot="native-select-wrapper"
      data-size={size}
    >
      <select
        data-slot="native-select"
        data-size={size}
        className={cn(
          "h-9 w-full min-w-0 appearance-none rounded-lg border border-input bg-transparent py-1 pr-8 pl-3 text-body text-foreground transition-colors duration-150 outline-none select-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/60 disabled:pointer-events-none disabled:cursor-not-allowed aria-invalid:border-destructive data-[size=sm]:h-8 data-[size=sm]:pl-2.5 data-[size=sm]:text-body-sm dark:bg-input/30 dark:hover:bg-input/50",
          selectClassName,
        )}
        {...props}
      />
      <ChevronDownIcon
        className="pointer-events-none absolute top-1/2 right-2.5 size-4 -translate-y-1/2 text-muted-foreground select-none"
        aria-hidden="true"
        data-slot="native-select-icon"
      />
    </div>
  );
}

export { NativeSelect };
