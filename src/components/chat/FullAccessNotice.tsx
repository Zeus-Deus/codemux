import { create } from "zustand";
import { persist } from "zustand/middleware";
import { ShieldAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { PermissionModeOption } from "@/tauri/types";
import { CHAT_COLUMN } from "./chat-column";

/**
 * Provider-native values for "run every tool without asking" (Claude,
 * Codex). See `FALLBACK_DEFAULT_PERMISSION_MODE_BY_PROVIDER` in
 * `capability-defaults.ts`, which makes these the default for new chats.
 */
const FULL_ACCESS_MODES = new Set(["bypassPermissions", "danger-full-access"]);

export function isFullAccessMode(mode: string | null | undefined): boolean {
  return mode != null && FULL_ACCESS_MODES.has(mode);
}

interface FullAccessNoticeStore {
  dismissed: boolean;
  dismiss: () => void;
}

/** Per machine: the explanation is shown until it is acknowledged once. */
export const useFullAccessNoticeStore = create<FullAccessNoticeStore>()(
  persist(
    (set) => ({
      dismissed: false,
      dismiss: () => set({ dismissed: true }),
    }),
    { name: "codemux-full-access-notice" },
  ),
);

/**
 * One-time explanation above a new chat's composer: agents start with
 * Full access, so they run commands and edit files without asking. Shown
 * only while the draft is actually in that mode.
 *
 * Provider capabilities load on first picker intent, so on a fresh install
 * `permissionModes` can still be unknown (null) here. The explanation still
 * shows then; only the pointer to the access menu waits until that menu is
 * actually in the composer footer.
 */
export function FullAccessNotice({
  permissionMode,
  permissionModes,
}: {
  permissionMode: string | null;
  permissionModes: PermissionModeOption[] | null;
}) {
  const dismissed = useFullAccessNoticeStore((s) => s.dismissed);
  const dismiss = useFullAccessNoticeStore((s) => s.dismiss);
  // An empty list means the provider has no permission concept at all.
  if (dismissed || permissionModes?.length === 0) return null;
  if (!isFullAccessMode(permissionMode)) return null;
  const hasPicker = !!permissionModes?.length;
  const label =
    permissionModes?.find((m) => m.value === permissionMode)?.label ??
    "Full access";

  return (
    <div className={CHAT_COLUMN}>
      <div
        role="note"
        className="flex items-start gap-2 rounded-md border border-hairline bg-surface-1 px-3 py-2 text-body-sm text-muted-foreground"
      >
        <ShieldAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden />
        <p className="min-w-0 flex-1 leading-relaxed">
          <span className="text-foreground">
            New chats start with {label}.
          </span>{" "}
          Agents run commands and edit files without asking first.
          {hasPicker &&
            ` Change it for this chat from the ${label} menu in the message box.`}
        </p>
        <Button
          variant="ghost"
          size="xs"
          className="shrink-0 text-muted-foreground"
          onClick={dismiss}
        >
          Got it
        </Button>
      </div>
    </div>
  );
}
