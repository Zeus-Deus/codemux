import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

const buttonVariants = cva(
  // Press feedback is a 3% shrink rather than a 1px drop, so it reads the same
  // on a 24px icon button and a 40px primary action. Menu triggers skip it:
  // the menu opening is their feedback. Disabled sits at 64% so a disabled
  // label stays legible. The focus ring is drawn inside the border with a
  // 1px gap, so it can't be clipped by a scrolling parent or bleed into a
  // neighbouring control.
  //
  // Icons take `--control-icon-color`. Outline, toolbar and ghost buttons
  // with a label dim the glyph to 72% of the label's colour, so the label
  // leads and the icon still follows hover and tone changes. Icon-only sizes
  // reset it: with no label beside it, a dimmed glyph is just a weaker one.
  "group/button inline-flex shrink-0 items-center justify-center rounded-control border border-transparent text-body font-medium whitespace-nowrap transition-[color,background-color,border-color,box-shadow,scale] duration-100 outline-none select-none [--control-icon-color:currentColor] focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset focus-visible:ring-offset-1 focus-visible:ring-offset-background active:not-aria-[haspopup]:scale-[0.97] disabled:pointer-events-none disabled:opacity-64 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4 [&_svg:not([class*='text-'])]:text-(--control-icon-color)",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow-xs shadow-primary/24 dark:shadow-black/40 inset-shadow-highlight hover:bg-primary/90 active:inset-shadow-pressed active:shadow-none disabled:shadow-none disabled:inset-shadow-none",
        outline:
          "border-input bg-popover text-foreground shadow-xs/5 [--control-icon-color:color-mix(in_oklch,currentColor_72%,transparent)] inset-shadow-bevel hover:bg-surface-2 hover:text-foreground aria-expanded:bg-muted aria-expanded:text-foreground active:shadow-none active:inset-shadow-none disabled:shadow-none disabled:inset-shadow-none dark:bg-input/32 dark:hover:bg-input/64",
        toolbar:
          "border-border bg-secondary/50 text-muted-foreground [--control-icon-color:color-mix(in_oklch,currentColor_72%,transparent)] inset-shadow-bevel active:inset-shadow-none disabled:inset-shadow-none hover:bg-secondary hover:text-foreground aria-expanded:bg-secondary aria-expanded:text-foreground",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-secondary/90 active:bg-secondary/80 aria-expanded:bg-secondary aria-expanded:text-secondary-foreground",
        ghost:
          "[--control-icon-color:color-mix(in_oklch,currentColor_72%,transparent)] hover:bg-surface-2 hover:text-foreground aria-expanded:bg-muted aria-expanded:text-foreground",
        destructive:
          "bg-destructive/10 text-destructive hover:bg-destructive/20 focus-visible:ring-destructive/60 dark:bg-destructive/20 dark:hover:bg-destructive/30",
        link: "text-primary underline-offset-4 hover:underline active:scale-100",
      },
      size: {
        // Control-height scale: xs 24, sm 28, default 32, lg 36. A desktop
        // pointer doesn't need a 36px target, and 32px is the height inputs
        // and select triggers share, so a button beside a field lines up.
        //
        // `sm` is 28px. When it was 34px, 82 of its 129 callers patched the
        // height from outside, the largest group (41) to 28px. With default
        // now 32px, 28px is the step below it, and still leaves room for a
        // 14px icon.
        default:
          "h-8 gap-1.5 px-3 has-data-[icon=inline-end]:pr-2.5 has-data-[icon=inline-start]:pl-2.5",
        xs: "h-6 gap-1 rounded-[min(var(--radius-md),8px)] px-2 text-label in-data-[slot=button-group]:rounded-control has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-7 gap-1.5 px-2.5 text-body-sm in-data-[slot=button-group]:rounded-control has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2 [&_svg:not([class*='size-'])]:size-3.5",
        lg: "h-9 gap-2 px-4 has-data-[icon=inline-end]:pr-3 has-data-[icon=inline-start]:pl-3",
        icon: "size-8 [--control-icon-color:currentColor]",
        "icon-xs":
          "size-6 rounded-[min(var(--radius-md),8px)] in-data-[slot=button-group]:rounded-control [--control-icon-color:currentColor] [&_svg:not([class*='size-'])]:size-3",
        // 28px control, 14px glyph. Stating the glyph size is not cosmetic:
        // without it the base's 16px applied, so an icon-sm button rendered a
        // 16px glyph while its call sites asked for 12 or 14 and were
        // silently overruled by the base rule's higher specificity. That is
        // how the left sidebar toggle came to render 16px next to a right
        // panel toggle at 14px while a class-string assertion called them
        // mirrored.
        "icon-sm":
          "size-7 in-data-[slot=button-group]:rounded-control [--control-icon-color:currentColor] [&_svg:not([class*='size-'])]:size-3.5",
        "icon-lg": "size-9 [--control-icon-color:currentColor]",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }

/** A text action that keeps surrounding typography and keyboard focus visible. */
export function InlineButton({ className, tone, type = "button", ...props }: React.ComponentProps<"button"> & { tone?: "muted" }) {
  return <button type={type} className={cn("inline-flex items-center gap-0.5 font-medium underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-64", tone === "muted" && "text-muted-foreground hover:text-foreground", className)} {...props} />
}
