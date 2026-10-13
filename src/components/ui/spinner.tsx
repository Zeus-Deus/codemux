import { cn } from "@/lib/utils"
import { Loader2Icon } from "lucide-react"

/**
 * The one loading spinner. Size it with the icon ladder (`size-3`,
 * `size-3.5`, default `size-4`) through `className`. `label` names the wait
 * for assistive tech ("Loading" by default). Under reduced motion the
 * `data-slot` hook in globals.css swaps the rotation for a slow fade.
 */
function Spinner({
  className,
  label = "Loading",
  ...props
}: React.ComponentProps<"svg"> & { label?: string }) {
  return (
    <Loader2Icon
      data-slot="spinner"
      role="status"
      aria-label={label}
      className={cn("size-4 shrink-0 animate-spin", className)}
      {...props}
    />
  )
}

export { Spinner }
