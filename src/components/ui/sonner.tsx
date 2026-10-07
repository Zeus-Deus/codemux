import { useSyncExternalStore } from "react"
import { Toaster as Sonner, type ToasterProps } from "sonner"
import { useAppTheme } from "@/hooks/use-app-theme"
import { CircleCheckIcon, InfoIcon, TriangleAlertIcon, OctagonXIcon, Loader2Icon } from "lucide-react"

function subscribeVisibility(onChange: () => void) {
  document.addEventListener("visibilitychange", onChange)
  return () => document.removeEventListener("visibilitychange", onChange)
}

const Toaster = ({ ...props }: ToasterProps) => {
  // Sonner's own stylesheet keys description, close and cancel colours off
  // this, so it has to follow the active theme rather than assume dark.
  const scheme = useAppTheme().theme.scheme
  // Sonner holds its dismiss timer while the window is hidden; this class lets
  // the undo bar in globals.css hold with it.
  const hidden = useSyncExternalStore(subscribeVisibility, () => document.hidden, () => false)
  return (
    <Sonner
      theme={scheme}
      className={hidden ? "toaster group cm-toaster-hidden" : "toaster group"}
      position="bottom-right"
      icons={{
        success: (
          <CircleCheckIcon className="size-4" />
        ),
        info: (
          <InfoIcon className="size-4" />
        ),
        warning: (
          <TriangleAlertIcon className="size-4" />
        ),
        error: (
          <OctagonXIcon className="size-4" />
        ),
        loading: (
          <Loader2Icon className="size-4 animate-spin" />
        ),
      }}
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--normal-bg-hover": "var(--accent)",
          "--normal-border-hover": "var(--border)",
          "--border-radius": "var(--radius)",
        } as React.CSSProperties
      }
      toastOptions={{
        classNames: {
          toast: "cn-toast",
        },
      }}
      {...props}
    />
  )
}

export { Toaster }
