/**
 * Imperative bridge between the file-dialog chokepoint and the React-rendered
 * {@link RemotePathPicker} modal.
 *
 * `src/lib/file-dialog.ts` is a plain async function, not a component, so on
 * the web remote client it can't render a modal directly. Instead it calls
 * {@link openRemotePathPicker}, which stashes a resolver and flips this store
 * into an "open" state; the always-mounted `<RemotePathPicker />` observes the
 * store, drives the navigation UI, and calls {@link RemotePathPickerState.finish}
 * with the user's choice — resolving the original promise. This mirrors the
 * native dialog's request/response shape so callers are unchanged.
 */
import { create } from "zustand";

export type PathPickerMode = "folder" | "files";

export interface PathPickerRequest {
  /** Monotonic id so the modal can detect a fresh request and reset. */
  id: number;
  /** `folder` → single directory; `files` → one-or-more files. */
  mode: PathPickerMode;
  /** Dialog title, forwarded from the original `pickFolder`/`pickFiles` call. */
  title: string;
}

interface RemotePathPickerState {
  /** The active request, or `null` when the picker is closed. */
  request: PathPickerRequest | null;
  /** Open the picker and return the selected absolute paths, or `null` on
   *  cancel. Prefer the {@link openRemotePathPicker} free function. */
  open: (mode: PathPickerMode, title: string) => Promise<string[] | null>;
  /** Resolve the in-flight request with the chosen paths (or `null` to
   *  cancel) and close the modal. Called by the modal component. */
  finish: (paths: string[] | null) => void;
}

let nextRequestId = 1;
let pendingResolve: ((paths: string[] | null) => void) | null = null;

export const useRemotePathPickerStore = create<RemotePathPickerState>(
  (set) => ({
    request: null,
    open: (mode, title) =>
      new Promise<string[] | null>((resolve) => {
        // Guard against overlapping opens: cancel any request still awaiting a
        // resolver before installing the new one so the old promise never
        // dangles.
        if (pendingResolve) {
          pendingResolve(null);
        }
        pendingResolve = resolve;
        set({ request: { id: nextRequestId++, mode, title } });
      }),
    finish: (paths) => {
      const resolve = pendingResolve;
      pendingResolve = null;
      set({ request: null });
      resolve?.(paths);
    },
  }),
);

/** Open the remote path picker and await the user's selection. Returns the
 *  chosen absolute paths, or `null` when the user cancels. */
export function openRemotePathPicker(
  mode: PathPickerMode,
  title: string,
): Promise<string[] | null> {
  return useRemotePathPickerStore.getState().open(mode, title);
}
