/**
 * Unsent composer text, kept across a reload of the window.
 *
 * A bound thread's composer text lives only in the in-memory chat store, so
 * reloading the page — including the app's own recovery reload after the
 * renderer crashes or freezes (src-tauri/src/webview_recovery.rs) — used to
 * throw away whatever the user had typed. This mirrors each thread's text into
 * `sessionStorage`, which survives a reload (and a crash of the web content
 * process) but not closing the app, and seeds a thread's slice from it the
 * first time the new page creates that slice.
 *
 * Text only: staged attachments and picker choices are not kept.
 */

const STORAGE_KEY = "codemux:composer-drafts";
const WRITE_DEBOUNCE_MS = 250;

type Drafts = Record<string, string>;

function storage(): Storage | null {
  try {
    return typeof window !== "undefined" ? window.sessionStorage : null;
  } catch {
    // Access can throw when storage is disabled.
    return null;
  }
}

function readAll(): Drafts {
  const raw = storage()?.getItem(STORAGE_KEY);
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const drafts: Drafts = {};
    for (const [threadId, text] of Object.entries(parsed)) {
      if (typeof text === "string" && text.length > 0) drafts[threadId] = text;
    }
    return drafts;
  } catch {
    return {};
  }
}

/** Drafts saved by the previous page, read once. */
let restored: Drafts | null = null;
/** What this page has written, applied on top of `restored`. */
let current: Drafts | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

function drafts(): Drafts {
  if (current === null) {
    restored = readAll();
    current = { ...restored };
  }
  return current;
}

/** Unsent text the previous page left for `threadId`, or "". */
export function restoredComposerDraft(threadId: string): string {
  drafts();
  return restored?.[threadId] ?? "";
}

function flush() {
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
  const store = storage();
  if (!store || current === null) return;
  try {
    if (Object.keys(current).length === 0) store.removeItem(STORAGE_KEY);
    else store.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch (err) {
    // Quota or disabled storage: the draft stays in memory as before.
    console.warn("[composer-draft-backup] could not save drafts:", err);
  }
}

/** Record `text` as the unsent draft for `threadId`. Writes are debounced. */
export function saveComposerDraft(threadId: string, text: string) {
  const all = drafts();
  if ((all[threadId] ?? "") === text) return;
  if (text.length > 0) all[threadId] = text;
  else delete all[threadId];
  // Once the new page has a live value, the old one must not be offered again.
  if (restored) delete restored[threadId];
  if (timer === null) timer = setTimeout(flush, WRITE_DEBOUNCE_MS);
}

type DraftSource = {
  getState: () => { threads: Record<string, { inputDraft: string }> };
  subscribe: (
    listener: (
      state: { threads: Record<string, { inputDraft: string }> },
      prev: { threads: Record<string, { inputDraft: string }> },
    ) => void,
  ) => () => void;
};

/**
 * Mirror every thread's `inputDraft` from `store` into session storage.
 * Evicted slices never held a draft (a slice with unsent text is never
 * evicted), so a thread missing from the store leaves its entry alone.
 */
export function installComposerDraftBackup(store: DraftSource): () => void {
  const unsubscribe = store.subscribe((state, prev) => {
    if (state.threads === prev.threads) return;
    for (const [threadId, slice] of Object.entries(state.threads)) {
      const before = prev.threads[threadId];
      if (before === slice) continue;
      if (before && before.inputDraft === slice.inputDraft) continue;
      saveComposerDraft(threadId, slice.inputDraft);
    }
  });
  const onHide = () => flush();
  if (typeof window !== "undefined" && typeof window.addEventListener === "function") {
    window.addEventListener("pagehide", onHide);
  }
  return () => {
    unsubscribe();
    if (typeof window !== "undefined" && typeof window.removeEventListener === "function") {
      window.removeEventListener("pagehide", onHide);
    }
  };
}

/** Test-only: forget cached state so the next read goes to storage. */
export function resetComposerDraftBackupForTests() {
  if (timer !== null) clearTimeout(timer);
  timer = null;
  restored = null;
  current = null;
}

/** Test-only: write pending drafts now. */
export function flushComposerDraftBackupForTests() {
  flush();
}
