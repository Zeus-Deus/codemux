import { isRemoteClient } from "@/components/remote/is-remote-client";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { SettingRow } from "@/components/settings/settings-primitives";
import { reportFailure } from "@/lib/toast";
import { SETTINGS_DEFAULTS, useSettingsStore } from "@/stores/settings-store";
import { worktreeCleanupSetAuto } from "@/tauri/commands";

import { openWorktreeCleanup } from "./worktree-cleanup-store";

const AUTO_KEY = "worktree_cleanup.auto";

/** Settings → Archive: the automatic-cleanup switch and the manual review.
 *  Desktop only: a paired browser can't reach the cleanup commands. */
export function WorktreeCleanupSettings() {
  return isRemoteClient() ? null : <WorktreeCleanupRows />;
}

function WorktreeCleanupRows() {
  const auto = useSettingsStore(
    (s) => (s.settings[AUTO_KEY] ?? SETTINGS_DEFAULTS[AUTO_KEY]) === "true",
  );

  const setAuto = (enabled: boolean) => {
    const previous = useSettingsStore.getState().settings[AUTO_KEY];
    const apply = (value: string | undefined) =>
      useSettingsStore.setState((s) => {
        const settings = { ...s.settings };
        if (value === undefined) delete settings[AUTO_KEY];
        else settings[AUTO_KEY] = value;
        return { settings };
      });
    // The backend command persists the value, so the store is only updated
    // locally here, and rolled back if the write fails.
    apply(String(enabled));
    worktreeCleanupSetAuto(enabled).catch((err: unknown) => {
      apply(previous);
      reportFailure("Couldn't change worktree cleanup")(err);
    });
  };

  return (
    <section className="divide-y divide-border/50">
      <SettingRow
        label="Clean up stale worktrees automatically"
        description="Every few hours, remove worktrees whose branch or pull request merged, or whose workspace was archived or closed over 30 days ago. Open workspaces, and worktrees holding uncommitted changes, local-only files or unpushed commits, are never touched. Branches are always kept."
        scope="device"
      >
        <Switch checked={auto} onCheckedChange={setAuto} />
      </SettingRow>
      <SettingRow
        label="Stale worktrees"
        description="See what would be removed, and why, before anything is."
      >
        <Button variant="secondary" size="sm" onClick={openWorktreeCleanup}>
          Review…
        </Button>
      </SettingRow>
    </section>
  );
}
