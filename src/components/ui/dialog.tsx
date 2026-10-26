"use client"

import * as React from "react"
import { Dialog as DialogPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { XIcon } from "lucide-react"

function Dialog({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />
}

function DialogTrigger({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

/**
 * How a dialog enters and leaves. `default` zooms in over a blurred backdrop:
 * right for dialogs that interrupt to ask something. `quick` is for overlays
 * summoned from the keyboard many times a day (the command palette, the
 * search overlays): a 100ms opacity fade over a plain dim, no movement and
 * no blur to composite, so the panel is there by the time the user types.
 */
type DialogMotion = "default" | "quick"

const OVERLAY_MOTION: Record<DialogMotion, string> = {
  default:
    "duration-250 ease-out data-closed:duration-150 supports-backdrop-filter:backdrop-blur-xs",
  quick: "duration-100 ease-out",
}

const CONTENT_MOTION: Record<DialogMotion, string> = {
  default:
    "duration-250 ease-drawer data-closed:duration-150 data-closed:ease-out data-open:zoom-in-97 data-closed:zoom-out-97",
  quick: "duration-100 ease-out",
}

function DialogOverlay({
  className,
  motion = "default",
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Overlay> & {
  motion?: DialogMotion
}) {
  return (
    <DialogPrimitive.Overlay
      data-slot="dialog-overlay"
      className={cn(
        "fixed inset-0 isolate z-50 bg-background/60 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        OVERLAY_MOTION[motion],
        className
      )}
      {...props}
    />
  )
}

/**
 * Transform-free centering, for dialogs whose text must stay crisp.
 *
 * `DialogContent` centers with `top-1/2 left-1/2` + `-translate-x-1/2
 * -translate-y-1/2`. A translate puts the element on its own composited
 * layer, and every one of those four halves is fractional as soon as the
 * thing it halves is odd — the window height, the window width, or the
 * dialog's own height. A composited layer sitting on a half pixel gets
 * resampled when it is drawn, and every glyph inside it goes soft on
 * WebKitGTK. Measured on the search dialog before this existed: `top:
 * 330.5px` at an 801px-tall window, `left: 448.5px` at a 1281px-wide one.
 *
 * This replaces both translates with layout: a whole `96px` from the top
 * (the same offset the command palette uses) and auto margins between
 * `inset-x-0` for the horizontal center. Layout offsets are painted snapped
 * to the pixel grid; transform offsets are not.
 *
 * Worth applying to any dialog that is mostly text and is opened often —
 * the search overlays are the ones where the softness was obvious.
 */
export const DIALOG_CRISP_POSITION =
  "inset-x-0 top-24 mx-auto translate-x-0 translate-y-0"

function DialogContent({
  className,
  children,
  showCloseButton = true,
  overlayClassName,
  motion = "default",
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  showCloseButton?: boolean
  overlayClassName?: string
  motion?: DialogMotion
}) {
  return (
    <DialogPortal>
      <DialogOverlay motion={motion} className={overlayClassName} />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          "fixed top-1/2 left-1/2 z-50 grid w-full max-w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 gap-4 rounded-2xl border border-border bg-popover p-(--dialog-pad) text-body shadow-dialog [--dialog-pad:--spacing(5)] outline-none sm:max-w-sm data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
          CONTENT_MOTION[motion],
          className
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close data-slot="dialog-close" asChild>
            <Button
              variant="ghost"
              className="absolute top-3 right-3"
              size="icon-sm"
            >
              <XIcon
              />
              <span className="sr-only">Close</span>
            </Button>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </DialogPortal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2", className)}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean
}) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        "-mx-(--dialog-pad) -mb-(--dialog-pad) flex flex-col-reverse gap-2 rounded-b-[calc(var(--radius-2xl)-1px)] border-t bg-muted/72 px-(--dialog-pad) py-4 sm:flex-row sm:justify-end",
        className
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close asChild>
          <Button variant="outline">Close</Button>
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn(
        "font-heading text-body-lg leading-none font-medium",
        className
      )}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        "text-body text-muted-foreground *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
