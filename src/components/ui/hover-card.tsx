import * as React from "react"
import { HoverCard as HoverCardPrimitive } from "radix-ui"

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
 *  `default` is the shadcn one: fade, 95% zoom, and an 8px slide in from the
 *  trigger's side. `subtle` keeps the fade but drops the slide and nearly all
 *  of the zoom — for cards that open on a light hover and re-open constantly,
 *  where a card still visibly travelling after it appeared reads as lag. */
const ENTRANCE_CLASS = {
  default:
    "data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2",
  subtle: "data-[state=closed]:zoom-out-98 data-[state=open]:zoom-in-98",
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
          "z-50 w-64 rounded-md border bg-popover p-3 text-popover-foreground shadow-md outline-hidden data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
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
