/**
 * A plugin's native sidebar tree, laid out by the host. The plugin only says
 * where chats go; the host resolves every reference against rows it already
 * lists, renders its own rows, and keeps every listed row reachable: a row the
 * tree leaves out stays where the host would put it.
 *
 * In Codemux a sidebar row is a workspace. A tree's `thread` node names a chat
 * thread; it places the workspace that holds that chat. `project` nodes name
 * the domain's project id (`project_uid`, else the project root, else cwd).
 */
import type { PluginTreeNode } from "./contract";

export interface PluginTreeRow<TItem> {
  readonly item: TItem;
  readonly key: string;
  readonly children: ReadonlyArray<PluginTreeRow<TItem>>;
}

export type PluginTreeHeading<TProject> =
  | { readonly kind: "group"; readonly title: string }
  | { readonly kind: "project"; readonly project: TProject }
  /** Live rows the tree does not place. */
  | { readonly kind: "other" };

export interface PluginTreeSection<TProject, TItem> {
  readonly key: string;
  /** Null for rows the tree places at its root, or rows continuing after a nested heading. */
  readonly heading: PluginTreeHeading<TProject> | null;
  /** Headings above this one. */
  readonly depth: number;
  /** Indent of the rows. */
  readonly rowDepth: number;
  readonly folded: boolean;
  /** The plugin's initial fold, for toggling; the person's choice is kept against it. */
  readonly collapsedByDefault: boolean;
  /** Rows under this heading, nested headings included. */
  readonly count: number;
  /** Rows to render: all of them, or only the open row while folded. */
  readonly rows: ReadonlyArray<PluginTreeRow<TItem>>;
  readonly items: ReadonlyArray<TItem>;
}

export const PLUGIN_TREE_OTHER_SECTION_KEY = "\0plugin-tree-other";

/** Persisted per device: an explicit unfold, which wins over `collapsedByDefault`. */
const expandedFoldKey = (sectionKey: string) => `+${sectionKey}`;

function isSectionFolded(sectionKey: string, collapsedByDefault: boolean, folds: ReadonlySet<string>): boolean {
  if (folds.has(sectionKey)) return true;
  if (folds.has(expandedFoldKey(sectionKey))) return false;
  return collapsedByDefault;
}

/** The fold entries after the person toggles a section, leaving at most one entry for it. */
export function togglePluginTreeFold(
  folds: ReadonlyArray<string>,
  sectionKey: string,
  collapsedByDefault: boolean,
): ReadonlyArray<string> {
  const folded = isSectionFolded(sectionKey, collapsedByDefault, new Set(folds));
  const expandedKey = expandedFoldKey(sectionKey);
  const rest = folds.filter((key) => key !== sectionKey && key !== expandedKey);
  if (folded === !collapsedByDefault) return rest;
  return [...rest, folded ? expandedKey : sectionKey];
}

const collapsedByDefault = (node: PluginTreeNode) => node.collapsedByDefault === true;

interface HeadingNode<TProject, TItem> {
  readonly kind: "heading";
  readonly key: string;
  readonly heading: PluginTreeHeading<TProject>;
  readonly collapsedByDefault: boolean;
  readonly children: Array<TreeEntry<TProject, TItem>>;
}

interface RowNode<TProject, TItem> {
  readonly kind: "row";
  readonly item: TItem;
  readonly key: string;
  /** Headings never nest under a row; one placed under a row goes beside it. */
  readonly heading: HeadingNode<TProject, TItem> | null;
  readonly children: Array<RowNode<TProject, TItem>>;
}

type TreeEntry<TProject, TItem> = HeadingNode<TProject, TItem> | RowNode<TProject, TItem>;

/**
 * - References the host does not list (archived, filtered, closed, outside the
 *   grants) are skipped and their children move up, so no listed row hides
 *   behind a missing parent.
 * - Nesting is display only; it says nothing about who owns or runs a chat.
 * - A row the tree leaves out follows its parent row when that is placed;
 *   otherwise a live one goes to the "other" section, and a parked one
 *   (snoozed, settled) stays on the host's shelf.
 * - Group and project headings fold, keeping only the open row.
 */
export function buildPluginTreeLayout<TProject, TItem>(input: {
  readonly nodes: ReadonlyArray<PluginTreeNode>;
  /** Every row the host lists, in its order. */
  readonly items: ReadonlyArray<TItem>;
  readonly keyOf: (item: TItem) => string;
  /** The row this one follows in the host's own nesting, or null. */
  readonly parentKeyOf: (item: TItem) => string | null;
  /** The row that holds a chat thread, or null when the host lists none. */
  readonly rowForThread: (threadId: string) => string | null;
  /** Rows that belong in the list rather than on a shelf when the tree leaves them out. */
  readonly liveKeys: ReadonlySet<string>;
  /** The host's project for a project id, or null when it lists none. */
  readonly project: (projectId: string) => { readonly key: string; readonly project: TProject } | null;
  /** Whether an empty project heading belongs in the current scope. */
  readonly showsEmptyProject: (project: TProject) => boolean;
  readonly folds: ReadonlySet<string>;
  /** The open row, which a folded heading still shows. */
  readonly routeKey: string | null;
}): {
  readonly sections: ReadonlyArray<PluginTreeSection<TProject, TItem>>;
  /** Rows the tree placed, directly or by following a placed parent. */
  readonly placedKeys: ReadonlySet<string>;
} {
  type Heading = HeadingNode<TProject, TItem>;
  type Row = RowNode<TProject, TItem>;
  const { keyOf } = input;
  const itemsByKey = new Map(input.items.map((item) => [keyOf(item), item]));
  const placedKeys = new Set<string>();
  const roots: Array<TreeEntry<TProject, TItem>> = [];
  const rootProjects = new Map<string, Heading>();

  const nodesById = new Map(input.nodes.map((node) => [node.id, node]));
  interface Resolved {
    readonly entry: TreeEntry<TProject, TItem> | null;
    readonly attachTo: Heading | Row | null;
    /** Where its children go: itself, or the ancestor that stands in for a skipped node. */
    readonly container: Heading | Row | null;
  }
  // Snapshots may list a child before its parent, so parents resolve on demand;
  // entries attach afterwards in snapshot order, so siblings keep the plugin's order.
  const resolved = new Map<string, Resolved>();
  const resolving = new Set<string>();
  const resolve = (node: PluginTreeNode): Resolved => {
    const known = resolved.get(node.id);
    if (known !== undefined) return known;
    resolving.add(node.id);
    // A parent chain that loops back leaves the node it reached unparented.
    const parentNode =
      node.parentId === undefined || resolving.has(node.parentId) ? undefined : nodesById.get(node.parentId);
    const parent = parentNode === undefined ? null : resolve(parentNode).container;
    const headingParent = parent?.kind === "row" ? parent.heading : parent;
    const result = ((): Resolved => {
      if (node.kind === "thread") {
        const key = input.rowForThread(node.threadId);
        const item = key === null ? undefined : itemsByKey.get(key);
        if (key === null || item === undefined || placedKeys.has(key)) {
          return { entry: null, attachTo: null, container: parent };
        }
        placedKeys.add(key);
        const entry: Row = { kind: "row", item, key, heading: headingParent, children: [] };
        return { entry, attachTo: parent, container: entry };
      }
      if (node.kind === "group") {
        const entry: Heading = {
          kind: "heading",
          key: `tree:${node.id}`,
          heading: { kind: "group", title: node.title },
          collapsedByDefault: collapsedByDefault(node),
          children: [],
        };
        return { entry, attachTo: headingParent, container: entry };
      }
      const project = input.project(node.projectId);
      if (project === null) return { entry: null, attachTo: null, container: headingParent };
      const merged = headingParent === null ? rootProjects.get(project.key) : undefined;
      if (merged !== undefined) return { entry: null, attachTo: null, container: merged };
      const entry: Heading = {
        kind: "heading",
        key: headingParent === null ? `project:${project.key}` : `tree:${node.id}`,
        heading: { kind: "project", project: project.project },
        collapsedByDefault: collapsedByDefault(node),
        children: [],
      };
      if (headingParent === null) rootProjects.set(project.key, entry);
      return { entry, attachTo: headingParent, container: entry };
    })();
    resolving.delete(node.id);
    resolved.set(node.id, result);
    return result;
  };
  for (const node of input.nodes) {
    const { entry, attachTo } = resolve(node);
    if (entry === null) continue;
    if (attachTo === null) roots.push(entry);
    else if (attachTo.kind === "heading") attachTo.children.push(entry);
    else if (entry.kind === "row") attachTo.children.push(entry);
  }

  // A row the tree leaves out follows its nearest placed ancestor, in host order.
  const parentKey = (item: TItem) => input.parentKeyOf(item);
  const treePlaced = new Set(placedKeys);
  const anchorOf = (item: TItem): string | null => {
    const seen = new Set<string>();
    let cursor = parentKey(item);
    while (cursor !== null && !seen.has(cursor)) {
      if (treePlaced.has(cursor)) return cursor;
      const next = itemsByKey.get(cursor);
      if (next === undefined) return null;
      seen.add(cursor);
      cursor = parentKey(next);
    }
    return null;
  };
  const followers = new Map<string, TItem[]>();
  const others: TItem[] = [];
  for (const item of input.items) {
    const key = keyOf(item);
    if (treePlaced.has(key)) continue;
    if (anchorOf(item) === null) {
      if (input.liveKeys.has(key)) others.push(item);
      continue;
    }
    placedKeys.add(key);
  }
  const otherKeys = new Set(others.map(keyOf));
  for (const item of input.items) {
    const key = keyOf(item);
    const parent = parentKey(item);
    if (treePlaced.has(key) || parent === null) continue;
    const follows = placedKeys.has(key) && (placedKeys.has(parent) || treePlaced.has(parent));
    if (!follows && !(otherKeys.has(key) && otherKeys.has(parent))) continue;
    const list = followers.get(parent);
    if (list) list.push(item);
    else followers.set(parent, [item]);
  }

  const followerRow = (item: TItem, seen: Set<string>): PluginTreeRow<TItem> => {
    const key = keyOf(item);
    seen.add(key);
    return {
      item,
      key,
      children: (followers.get(key) ?? [])
        .filter((child) => !seen.has(keyOf(child)))
        .map((child) => followerRow(child, seen)),
    };
  };
  const placedRow = (entry: Row): PluginTreeRow<TItem> => {
    const own = followerRow(entry.item, new Set([entry.key]));
    return { ...own, children: [...entry.children.map(placedRow), ...own.children] };
  };
  const flatten = (rows: ReadonlyArray<PluginTreeRow<TItem>>): TItem[] =>
    rows.flatMap((row) => [row.item, ...flatten(row.children)]);
  const findRoute = (rows: ReadonlyArray<PluginTreeRow<TItem>>): PluginTreeRow<TItem>[] => {
    for (const row of rows) {
      if (row.key === input.routeKey) return [{ ...row, children: [] }];
      const nested = findRoute(row.children);
      if (nested.length > 0) return nested;
    }
    return [];
  };
  const section = (
    key: string,
    heading: PluginTreeHeading<TProject> | null,
    depth: number,
    rowDepth: number,
    rows: ReadonlyArray<PluginTreeRow<TItem>>,
    count = flatten(rows).length,
    folded = false,
    defaultFolded = false,
  ): PluginTreeSection<TProject, TItem> => ({
    key,
    heading,
    depth,
    rowDepth,
    folded,
    collapsedByDefault: defaultFolded,
    count,
    rows,
    items: flatten(rows),
  });

  interface Laid {
    readonly rows: PluginTreeRow<TItem>[];
    count: number;
    readonly allRows: PluginTreeRow<TItem>[];
    readonly sections: PluginTreeSection<TProject, TItem>[];
  }
  const lay = (entries: ReadonlyArray<TreeEntry<TProject, TItem>>, depth: number, key: string): Laid => {
    const laid: Laid = { rows: [], count: 0, allRows: [], sections: [] };
    let loose: PluginTreeRow<TItem>[] = [];
    let continuation = 0;
    const flush = () => {
      if (loose.length === 0) return;
      laid.sections.push(section(`${key}:${continuation++}`, null, depth, depth, loose));
      loose = [];
    };
    for (const entry of entries) {
      if (entry.kind === "row") {
        const row = placedRow(entry);
        laid.count += flatten([row]).length;
        laid.allRows.push(row);
        if (laid.sections.length === 0) laid.rows.push(row);
        else loose.push(row);
        continue;
      }
      flush();
      const inner = lay(entry.children, depth + 1, entry.key);
      laid.count += inner.count;
      laid.allRows.push(...inner.allRows);
      if (inner.count === 0 && !(entry.heading.kind === "project" && input.showsEmptyProject(entry.heading.project))) {
        continue;
      }
      const folded = inner.count > 0 && isSectionFolded(entry.key, entry.collapsedByDefault, input.folds);
      laid.sections.push(
        section(
          entry.key,
          entry.heading,
          depth,
          depth + 1,
          folded ? findRoute(inner.allRows) : inner.rows,
          inner.count,
          folded,
          entry.collapsedByDefault,
        ),
      );
      if (!folded) laid.sections.push(...inner.sections);
    }
    flush();
    return laid;
  };

  const top = lay(roots, 0, "tree-root");
  const sections: PluginTreeSection<TProject, TItem>[] = [];
  if (top.rows.length > 0) sections.push(section("tree-root", null, 0, 0, top.rows));
  sections.push(...top.sections);

  if (others.length > 0) {
    const seen = new Set<string>();
    const rows = others
      .filter((item) => {
        const parent = parentKey(item);
        return parent === null || !otherKeys.has(parent);
      })
      .map((item) => followerRow(item, seen));
    // A lineage cycle would hide every row in it; list those flat instead.
    for (const item of others) if (!seen.has(keyOf(item))) rows.push(followerRow(item, seen));
    const folded = isSectionFolded(PLUGIN_TREE_OTHER_SECTION_KEY, false, input.folds);
    sections.push(
      section(
        PLUGIN_TREE_OTHER_SECTION_KEY,
        { kind: "other" },
        0,
        1,
        folded ? findRoute(rows) : rows,
        flatten(rows).length,
        folded,
      ),
    );
  }
  return { sections, placedKeys };
}

/** Row keys in the order the tree shows them, for jump hints. */
export const pluginTreeRowKeys = <TItem>(
  sections: ReadonlyArray<{ readonly rows: ReadonlyArray<PluginTreeRow<TItem>> }>,
): string[] => {
  const walk = (rows: ReadonlyArray<PluginTreeRow<TItem>>): string[] =>
    rows.flatMap((row) => [row.key, ...walk(row.children)]);
  return sections.flatMap((section) => walk(section.rows));
};

/**
 * One line telling the person a failed read changed what they see, or null.
 * Rows the tree did not place stay reachable either way.
 */
export function pluginTreeNotice(title: string, read: { failed: boolean; hasNodes: boolean } | null): string | null {
  if (read === null || !read.failed) return null;
  return read.hasNodes ? `${title} may be out of date.` : `Couldn't load ${title}. Showing Projects.`;
}
