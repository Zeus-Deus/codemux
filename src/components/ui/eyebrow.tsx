import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

/**
 * A section label — the small uppercase line that titles a group of rows,
 * a card's field, or a panel's subsection.
 *
 * There were 35 different versions of this one label across 63 sites,
 * differing by 19 tracking values, four weights and five muted levels.
 * Individually invisible; collectively it reads as sloppiness. This locks
 * the five things that make an eyebrow an eyebrow — 11px, mono, semibold,
 * `tracking-eyebrow` (uppercase mono needs the letters opened up or it
 * reads as a smudge), uppercase, and a muted level — and leaves the
 * caller only layout and, where a status genuinely owns the label, colour.
 *
 * The canonical form is the one the settings primitives converged on.
 * `eyebrowVariants()` is exported for the few labels that must stay on an
 * element they do not own (a `<p>` inside another primitive), so those
 * still read from this one definition.
 */
const eyebrowVariants = cva(
  // `inline-block`, not `inline`: an eyebrow is often the first line of a
  // stack and has to be able to carry its own bottom margin. Flex parents
  // blockify their children anyway, so this costs nothing there.
  "inline-block font-mono text-caption font-semibold uppercase tracking-eyebrow",
  {
    variants: {
      tone: {
        /** Default: a label, not content. Quiet enough to scan past. */
        muted: "text-muted-foreground/55",
        /** For a label that has to hold its own against a dense row. */
        strong: "text-muted-foreground",
        /** Reserved for the one label on a surface that is an accent. */
        accent: "text-accent-ember",
      },
    },
    defaultVariants: {
      tone: "muted",
    },
  },
)

function Eyebrow({
  className,
  tone,
  asChild = false,
  ...props
}: React.ComponentProps<"span"> &
  VariantProps<typeof eyebrowVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "span"

  return (
    <Comp
      data-slot="eyebrow"
      className={cn(eyebrowVariants({ tone }), className)}
      {...props}
    />
  )
}

export { Eyebrow, eyebrowVariants }
