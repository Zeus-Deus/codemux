import * as React from "react"
import { Switch as SwitchPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

function Switch({
  className,
  size = "default",
  ...props
}: React.ComponentProps<typeof SwitchPrimitive.Root> & {
  size?: "sm" | "default"
}) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      data-size={size}
      className={cn(
        // Geometry derives from one thumb size: the track is the thumb plus a
        // 2px rail on every side, two thumbs wide less the overlap. The focus
        // ring sits inside the track with a 1px gap, like the button's.
        "peer group/switch relative inline-flex h-[calc(var(--thumb-size)+2px)] w-[calc(var(--thumb-size)*2-2px)] shrink-0 items-center rounded-full p-[2px] transition-[background-color,box-shadow] duration-100 outline-none after:absolute after:-inset-x-3 after:-inset-y-2 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset focus-visible:ring-offset-1 focus-visible:ring-offset-background aria-invalid:ring-2 aria-invalid:ring-destructive/40 data-[size=default]:[--thumb-size:--spacing(4)] data-[size=sm]:[--thumb-size:--spacing(3.5)] data-checked:bg-foreground data-unchecked:bg-input dark:data-unchecked:bg-input/80 data-disabled:cursor-not-allowed data-disabled:opacity-64",
        className
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        // A press stretches the thumb toward its travel, anchored at the side
        // it rests on, so the switch answers the press before it flips.
        className="pointer-events-none block size-[calc(var(--thumb-size)-2px)] origin-left rounded-full bg-background shadow-sm/5 ring-0 transition-[translate,scale] duration-100 group-active/switch:not-data-disabled:scale-x-110 data-checked:origin-right data-checked:translate-x-[calc(var(--thumb-size)-4px)] dark:data-checked:bg-background data-unchecked:translate-x-0 dark:data-unchecked:bg-foreground"
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
