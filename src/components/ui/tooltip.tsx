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

/** Whether the last key pressed moves focus by navigation: Tab, or an arrow
 *  key roving through a toolbar or list. Everything else that lands focus on
 *  a control — a popup auto-focusing its first item after Enter or a
 *  shortcut, a closing popup handing focus back after Esc or a click, a
 *  shortcut jumping to a control — happens after some other key or a pointer
 *  press, and only this tells the two apart. */
let navigating = false
if (typeof document !== "undefined") {
  document.addEventListener("keydown", (event) => {
    navigating = event.key === "Tab" || event.key.startsWith("Arrow")
  }, true)
  document.addEventListener("pointerdown", () => {
    navigating = false
  }, true)
}

function TooltipTrigger({
  onFocus,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return (
    <TooltipPrimitive.Trigger
      data-slot="tooltip-trigger"
      // Only focus the user moved there by navigating opens a tooltip. A popup
      // that focuses its first item as it opens (from a click, Enter or a
      // shortcut) would otherwise pop a label over the content just opened,
      // and the first Esc would only dismiss that label; focus handed back by
      // a closing menu, popover or dialog isn't the user arriving either.
      // Radix skips its handler once default is prevented.
      onFocus={(event) => {
        onFocus?.(event)
        const target = event.currentTarget
        if (target.ownerDocument.activeElement !== target) return
        if (!navigating || !target.matches(":focus-visible")) {
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
