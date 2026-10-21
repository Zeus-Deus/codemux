import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

/**
 * A section label — the small line that titles a group of rows, a card's
 * field, or a panel's subsection.
 *
 * There were 35 different versions of this one label across 63 sites,
 * differing by tracking, weight and muted level. Individually invisible;
 * collectively it reads as sloppiness. This locks what makes an eyebrow an
 * eyebrow — 12px interface sans, medium weight, sentence case as written,
 * and a muted level — and leaves the caller only layout and, where a status
 * genuinely owns the label, colour. It is deliberately not mono: mono is
 * reserved for code-like data, and an uppercase mono label is noise.
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
  "inline-block text-label font-medium",
  {
    variants: {
      tone: {
        /** Default: a label, not content. Quiet enough to scan past. */
        muted: "text-muted-foreground/80",
        /** For a label that has to hold its own against a dense row. */
        strong: "text-foreground/80",
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
