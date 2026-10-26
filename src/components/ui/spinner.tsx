import { cn } from "@/lib/utils"
import { Loader2Icon } from "lucide-react"

/** How long a spinner waits before it shows, so a wait that settles quickly
 *  never flashes one. Mirrors `--motion-slow` in the `[data-delayed]` rule
 *  in globals.css. */
export const SPINNER_DELAY_MS = 250

/**
 * The one loading spinner. Size it with the icon ladder (`size-3`,
 * `size-3.5`, default `size-4`) through `className`. `label` names the wait
 * for assistive tech ("Loading" by default). Under reduced motion the
 * `data-slot` hook in globals.css swaps the rotation for a slow fade.
 *
 * It stays invisible for `SPINNER_DELAY_MS` while keeping its size, so
 * nothing shifts when it appears. Pass `immediate` when the caller already
 * waited, e.g. behind `useDelayedPending`.
 */
function Spinner({
  className,
  label = "Loading",
  immediate = false,
  ...props
}: React.ComponentProps<"svg"> & { label?: string; immediate?: boolean }) {
  return (
    <Loader2Icon
      data-slot="spinner"
      data-delayed={immediate ? undefined : ""}
      role="status"
      aria-label={label}
      className={cn("size-4 shrink-0 animate-spin", className)}
      {...props}
    />
  )
}

export { Spinner }
