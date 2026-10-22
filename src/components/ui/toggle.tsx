"use client"

import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Toggle as TogglePrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

/**
 * shadcn Toggle (radix-nova), ported to the token contract: type on the rem
 * scale, a 2px focus ring, the 100/150/250 duration ladder and surface
 * tokens. The pressed state is a raised surface plus a hairline inset ring,
 * so it reads in any palette and not by colour alone. Radix exposes it as
 * `aria-pressed` (`aria-checked` inside a single-select group) and
 * `data-state=on`; the style keys off all three because a wrapping
 * `asChild` trigger (a Tooltip, say) can overwrite `data-state`.
 */
const toggleVariants = cva(
  "group/toggle inline-flex items-center justify-center gap-1 rounded-control text-body font-medium whitespace-nowrap text-muted-foreground transition-[color,background-color,box-shadow] duration-150 outline-none hover:bg-surface-2 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64 aria-invalid:ring-2 aria-invalid:ring-destructive/40 aria-pressed:bg-surface-3 aria-pressed:text-foreground aria-pressed:shadow-[inset_0_0_0_1px_var(--border)] aria-checked:bg-surface-3 aria-checked:text-foreground aria-checked:shadow-[inset_0_0_0_1px_var(--border)] data-[state=on]:bg-surface-3 data-[state=on]:text-foreground data-[state=on]:shadow-[inset_0_0_0_1px_var(--border)] [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-transparent",
        outline: "border border-input bg-popover shadow-xs/5 inset-shadow-bevel hover:bg-surface-2 aria-pressed:shadow-none aria-pressed:inset-shadow-none data-[state=on]:shadow-none data-[state=on]:inset-shadow-none dark:bg-input/32",
      },
      size: {
        default:
          "h-8 min-w-8 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2",
        sm: "h-7 min-w-7 px-2.5 text-label has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3.5",
        lg: "h-9 min-w-9 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Toggle({
  className,
  variant = "default",
  size = "default",
  ...props
}: React.ComponentProps<typeof TogglePrimitive.Root> &
  VariantProps<typeof toggleVariants>) {
  return (
    <TogglePrimitive.Root
      data-slot="toggle"
      className={cn(toggleVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Toggle, toggleVariants }
