import { clsx, type ClassValue } from "clsx"
import { extendTailwindMerge } from "tailwind-merge"

/**
 * The UI type scale in `globals.css` (`@theme --text-*`). tailwind-merge only
 * knows Tailwind's stock sizes, so without this `text-label` would be read as a
 * text colour and `cn("text-label", "text-muted-foreground")` would drop it.
 */
export const UI_TEXT_SIZES = ["micro", "caption", "label", "body-sm", "body", "body-lg", "title"] as const

/** The control radius and elevation tokens in `globals.css`, registered for
 *  the same reason: an unknown `rounded-control` would survive next to a
 *  caller's `rounded-full` and let stylesheet order pick the winner. */
const UI_RADII = ["control"] as const
const UI_SHADOWS = ["popup", "dialog"] as const
const UI_INSET_SHADOWS = ["bevel", "highlight", "pressed"] as const

const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      text: [...UI_TEXT_SIZES],
      radius: [...UI_RADII],
      shadow: [...UI_SHADOWS],
      "inset-shadow": [...UI_INSET_SHADOWS],
    },
  },
})

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
