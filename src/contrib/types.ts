import type { ReactNode } from "react";

/**
 * Where a contribution came from: `"core"` for the app's own UI, otherwise
 * `plugin:<id>`. Carried on every contribution for debugging and attribution.
 */
export type ContributionSource = "core" | (string & {});

/**
 * The one primitive every plugin area consumes. UI areas mount `render`;
 * data areas (palette commands, keybinds, themes) read `data`.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export interface Contribution<Data = unknown, Props = any> {
  /** Unique within its area. Re-registering the same id replaces it. */
  id: string;
  /** Area constant this contribution targets, e.g. `palette.commands`. */
  area: string;
  source?: ContributionSource;
  title?: string;
  /** Ascending sort key within the area; ties keep insertion order. */
  order?: number;
  /**
   * Visibility predicate. NOT reactive: evaluated only when the area's
   * snapshot is rebuilt (a register/remove in that area). For visibility that
   * follows app state, return null from `render` instead.
   */
  when?: () => boolean;
  /** `false` hides it without unregistering. */
  enabled?: boolean;
  /**
   * Mounted as a React component inside its own error boundary, so hooks and
   * errors belong to the contribution. Keep its identity stable.
   */
  render?: (props: Props) => ReactNode;
  data?: Data;
}
