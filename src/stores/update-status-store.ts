import { create } from "zustand";

export type UpdateState =
  | "idle"
  | "checking"
  | "update-available"
  | "downloading"
  | "ready"
  | "error";

/**
 * A read-only mirror of the updater's live state, published by the single
 * mounted `useUpdateChecker` instance.
 *
 * The checker is a singleton by convention — `UpdateToast` is the only thing
 * that mounts it, because a second mount would mean a second 4-hour poll and a
 * second `check()` round trip. The app-menu footer wants the same answer
 * ("Up to date" / "Update available" / …) without owning a checker, so the
 * hook pushes its state here and the footer subscribes. One poller, two
 * readers.
 *
 * The actions are the checker's own callbacks, forwarded verbatim, so a click
 * in the menu footer drives exactly the flow the toast's buttons drive —
 * including `isRemote`, because the web client has no updater plugin and its
 * only route to an update is asking the desktop to run one.
 */
interface UpdateStatusStore {
  state: UpdateState;
  updateVersion: string | null;
  downloadProgress: number;
  /** False until a checker has mounted and published at least once. */
  published: boolean;
  /** True on the web (remote) client, where `startDownload` cannot work. */
  isRemote: boolean;
  /**
   * False for package-manager installs (deb/rpm/pacman), where the in-app
   * updater cannot replace the binary and the only route is the release page
   * or the package manager.
   */
  canAutoUpdate: boolean;
  /** `get_package_format`'s answer, or null until it resolves. */
  packageFormat: string | null;
  /** Why the last download or restart failed, while `state` is `error`. */
  errorMessage: string | null;
  startDownload: (() => void) | null;
  installAndRestart: (() => void) | null;
  /** Re-run whichever step failed: the download, or the restart after it. */
  retry: (() => void) | null;
  /** Web only: ask the desktop to run its update + restart flow. */
  requestDesktopUpdate: (() => void) | null;
  publish: (
    snapshot: Pick<
      UpdateStatusStore,
      | "state"
      | "updateVersion"
      | "downloadProgress"
      | "isRemote"
      | "canAutoUpdate"
      | "packageFormat"
      | "errorMessage"
      | "startDownload"
      | "installAndRestart"
      | "retry"
      | "requestDesktopUpdate"
    >,
  ) => void;
}

const INITIAL = {
  state: "idle" as UpdateState,
  updateVersion: null,
  downloadProgress: 0,
  published: false,
  isRemote: false,
  canAutoUpdate: false,
  packageFormat: null,
  errorMessage: null,
  startDownload: null,
  installAndRestart: null,
  retry: null,
  requestDesktopUpdate: null,
};

export const useUpdateStatusStore = create<UpdateStatusStore>((set) => ({
  ...INITIAL,
  publish: (snapshot) => set({ ...snapshot, published: true }),
}));

/** Test-only reset, mirroring the other stores' escape hatch. */
export function __resetUpdateStatusStoreForTests() {
  useUpdateStatusStore.setState({ ...INITIAL });
}
