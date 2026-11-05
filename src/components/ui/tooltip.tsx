import { Kbd } from "@/components/ui/kbd"
import * as React from "react"
import { Tooltip as TooltipPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"
import { useShortcutLabel } from "@/components/ui/menu-chrome"

function TooltipProvider({
  // Matches the app root, so a nested provider never makes its tooltips
  // instant by accident.
  delayDuration = 300,
  skipDelayDuration = 300,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      skipDelayDuration={skipDelayDuration}
      {...props}
    />
  )
}

function Tooltip({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Root>) {
  return <TooltipPrimitive.Root data-slot="tooltip" {...props} />
}

/** Whether the last key pressed was Tab. Focus that arrives without a
 *  previous element is either the user tabbing in from the page or a popup
 *  handing focus back as it closes (after Esc or a click), and only this
 *  tells the two apart. */
let tabbing = false
if (typeof document !== "undefined") {
  document.addEventListener("keydown", (event) => {
    tabbing = event.key === "Tab"
  }, true)
  document.addEventListener("pointerdown", () => {
    tabbing = false
  }, true)
}

function TooltipTrigger({
  onFocus,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return (
    <TooltipPrimitive.Trigger
      data-slot="tooltip-trigger"
      // Only keyboard focus opens a tooltip. A popover that moves focus into
      // itself on a mouse click would otherwise pop a label over the content
      // the user just opened. Radix skips its handler once default is prevented.
      // Focus handed back by a closing menu, popover or dialog isn't the user
      // arriving either: it comes from nowhere (the popup that held it was
      // removed) or straight out of a popup's content around something else.
      // Tab navigation shows the label wherever it comes from.
      onFocus={(event) => {
        onFocus?.(event)
        const target = event.currentTarget
        if (target.ownerDocument.activeElement !== target) return
        const from = event.relatedTarget
        const popup = from instanceof Element ? from.closest('[data-slot$="-content"]') : null
        const returned = !tabbing && (from === null || (popup !== null && !popup.contains(target)))
        if (returned || !target.matches(":focus-visible")) {
          event.preventDefault()
        }
      }}
      {...props}
    />
  )
}

/** The keycap a tooltip carries for its control's binding. Resolved through
 *  the user's overrides, and absent when the action is unbound. */
function TooltipShortcut({ actionId }: { actionId: string }) {
  const keys = useShortcutLabel(actionId)
  if (!keys) return null
  return (
    <Kbd>{keys}</Kbd>
  )
}

/** A tooltip is a small popover: the popover surface, border and bevel at
 *  the control-label size, with no arrow. An inverted chip shouted over the
 *  quiet surfaces around it, and an arrow can't carry the border. */
function TooltipContent({
  className,
  sideOffset = 4,
  collisionPadding = 8,
  shortcut,
  hint,
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content> & {
  /** A `keybind-registry` action id whose binding is shown after the label. */
  shortcut?: string
  /** A secondary line under the label (e.g. a modifier-click variant). The
   *  keycap stays on the right, centred against both lines. */
  hint?: React.ReactNode
}) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        data-slot="tooltip-content"
        sideOffset={sideOffset}
        collisionPadding={collisionPadding}
        className={cn(
          "z-50 inline-flex w-fit max-w-xs items-center gap-1.5 rounded-md border border-border bg-popover px-2 py-1 text-label text-popover-foreground shadow-md/5 inset-shadow-bevel has-data-[slot=kbd]:pr-1 **:data-[slot=kbd]:relative **:data-[slot=kbd]:isolate **:data-[slot=kbd]:z-50 **:data-[slot=kbd]:rounded-sm",
          // Only a tooltip that waited out its delay fades in. One that opens
          // instantly mid-sweep (`instant-open`) hard-cuts, so dragging the
          // pointer along a toolbar never stacks fades on top of each other.
          "origin-(--radix-tooltip-content-transform-origin) duration-100 ease-drawer motion-safe:data-[state=delayed-open]:animate-in motion-safe:data-[state=delayed-open]:fade-in-0 motion-safe:data-[state=delayed-open]:zoom-in-97",
          className
        )}
        {...props}
      >
        {hint ? (
          <span className="flex flex-col gap-0.5">
            <span>{children}</span>
            <span className="text-caption text-muted-foreground">{hint}</span>
          </span>
        ) : (
          children
        )}
        {shortcut && <TooltipShortcut actionId={shortcut} />}
      </TooltipPrimitive.Content>
    </TooltipPrimitive.Portal>
  )
}

/** A bottom tooltip naming a control and showing the binding for the same
 *  action, for icon buttons that would otherwise rely on a native `title`. */
function ShortcutTooltip({
  label,
  shortcut,
  children,
}: {
  label: React.ReactNode
  /** A `keybind-registry` action id. */
  shortcut: string
  children: React.ReactNode
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side="bottom" shortcut={shortcut}>
        {label}
      </TooltipContent>
    </Tooltip>
  )
}

export { ShortcutTooltip, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger }
