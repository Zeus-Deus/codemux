import { useCallback, useEffect, useMemo, useState } from "react";

import { Loader2, RotateCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  detectConflicts,
  groupHeadingFor,
  groupSkillsByScope,
} from "@/lib/agent-chat/skill-groups";
import { toast } from "@/lib/toast";
import { useSkillsStore } from "@/stores/skills-store";
import {
  selectDefaultEditor,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import {
  detectEditors,
  openInEditor,
  SKILLS_CHANGED_EVENT,
  startSkillsWatcher,
  type Skill,
} from "@/tauri/commands";
import { listen } from "@tauri-apps/api/event";

import { SkillRow } from "./skill-row";
import { SkillViewModal } from "./skill-view-modal";

interface Props {
  /** Active workspace's project root (or null when no project is
   *  selected — e.g. user opened settings from Home). Project-scoped
   *  skill discovery is skipped when this is null; user-wide and
   *  plugin scopes still load. */
  projectRoot: string | null;
}

/**
 * Settings → Skills section. Mirrors `permissions-section.tsx` in
 * shape: header + grouped rows + hover-reveal actions. Refresh forces
 * the skills store to bypass its 60s TTL and re-walk the disk; the
 * plugin toggle invalidates the cache so the next load reflects it.
 *
 * View opens a modal with the full SKILL.md body via ChatMarkdown;
 * Open file invokes the user's first detected editor against the skill
 * file. Skill creation lives in v2 (the user can `/skill-creator` from
 * chat in the meantime).
 */
export function SkillsSection({ projectRoot }: Props) {
  const skills = useSkillsStore((s) => s.skills);
  const loaded = useSkillsStore((s) => s.loaded);
  const loading = useSkillsStore((s) => s.loading);
  const error = useSkillsStore((s) => s.error);
  const includePlugins = useSkillsStore((s) => s.includePlugins);
  const loadSkills = useSkillsStore((s) => s.loadSkills);
  const setIncludePlugins = useSkillsStore((s) => s.setIncludePlugins);
  const disabledIds = useSkillsStore((s) => s.disabledIds);
  const toggleSkillDisabled = useSkillsStore((s) => s.toggleSkillDisabled);

  // Settings is the authoritative refresh path: every section mount
  // forces a fresh disk walk so users always see the current state of
  // ~/.claude/skills/ etc, even if the popup hydrated from the TTL
  // cache earlier in the session.
  useEffect(() => {
    void loadSkills(projectRoot, true);
    // `includePlugins` change triggers re-load via cache-invalidation
    // inside `setIncludePlugins`; including it as a dep ensures the
    // re-load fires at the right moment.
  }, [projectRoot, includePlugins, loadSkills]);

  // File watcher: on Settings mount, start watching every enumerated
  // skill path and listen for Tauri events. When a file changes the
  // backend emits `skills-changed`; we force-refresh so the user sees
  // the new state without clicking Refresh. Unmount tears down the
  // event listener; the Rust watcher itself stays alive (keeping it
  // running across Settings open/close avoids re-walking inotify on
  // every visit, and the cost is one inotify fd per watched dir).
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void startSkillsWatcher(projectRoot, includePlugins).catch((err) => {
      console.warn("[skills] watcher failed to start:", err);
    });

    void listen(SKILLS_CHANGED_EVENT, () => {
      if (cancelled) return;
      void loadSkills(projectRoot, true);
    }).then((dispose) => {
      if (cancelled) {
        dispose();
        return;
      }
      unlisten = dispose;
    });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [projectRoot, includePlugins, loadSkills]);

  const groups = useMemo(() => groupSkillsByScope(skills), [skills]);
  const conflicts = useMemo(() => detectConflicts(skills), [skills]);

  const [pendingView, setPendingView] = useState<Skill | null>(null);

  // Honor the user's configured default editor (Settings → Editor →
  // Default IDE). The title-bar "Open in <editor>" button writes the
  // same setting; falls back to the first detected editor when the
  // preference is unset (matches the title-bar's behavior on first run).
  const preferredEditorId = useSyncedSettingsStore(selectDefaultEditor);

  const handleOpenFile = useCallback(
    async (skill: Skill) => {
      // Diagnostic logging — visible in DevTools console to help
      // diagnose silent no-op cases (editor not on PATH, sandbox
      // stripping DISPLAY, etc.). Cheap and safe in production.
      console.info(
        "[skills] open file requested",
        { skillId: skill.id, filePath: skill.filePath, preferredEditorId },
      );
      try {
        const editors = await detectEditors();
        console.info("[skills] detectEditors →", editors);
        if (editors.length === 0) {
          toast.error("No editor detected", {
            description:
              "Install VS Code, Cursor, or another supported editor on your PATH.",
          });
          return;
        }
        // Prefer the user's configured default; if it's unset or the
        // editor was uninstalled since they picked it, fall back to the
        // first detected editor so the button never silently no-ops.
        const editorId =
          (preferredEditorId &&
            editors.find((e) => e.id === preferredEditorId)?.id) ||
          editors[0].id;
        console.info("[skills] opening in editor", { editorId });
        await openInEditor(editorId, skill.filePath);
        console.info("[skills] openInEditor resolved");
      } catch (err) {
        console.error("[skills] open file failed", err);
        toast.error("Failed to open file", { description: String(err) });
      }
    },
    [preferredEditorId],
  );

  return (
    <div>
      <div className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h2 className="text-base font-semibold tracking-tight">Skills</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Skills are reusable instruction sets that get injected into your
            messages when you mention them with{" "}
            <code className="font-mono text-xs">/skill-name</code>. Discovered
            from your installed providers (Claude, Codex, OpenCode) and
            Codemux's own skills folder.
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => loadSkills(projectRoot, true)}
          disabled={loading}
          aria-label="Refresh skills"
        >
          <RotateCw
            className={loading ? "mr-1 h-3 w-3 animate-spin" : "mr-1 h-3 w-3"}
            aria-hidden
          />
          Refresh
        </Button>
      </div>

      <div className="mb-4 flex items-center justify-between rounded-md border border-border/50 p-3">
        <div className="min-w-0 flex-1">
          <Label htmlFor="include-plugins" className="text-sm font-medium">
            Include plugin-bundled skills
          </Label>
          <p className="mt-0.5 text-xs text-muted-foreground">
            Plugin skills come from{" "}
            <code className="font-mono text-[11px]">~/.claude/plugins/</code>,
            including marketplace and external installs.
          </p>
        </div>
        <Switch
          id="include-plugins"
          checked={includePlugins}
          onCheckedChange={setIncludePlugins}
          data-testid="include-plugins-switch"
        />
      </div>

      {error && (
        <p
          data-testid="skills-error"
          className="mb-4 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          Failed to load skills: {error}
        </p>
      )}

      {loading && !loaded ? (
        <div
          data-testid="skills-loading"
          className="flex items-center gap-2 py-6 text-sm text-muted-foreground"
        >
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          Loading skills…
        </div>
      ) : skills.length === 0 && !error ? (
        <p
          data-testid="skills-empty"
          className="py-6 text-center text-sm text-muted-foreground"
        >
          No skills found. Skills live in{" "}
          <code className="font-mono text-xs">~/.claude/skills/</code>,{" "}
          <code className="font-mono text-xs">~/.codex/skills/</code>, and the
          per-project equivalents.
        </p>
      ) : (
        <div className="space-y-6">
          {conflicts.size > 0 && (
            <ConflictsSection
              conflicts={conflicts}
              onView={setPendingView}
              onOpenFile={handleOpenFile}
            />
          )}
          {groups.map((group) => (
            <SkillsGroupSection
              key={group.heading}
              heading={group.heading}
              skills={group.skills}
              disabledIds={disabledIds}
              onToggleDisabled={toggleSkillDisabled}
              onView={setPendingView}
              onOpenFile={handleOpenFile}
            />
          ))}
        </div>
      )}

      <SkillViewModal
        skill={pendingView}
        onClose={() => setPendingView(null)}
      />
    </div>
  );
}

function ConflictsSection({
  conflicts,
  onView,
  onOpenFile,
}: {
  conflicts: Map<string, Skill[]>;
  onView: (skill: Skill) => void;
  onOpenFile: (skill: Skill) => void;
}) {
  // Render in alphabetical name order so the same conflict surfaces in
  // the same place across refreshes, even as the underlying skill list
  // changes order.
  const entries = [...conflicts.entries()].sort(([a], [b]) =>
    a.toLowerCase().localeCompare(b.toLowerCase()),
  );
  return (
    <section data-testid="skills-conflicts">
      <header className="mb-2 flex items-baseline justify-between">
        <h3 className="text-xs font-semibold uppercase tracking-wider text-status-working dark:text-status-working">
          Naming conflicts
        </h3>
        <span className="text-[10px] text-muted-foreground/70">
          {entries.length} name{entries.length === 1 ? "" : "s"} clashing
        </span>
      </header>
      <p className="mb-3 text-xs text-muted-foreground">
        These skill names appear in more than one source. Both stay
        available in the slash popup with a scope suffix; pick the one
        you want explicitly.
      </p>
      <div className="space-y-3">
        {entries.map(([name, skills]) => (
          <div
            key={name}
            className="rounded-md border border-status-working/30 bg-status-working/5 p-2"
            data-testid={`conflict-group-${name}`}
          >
            <div className="mb-1.5 px-1 text-xs font-mono text-foreground">
              {name}{" "}
              <span className="text-muted-foreground">
                ({skills.length} sources)
              </span>
            </div>
            <ul className="divide-y divide-status-working/20">
              {skills.map((skill) => (
                <li key={skill.id}>
                  <ConflictRow
                    skill={skill}
                    onView={() => onView(skill)}
                    onOpenFile={() => onOpenFile(skill)}
                  />
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </section>
  );
}

function ConflictRow({
  skill,
  onView,
  onOpenFile,
}: {
  skill: Skill;
  onView: () => void;
  onOpenFile: () => void;
}) {
  // Slim variant of SkillRow that always shows the scope heading
  // inline (no hover required) since that's the whole point here.
  return (
    <div className="group flex items-center gap-3 px-2 py-1.5">
      <span className="flex-1 truncate text-xs">
        <span className="text-muted-foreground">{groupHeadingFor(skill)}</span>
        {skill.description && (
          <span className="ml-2 text-muted-foreground/70">
            {skill.description}
          </span>
        )}
      </span>
      <div className="flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
        <button
          type="button"
          onClick={onView}
          className="rounded px-2 py-0.5 text-xs hover:bg-foreground/10"
        >
          View
        </button>
        <button
          type="button"
          onClick={onOpenFile}
          aria-label={`Open ${skill.name} in editor`}
          className="rounded px-1 py-0.5 hover:bg-foreground/10"
        >
          ↗
        </button>
      </div>
    </div>
  );
}

function SkillsGroupSection({
  heading,
  skills,
  disabledIds,
  onToggleDisabled,
  onView,
  onOpenFile,
}: {
  heading: string;
  skills: Skill[];
  disabledIds: string[];
  onToggleDisabled: (id: string) => void;
  onView: (skill: Skill) => void;
  onOpenFile: (skill: Skill) => void;
}) {
  return (
    <section>
      <header className="mb-2 flex items-baseline justify-between">
        <h3 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          {heading}
        </h3>
        <span className="text-[10px] text-muted-foreground/70">
          {skills.length} skill{skills.length === 1 ? "" : "s"}
        </span>
      </header>
      <ul className="divide-y divide-border/40 rounded-md border border-border/50">
        {skills.map((skill) => (
          <li key={skill.id}>
            <SkillRow
              skill={skill}
              enabled={!disabledIds.includes(skill.id)}
              onToggleEnabled={() => onToggleDisabled(skill.id)}
              onView={() => onView(skill)}
              onOpenFile={() => onOpenFile(skill)}
            />
          </li>
        ))}
      </ul>
    </section>
  );
}
