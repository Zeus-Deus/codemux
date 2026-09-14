import { clsx, type ClassValue } from "clsx"
import { extendTailwindMerge } from "tailwind-merge"

/**
 * The UI type scale in `globals.css` (`@theme --text-*`). tailwind-merge only
 * knows Tailwind's stock sizes, so without this `text-label` would be read as a
 * text colour and `cn("text-label", "text-muted-foreground")` would drop it.
 */
export const UI_TEXT_SIZES = ["micro", "caption", "label", "body-sm", "body", "body-lg"] as const

const twMerge = extendTailwindMerge({
  extend: { theme: { text: [...UI_TEXT_SIZES] } },
})

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
