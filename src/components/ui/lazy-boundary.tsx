import {
  Component,
  Suspense,
  type ErrorInfo,
  type ReactNode,
} from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

interface LazyBoundaryProps {
  children: ReactNode;
  /** Human-readable surface name used by both loading and error states. */
  label: string;
  className?: string;
  /** Overlay chunks retain the visible shell and load inside a compact card. */
  presentation?: "surface" | "overlay";
}

interface ChunkErrorBoundaryProps extends LazyBoundaryProps {}

interface ChunkErrorBoundaryState {
  error: Error | null;
}

/** A rejected dynamic import is a render error, not a Suspense state. Keep the
 * shell usable and give the user a deterministic recovery path. */
class ChunkErrorBoundary extends Component<
  ChunkErrorBoundaryProps,
  ChunkErrorBoundaryState
> {
  state: ChunkErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ChunkErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[lazy-boundary] Failed to load ${this.props.label}`, error, info);
  }

  render() {
    if (!this.state.error) return this.props.children;
    const overlay = this.props.presentation === "overlay";
    return (
      <div
        role="alert"
        className={cn(
          "flex min-h-20 w-full items-center justify-center px-4 text-center text-sm text-muted-foreground",
          overlay ? "bg-background/20 backdrop-blur-[1px]" : "bg-background",
          this.props.className,
        )}
      >
        <div
          className={cn(
            "flex flex-col items-center justify-center gap-2",
            overlay &&
              "rounded-lg border border-border/70 bg-popover/95 px-4 py-3 shadow-xl",
          )}
        >
          <span>Couldn’t load {this.props.label}.</span>
          <Button size="sm" variant="outline" onClick={() => window.location.reload()}>
            Reload Codemux
          </Button>
        </div>
      </div>
    );
  }
}

/** Shared loading/error contract for route, overlay, and pane chunks. */
export function LazyBoundary({
  children,
  label,
  className,
  presentation = "surface",
}: LazyBoundaryProps) {
  const overlay = presentation === "overlay";
  const loading = (
    <div
      role="status"
      aria-label={`Loading ${label}`}
      className={cn(
        "flex min-h-20 w-full items-center justify-center px-4 text-sm text-muted-foreground",
        overlay ? "bg-background/20 backdrop-blur-[1px]" : "bg-background",
        className,
      )}
    >
      <span
        className={cn(
          "flex items-center gap-2",
          overlay &&
            "rounded-lg border border-border/70 bg-popover/95 px-3 py-2 shadow-xl",
        )}
      >
        {overlay && (
          <span
            aria-hidden
            className="h-1.5 w-1.5 rounded-full bg-primary motion-safe:animate-pulse"
          />
        )}
        Loading {label}…
      </span>
    </div>
  );

  return (
    <ChunkErrorBoundary
      key={label}
      label={label}
      className={className}
      presentation={presentation}
    >
      <Suspense fallback={loading}>{children}</Suspense>
    </ChunkErrorBoundary>
  );
}
