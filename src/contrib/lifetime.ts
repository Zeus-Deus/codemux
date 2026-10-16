/**
 * Plugin ownership of disposers and host subscriptions.
 *
 * Every plugin incarnation has one `Owner`. Its disposers run, last first,
 * when the plugin is turned off, reloaded or removed; a disposer attached
 * after that (a late async registration) runs immediately instead of leaking.
 *
 * Host subscriptions (`host.chat.onEvent`, `host.theme.subscribe`, …) attach
 * to the owner that is current when they are made:
 * - while `register()` runs synchronously;
 * - inside every plugin callback the host invokes (ctx timers and listeners,
 *   host subscription callbacks), which re-enter their owner;
 * - after an `await` in an async `register()`: the registration still in
 *   flight owns them (JS has no async context, so with two plugins awaiting
 *   at the same moment the most recent registration is chosen).
 */
export class Owner {
  private disposers: (() => void)[] = [];
  private done = false;

  constructor(readonly pluginId: string) {}

  get ended(): boolean {
    return this.done;
  }

  track(dispose: () => void): void {
    if (this.done) {
      safely(this.pluginId, dispose, "late disposer failed");
      return;
    }
    this.disposers.push(dispose);
  }

  /** Run every disposer, last first, each isolated. Idempotent. */
  end(): void {
    if (this.done) return;
    this.done = true;
    const disposers = this.disposers.reverse();
    this.disposers = [];
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

let current: Owner | null = null;
/** Async registrations still in flight, in start order. */
const pending: Owner[] = [];

export function runAs<T>(owner: Owner | null, fn: () => T): T {
  const previous = current;
  current = owner;
  try {
    return fn();
  } finally {
    current = previous;
  }
}

/** The plugin making this call, if the host can tell. */
export function currentOwner(): Owner | null {
  return current ?? pending[pending.length - 1] ?? null;
}

/** Wrap a plugin callback so host calls made inside it belong to `owner`. */
export function bindOwner<A extends unknown[], R>(owner: Owner | null, fn: (...args: A) => R): (...args: A) => R {
  if (!owner) return fn;
  return (...args: A) => runAs(owner, () => fn(...args));
}

/** Mark an async `register()` as in flight until `settled`. */
export function trackAsyncRegistration(owner: Owner, settled: Promise<unknown>): void {
  pending.push(owner);
  const done = () => {
    const i = pending.indexOf(owner);
    if (i >= 0) pending.splice(i, 1);
  };
  void settled.then(done, done);
}

/** Attach `dispose` to the plugin making this call, if any. */
export function trackAmbient(dispose: () => void): () => void {
  currentOwner()?.track(dispose);
  return dispose;
}

/** Test-only. */
export function resetLifetimeForTests(): void {
  current = null;
  pending.length = 0;
}
