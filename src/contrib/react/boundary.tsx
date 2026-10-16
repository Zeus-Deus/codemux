import {
  Component,
  createElement,
  type ErrorInfo,
  type ReactNode,
} from "react";
import { AlertTriangle, RotateCw } from "lucide-react";
import { pluginIdOf, pluginLabel } from "../attribution";

interface ContribRenderProps<P> {
  render: (props: P) => ReactNode;
  props?: P;
}

/** Mount a contribution's render as a component, so its hooks and errors
 *  belong to the contribution rather than the surface that hosts it. */
export function ContribRender<P extends object>({ render, props }: ContribRenderProps<P>) {
  return createElement(render as (p: P) => ReactNode, props ?? ({} as P));
}

interface BoundaryProps {
  children: ReactNode;
  /** Contribution id (`<pluginId>:<localId>`); names the plugin in the chip. */
  id: string;
  /** `chip`: a tiny inline fallback for bars; `block`: a padded row. */
  variant?: "chip" | "block" | "silent";
}

interface BoundaryState {
  error: Error | null;
}

/**
 * The blast wall between plugin render code and the app. A throwing
 * contribution degrades to a small "Plugin X failed" chip with Retry in its
 * own slot; the surrounding surface and every other plugin keep working.
 */
export class ContribBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): BoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[plugins] ${this.props.id} failed to render`, error, info.componentStack);
  }

  private reset = () => this.setState({ error: null });

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    if (this.props.variant === "silent") return null;
    const name = pluginLabel(pluginIdOf(this.props.id));
    return (
      <span
        role="alert"
        title={`${this.props.id}: ${error.message}`}
        data-plugin-error={this.props.id}
        className={
          this.props.variant === "block"
            ? "flex items-center gap-2 px-3 py-2 text-caption text-destructive"
            : "inline-flex items-center gap-1 rounded px-1.5 text-caption text-destructive"
        }
      >
        <AlertTriangle className="size-3 shrink-0" aria-hidden />
        <span className="truncate">Plugin {name} failed</span>
        <button
          type="button"
          onClick={this.reset}
          className="inline-flex items-center gap-0.5 rounded px-1 text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <RotateCw className="size-3" aria-hidden />
          Retry
        </button>
      </span>
    );
  }
}
