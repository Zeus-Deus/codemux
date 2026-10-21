import * as React from "react"

import { cn } from "@/lib/utils"

function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "flex field-sizing-content min-h-16 w-full rounded-lg border border-input bg-background px-3 py-2 text-body-lg shadow-xs/5 transition-[color,background-color,border-color,box-shadow] duration-100 outline-none placeholder:text-muted-foreground inset-shadow-bevel focus-visible:border-ring focus-visible:inset-shadow-none focus-visible:ring-[3px] focus-visible:ring-ring/24 focus-visible:ring-inset disabled:cursor-not-allowed disabled:opacity-64 disabled:shadow-none disabled:inset-shadow-none aria-invalid:border-destructive/36 aria-invalid:inset-shadow-none focus-visible:aria-invalid:border-destructive/64 focus-visible:aria-invalid:ring-destructive/16 md:text-body dark:bg-input/32 dark:focus-visible:aria-invalid:ring-destructive/24",
        className
      )}
      {...props}
    />
  )
}

export { Textarea }
