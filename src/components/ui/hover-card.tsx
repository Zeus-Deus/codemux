import * as React from "react"
import { HoverCard as HoverCardPrimitive } from "radix-ui"

import { POP_MOTION } from "@/lib/motion"
import { cn } from "@/lib/utils"

function HoverCard({
  ...props
}: React.ComponentProps<typeof HoverCardPrimitive.Root>) {
  return <HoverCardPrimitive.Root data-slot="hover-card" {...props} />
}

function HoverCardTrigger({
  ...props
}: React.ComponentProps<typeof HoverCardPrimitive.Trigger>) {
  return <HoverCardPrimitive.Trigger data-slot="hover-card-trigger" {...props} />
}

/** Entrance motion.
 *
 *  `default` is the shared overlay pop: fade plus a slight scale from the
 *  trigger. `subtle` keeps the fade but drops nearly all of the scale — for
 *  cards that open on a light hover and re-open constantly, where a card
 *  still visibly growing after it appeared reads as lag. */
const ENTRANCE_CLASS = {
  default: POP_MOTION,
  subtle:
    "data-[state=open]:animate-in data-[state=closed]:animate-out fade-in-0 fade-out-0 zoom-in-99 zoom-out-99 duration-150 ease-drawer data-[state=closed]:duration-100 data-[state=closed]:ease-out",
} as const

function HoverCardContent({
  className,
  align = "center",
  sideOffset = 4,
  entrance = "default",
  ...props
}: React.ComponentProps<typeof HoverCardPrimitive.Content> & {
  entrance?: keyof typeof ENTRANCE_CLASS
}) {
  return (
    <HoverCardPrimitive.Portal>
      <HoverCardPrimitive.Content
        data-slot="hover-card-content"
        align={align}
        sideOffset={sideOffset}
        className={cn(
          "z-50 w-64 origin-(--radix-hover-card-content-transform-origin) rounded-lg border border-border bg-popover p-3 text-popover-foreground shadow-popup outline-hidden",
          ENTRANCE_CLASS[entrance],
          // Set by a card that opened inside a group phase (see
          // `@/lib/hover-card-group`): it did not wait, so it must not spend
          // another 150ms arriving either. This zeroes the EXIT animation as
          // well, so such a card also hard-cuts on close — deliberate, not an
          // oversight: a card that opened instantly only ever closes mid-sweep,
          // where its fade-out would play on top of the card that just
          // superseded it. Cards that did wait their delay keep both halves.
          "data-instant:animation-duration-0",
          className
        )}
        {...props}
      />
    </HoverCardPrimitive.Portal>
  )
}

export { HoverCard, HoverCardTrigger, HoverCardContent }
