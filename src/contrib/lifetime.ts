/**
 * Ambient disposer tracking. While a plugin's `register()` runs (and inside
 * tracked callbacks), host subscriptions such as `host.chat.onEvent` attach
 * their cleanup to that plugin, so even a bare `host.*` subscription made
 * synchronously in `register` is removed when the plugin is turned off.
 */
type Track = (dispose: () => void) => void;

let ambient: Track | null = null;

export function withAmbientTracker<T>(track: Track, fn: () => T): T {
  const previous = ambient;
  ambient = track;
  try {
    return fn();
  } finally {
    ambient = previous;
  }
}

/** Attach `dispose` to the plugin currently registering, if any. */
export function trackAmbient(dispose: () => void): () => void {
  ambient?.(dispose);
  return dispose;
}
