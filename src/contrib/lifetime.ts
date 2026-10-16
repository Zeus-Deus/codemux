/**
 * Plugin ownership of disposers and host subscriptions.
 *
 * Every plugin incarnation has one `Owner`. Its live disposers run, last
 * first, when the plugin is turned off, reloaded or removed. A resource that
 * finishes or is cancelled earlier releases itself, so a long-lived plugin
 * never accumulates them. A disposer attached after the owner ended runs at
 * once instead of leaking.
 *
 * Which plugin is calling a bare `host.*` subscription is known for certain
 * only while `register()` runs synchronously and inside callbacks the host
 * invokes for a plugin (ctx timers and listeners, host subscription
 * callbacks). JS has no async context, so after an `await` the caller is
 * unknown and nothing here guesses: `ctx.host` is bound to its incarnation
 * and is the way to subscribe there.
 */
export class Owner {
  private live = new Set<() => void>();
  private done = false;

  constructor(readonly pluginId: string) {}

  get ended(): boolean {
    return this.done;
  }

  /** Live disposers (diagnostics and tests). */
  get size(): number {
    return this.live.size;
  }

  /**
   * Attach a cleanup. Returns `release`, which forgets it without running
   * it. After the owner ended, the cleanup runs immediately.
   */
  track(dispose: () => void): () => void {
    if (this.done) {
      safely(this.pluginId, dispose, "late disposer failed");
      return () => {};
    }
    // Wrapped, so the same function tracked twice is two entries.
    const entry = () => dispose();
    this.live.add(entry);
    return () => void this.live.delete(entry);
  }

  /** Run every live disposer, last first, each isolated. Idempotent. */
  end(): void {
    if (this.done) return;
    this.done = true;
    const disposers = [...this.live].reverse();
    this.live.clear();
    for (const dispose of disposers) safely(this.pluginId, dispose, "disposer failed during unload");
  }
}

function safely(pluginId: string, fn: () => void, what: string) {
  try {
    fn();
  } catch (error) {
    console.error(`[plugins] ${pluginId}: ${what}`, error);
  }
}

/**
 * A cleanup owned by `owner` that may also be called early. Calling it runs
 * the cleanup once and releases it from the owner; unloading runs it if it
 * is still live; for an ended owner it runs at once.
 */
export function ownedDisposer(owner: Owner | null, cleanup: () => void): () => void {
  let live = true;
  const run = () => {
    if (!live) return;
    live = false;
    cleanup();
  };
  if (!owner) return run;
  const release = owner.track(run);
  return () => {
    release();
    run();
  };
}

let current: Owner | null = null;

export function runAs<T>(owner: Owner | null, fn: () => T): T {
  const previous = current;
  current = owner;
  try {
    return fn();
  } finally {
    current = previous;
  }
}

/** The plugin making this call, when that is certain; else null. */
export function currentOwner(): Owner | null {
  return current;
}

/** Wrap a plugin callback so host calls made inside it belong to `owner`. */
export function bindOwner<A extends unknown[], R>(owner: Owner | null, fn: (...args: A) => R): (...args: A) => R {
  if (!owner) return fn;
  return (...args: A) => runAs(owner, () => fn(...args));
}

/** Test-only. */
export function resetLifetimeForTests(): void {
  current = null;
}
