import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

/**
 * The bar at the top of a panel, pane or overlay — one primitive, two
 * documented heights.
 *
 * Before this existed, fifteen hand-rolled headers drifted across five
 * heights (26, 28, 31, 36, 40px) with four different border alphas, so a
 * split pane's two headers, the right panel's tab row and a peek overlay's
 * URL bar all sat on different baselines. The two heights below are not a
 * compromise between them — they are the two the app genuinely needs:
 *
 * - `floating` (40px) is GUI chrome that overlaps content and reaches the
 *   window's top edge, so it carries no rule of its own.
 * - `inline` (36px) is in-flow and ruled off from the content beneath it.
 *
 * Deliberately not one height: `right-panel.test.tsx` pins both, because
 * the panel is 40px tall when it sits in the frameless titlebar band and
 * 36px when the legacy in-flow bar above it already spent that space.
 *
 * 36px is also the floor that the control ladder needs: `size="sm"` is a
 * 32px button and `icon-sm` is 28px, neither of which breathes inside the
 * 28px bars these headers used to be.
 *
 * Padding, gap and the border alpha are part of the contract — a caller
 * that needs tighter insets can still override them through `className`
 * (`cn` resolves the conflict in the caller's favour), but it then owns
 * the reason.
 */
const panelHeaderVariants = cva(
  "flex shrink-0 items-center gap-2 px-2.5",
  {
    variants: {
      variant: {
        floating: "h-10",
        inline: "h-9 border-b border-hairline",
      },
    },
    defaultVariants: {
      variant: "inline",
    },
  },
)

function PanelHeader({
  className,
  variant = "inline",
  asChild = false,
  ...props
}: React.ComponentProps<"header"> &
  VariantProps<typeof panelHeaderVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "header"

  return (
    <Comp
      data-slot="panel-header"
      data-variant={variant}
      className={cn(panelHeaderVariants({ variant }), className)}
      {...props}
    />
  )
}

export { PanelHeader, panelHeaderVariants }
