import * as React from "react"

import { cn } from "@/lib/utils"

/** A 32px field: the same height as a default button and select trigger, so
 *  a field and its action line up. It carries the outlined-control bevel at
 *  rest; focus swaps the bevel for a soft ring drawn inside the border, which
 *  a scrolling or clipped parent can't cut off. */
function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-8 w-full min-w-0 rounded-lg border border-input bg-background px-3 py-1 text-body-lg shadow-xs/5 transition-[color,background-color,border-color,box-shadow] duration-100 outline-none file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-body file:font-medium file:text-foreground placeholder:text-muted-foreground inset-shadow-bevel focus-visible:border-ring focus-visible:inset-shadow-none focus-visible:ring-[3px] focus-visible:ring-ring/24 focus-visible:ring-inset disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-64 disabled:shadow-none disabled:inset-shadow-none aria-invalid:border-destructive/36 aria-invalid:inset-shadow-none focus-visible:aria-invalid:border-destructive/64 focus-visible:aria-invalid:ring-destructive/16 md:text-body dark:bg-input/32 dark:focus-visible:aria-invalid:ring-destructive/24",
        className
      )}
      {...props}
    />
  )
}

export { Input }
