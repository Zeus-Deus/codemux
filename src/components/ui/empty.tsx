import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"

/**
 * A panel with nothing to show yet: a title, a sentence on why, and when
 * there is one, the action that fixes it. A state that names a next step
 * the user can take here should carry that step as a button, not just
 * describe it.
 *
 * Sized on the type scale (`text-body` title, `text-body-sm` body) so it
 * follows the interface-size setting, and fades in on the surface tier so
 * it does not pop in under a list that just emptied.
 */
const emptyVariants = cva(
  "flex flex-col gap-1.5 px-3 py-8 motion-safe:animate-in motion-safe:fade-in-0 motion-safe:duration-150",
  {
    variants: {
      align: {
        center: "items-center text-center",
        start: "items-start text-left",
      },
    },
    defaultVariants: {
      align: "center",
    },
  },
)

function Empty({
  className,
  align,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof emptyVariants>) {
  return (
    <div
      data-slot="empty"
      className={cn(emptyVariants({ align }), className)}
      {...props}
    />
  )
}

function EmptyTitle({ className, ...props }: React.ComponentProps<"p">) {
  return (
    <p
      data-slot="empty-title"
      className={cn("text-body font-medium text-foreground", className)}
      {...props}
    />
  )
}

function EmptyDescription({ className, ...props }: React.ComponentProps<"p">) {
  return (
    <p
      data-slot="empty-description"
      className={cn(
        "max-w-[420px] text-body-sm leading-relaxed text-muted-foreground",
        className,
      )}
      {...props}
    />
  )
}

function EmptyActions({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="empty-actions"
      className={cn("mt-2 flex flex-wrap items-center gap-2", className)}
      {...props}
    />
  )
}

export { Empty, EmptyTitle, EmptyDescription, EmptyActions }
