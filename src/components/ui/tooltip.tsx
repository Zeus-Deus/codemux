import * as React from "react"
import { Tooltip as TooltipPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"
import { useShortcutLabel } from "@/components/ui/menu-chrome"

function TooltipProvider({
  delayDuration = 0,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      {...props}
    />
  )
}

function Tooltip({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Root>) {
  return <TooltipPrimitive.Root data-slot="tooltip" {...props} />
}

function TooltipTrigger({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />
}

/** The keycap a tooltip carries for its control's binding. Resolved through
 *  the user's overrides, and absent when the action is unbound. */
function TooltipShortcut({ actionId }: { actionId: string }) {
  const keys = useShortcutLabel(actionId)
  if (!keys) return null
  return (
    <kbd
      data-slot="kbd"
      className="bg-background/15 px-1 py-px font-mono text-micro leading-none font-normal text-background/75"
    >
      {keys}
    </kbd>
  )
}

function TooltipContent({
  className,
  sideOffset = 0,
  shortcut,
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content> & {
  /** A `keybind-registry` action id whose binding is shown after the label. */
  shortcut?: string
}) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        data-slot="tooltip-content"
        sideOffset={sideOffset}
        className={cn(
          "z-50 inline-flex w-fit max-w-xs items-center gap-1.5 rounded-md bg-foreground px-3 py-1.5 text-label text-background has-data-[slot=kbd]:pr-1.5 **:data-[slot=kbd]:relative **:data-[slot=kbd]:isolate **:data-[slot=kbd]:z-50 **:data-[slot=kbd]:rounded-sm",
          // Only a tooltip that waited out its delay fades in. One that opens
          // instantly mid-sweep (`instant-open`) hard-cuts, so dragging the
          // pointer along a toolbar never stacks fades on top of each other.
          "duration-100 motion-safe:data-[state=delayed-open]:animate-in motion-safe:data-[state=delayed-open]:fade-in-0 motion-safe:data-[state=delayed-open]:zoom-in-95",
          className
        )}
        {...props}
      >
        {children}
        {shortcut && <TooltipShortcut actionId={shortcut} />}
        <TooltipPrimitive.Arrow className="z-50 size-2.5 translate-y-[calc(-50%_-_2px)] rotate-45 rounded-sm bg-foreground fill-foreground" />
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
      <TooltipContent side="bottom" sideOffset={4} shortcut={shortcut}>
        {label}
      </TooltipContent>
    </Tooltip>
  )
}

export { ShortcutTooltip, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger }
