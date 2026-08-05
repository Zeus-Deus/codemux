/**
 * Connects chat scroll viewports to the floating titlebar. A workspace can
 * contain more than one chat pane, so each viewport publishes under its own
 * token and the titlebar activates while any mounted viewport has content
 * scrolled beneath it.
 */
type SourceToken = symbol;
type Listener = () => void;

const sourcesByWorkspace = new Map<string, Map<SourceToken, boolean>>();
const listeners = new Set<Listener>();

function workspaceHasContentUnder(workspaceId: string): boolean {
  const sources = sourcesByWorkspace.get(workspaceId);
  if (!sources) return false;
  for (const contentUnder of sources.values()) {
    if (contentUnder) return true;
  }
  return false;
}

export function publishTitlebarContentUnder(
  workspaceId: string,
  source: SourceToken,
  contentUnder: boolean,
): void {
  const before = workspaceHasContentUnder(workspaceId);
  let sources = sourcesByWorkspace.get(workspaceId);
  if (!sources) {
    sources = new Map();
    sourcesByWorkspace.set(workspaceId, sources);
  }
  if (sources.get(source) === contentUnder) return;
  sources.set(source, contentUnder);
  const after = workspaceHasContentUnder(workspaceId);
  if (before !== after) listeners.forEach((listener) => listener());
}

export function clearTitlebarContentUnder(
  workspaceId: string,
  source: SourceToken,
): void {
  const before = workspaceHasContentUnder(workspaceId);
  const sources = sourcesByWorkspace.get(workspaceId);
  if (!sources?.delete(source)) return;
  if (sources.size === 0) sourcesByWorkspace.delete(workspaceId);
  const after = workspaceHasContentUnder(workspaceId);
  if (before !== after) listeners.forEach((listener) => listener());
}

export function getTitlebarContentUnder(workspaceId: string | null): boolean {
  return workspaceId ? workspaceHasContentUnder(workspaceId) : false;
}

export function subscribeTitlebarContentUnder(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

// ── Live transcript element registry ──
//
// `TitleBar` measures whether its floating islands physically intersect a
// chat transcript's centered reading column. It cannot snapshot
// `document.querySelectorAll` once and be done: `PaneContainer` renders
// only the active surface, so every tab / workspace switch unmounts the
// current `MessageList` and mounts a fresh one with a brand-new DOM node.
// Each viewport therefore registers itself here, and the version counter
// gives the titlebar a `useSyncExternalStore`-friendly scalar to re-key
// its measurement effect on — so it always observes live nodes.

const transcriptElements = new Set<HTMLElement>();
const transcriptListeners = new Set<Listener>();
let transcriptVersion = 0;

function bumpTranscriptVersion(): void {
  transcriptVersion += 1;
  transcriptListeners.forEach((listener) => listener());
}

/** Register a mounted chat scroll viewport. Returns its unregister fn. */
export function registerTitlebarTranscript(element: HTMLElement): () => void {
  if (!transcriptElements.has(element)) {
    transcriptElements.add(element);
    bumpTranscriptVersion();
  }
  return () => {
    if (transcriptElements.delete(element)) bumpTranscriptVersion();
  };
}

export function getTitlebarTranscriptElements(): HTMLElement[] {
  return Array.from(transcriptElements);
}

/** Monotonic counter, bumped whenever the registered element set changes. */
export function getTitlebarTranscriptVersion(): number {
  return transcriptVersion;
}

export function subscribeTitlebarTranscripts(listener: Listener): () => void {
  transcriptListeners.add(listener);
  return () => transcriptListeners.delete(listener);
}
