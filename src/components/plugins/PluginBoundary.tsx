import { Component, type ErrorInfo, type ReactNode } from "react";

/**
 * Keeps a fault in plugin UI inside its own spot: a card falls back to its
 * recorded summary, a menu entry or chip to nothing, and the transcript,
 * sidebar or menu around it keeps working. A new `resetKey` (another card,
 * a refreshed catalogue) tries again.
 */
export class PluginBoundary extends Component<
  { children: ReactNode; fallback: ReactNode; resetKey?: string; label: string },
  { failed: boolean; resetKey: string | undefined }
> {
  state = { failed: false, resetKey: this.props.resetKey };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  static getDerivedStateFromProps(
    props: { resetKey?: string },
    state: { failed: boolean; resetKey: string | undefined },
  ) {
    return props.resetKey === state.resetKey ? null : { failed: false, resetKey: props.resetKey };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[plugins] ${this.props.label} failed to render`, error, info.componentStack);
  }

  render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}
