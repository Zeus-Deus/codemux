import { useCallback, useEffect, useMemo, useState } from "react";

import { ChevronDown, ExternalLink, RotateCw, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  groupSkillsForSettings,
  skillSourceLabel,
  skillSourceLabels,
  type SkillEntry,
} from "@/lib/agent-chat/skill-groups";
import { skillTokenFor } from "@/lib/agent-chat/skill-tokens";
import { toast } from "@/lib/toast";
import { useSkillsStore } from "@/stores/skills-store";
import {
  selectDefaultEditor,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import {
  openInEditor,
  SKILLS_CHANGED_EVENT,
  startSkillsWatcher,
  type Skill,
} from "@/tauri/commands";
import { ensureEditorsDetected } from "@/stores/editor-discovery-store";
import { listen } from "@tauri-apps/api/event";

import { SkillRow, SourceTags } from "./skill-row";
import { SkillViewModal } from "./skill-view-modal";
import { SectionHeader } from "./settings-primitives";
import { cn } from "@/lib/utils";
import { Spinner } from "@/components/ui/spinner";

interface Props {
  /** Active workspace's project root (or null when no project is
   *  selected — e.g. user opened settings from Home). Project-scoped
   *  skill discovery is skipped when this is null; user-wide and
   *  plugin scopes still load. */
  projectRoot: string | null;
}

/**
 * Settings → Skills section. One row per skill name, tagged with every
 * provider that has it; copies whose content differs open into a list of
 * versions under the row. Refresh forces
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
  const adapterErrors = useSkillsStore((s) => s.adapterErrors);
  const includePlugins = useSkillsStore((s) => s.includePlugins);
  const loadSkills = useSkillsStore((s) => s.loadSkills);
  const setIncludePlugins = useSkillsStore((s) => s.setIncludePlugins);
  const disabledIds = useSkillsStore((s) => s.disabledIds);
  const setSkillsDisabled = useSkillsStore((s) => s.setSkillsDisabled);

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

  const entries = useMemo(() => groupSkillsForSettings(skills), [skills]);
  const sources = useMemo(() => skillSourceLabels(skills), [skills]);
  const differing = entries.filter((entry) => entry.versions.length > 1).length;
  const [query, setQuery] = useState("");
  const [sourceFilter, setSourceFilter] = useState("all");
  // A reload can drop the selected source (e.g. plugin skills turned off);
  // fall back to All rather than hiding every row behind a hidden control.
  const activeSource = sources.includes(sourceFilter) ? sourceFilter : "all";
  const shownEntries = useMemo(
    () => filterEntries(entries, query, activeSource),
    [entries, query, activeSource],
  );

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
      console.info("[skills] open file requested", {
        skillId: skill.id,
        filePath: skill.filePath,
        preferredEditorId,
      });
      try {
        const editors = await ensureEditorsDetected();
        console.info("[skills] editor discovery →", editors);
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
      <SectionHeader
        title="Skills"
        description={
          <>
            Skills are reusable instruction sets you can select with{" "}
            <code className="font-mono text-label">/skill-name</code>. Discovered
            from your installed providers (Claude, Codex, OpenCode) and
            Codemux's own skills folder; a skill found in several places shows
            once, tagged with each source. Provider-native automatic discovery
            remains controlled by each provider.
          </>
        }
        action={
          <Button
            variant="outline"
            size="sm"
            onClick={() => loadSkills(projectRoot, true)}
            disabled={loading}
            aria-busy={loading}
            aria-label="Refresh skills"
          >
            <RotateCw
              className={loading ? "mr-1 size-3 animate-spin" : "mr-1 size-3"}
              aria-hidden
            />
            Refresh
          </Button>
        }
      />

      <div className="mb-4 flex items-center justify-between rounded-md border border-border/50 p-3">
        <div className="min-w-0 flex-1">
          <Label htmlFor="include-plugins" className="text-body font-medium">
            Include plugin-bundled skills
          </Label>
          <p className="mt-0.5 text-label text-muted-foreground">
            Plugin skills come from{" "}
            <code className="font-mono text-label">~/.claude/plugins/</code>,
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

      <p className="mb-4 text-label text-muted-foreground">
        Each row controls whether the skill is available in Codemux, for every
        source it was found in. Turning it off does not change provider
        configuration, so its native provider may still discover or invoke it
        automatically.
      </p>

      {error && (
        <p
          data-testid="skills-error"
          className="mb-4 rounded-md bg-destructive/10 px-3 py-2 text-label text-destructive"
        >
          Failed to load skills: {error}
        </p>
      )}

      {adapterErrors.length > 0 && (
        <div className="mb-4 rounded-md border border-border/50 bg-muted/30 px-3 py-2 text-label text-muted-foreground">
          Some provider catalogs could not be refreshed. Readable filesystem
          skills are still available.
          <ul className="mt-1 list-inside list-disc">
            {adapterErrors.map((adapterError) => (
              <li key={`${adapterError.provider}:${adapterError.message}`}>
                {adapterError.provider}: {adapterError.message}
              </li>
            ))}
          </ul>
        </div>
      )}

      {loading && !loaded ? (
        <div
          data-testid="skills-loading"
          className="flex items-center gap-2 py-6 text-body text-muted-foreground"
        >
          <Spinner className="size-4" aria-hidden />
          Loading skills…
        </div>
      ) : skills.length === 0 && !error ? (
        <p
          data-testid="skills-empty"
          className="py-6 text-center text-body text-muted-foreground"
        >
          No skills found. Skills live in{" "}
          <code className="font-mono text-label">~/.claude/skills/</code>,{" "}
          <code className="font-mono text-label">~/.codex/skills/</code>, and the
          per-project equivalents.
        </p>
      ) : (
        <div>
          {differing > 0 && (
            <p
              data-testid="skills-differing-note"
              className="mb-3 rounded-md border border-status-working/30 bg-status-working/5 px-3 py-2 text-label text-muted-foreground"
            >
              <span className="font-medium text-status-working">
                {differing} skill{differing === 1 ? " has" : "s have"} different
                versions
              </span>{" "}
              in different folders. Open a row's versions to compare them.
            </p>
          )}
          <div className="mb-2 flex flex-wrap items-center gap-2">
            <div className="relative min-w-40 flex-1">
              <Search
                className="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground/60"
                aria-hidden
              />
              <Input
                placeholder="Filter skills…"
                aria-label="Filter skills"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                className="pl-9 text-body"
              />
            </div>
            {sources.length > 1 && (
              <ToggleGroup
                type="single"
                size="sm"
                variant="outline"
                spacing={0}
                value={activeSource}
                onValueChange={(next) => setSourceFilter(next || "all")}
                aria-label="Show skills from"
              >
                <ToggleGroupItem value="all">All</ToggleGroupItem>
                {sources.map((source) => (
                  <ToggleGroupItem key={source} value={source}>
                    {source}
                  </ToggleGroupItem>
                ))}
              </ToggleGroup>
            )}
          </div>
          {shownEntries.length === 0 ? (
            <p
              data-testid="skills-no-matches"
              className="py-6 text-center text-body text-muted-foreground"
            >
              No skills match this filter.
            </p>
          ) : (
            <ul className="divide-y divide-border/40 rounded-md border border-border/50">
              {shownEntries.map((entry) => (
                <li key={entry.name}>
                  <SkillEntryRow
                    entry={entry}
                    allSkills={skills}
                    disabledIds={disabledIds}
                    onSetDisabled={setSkillsDisabled}
                    onView={setPendingView}
                    onOpenFile={handleOpenFile}
                  />
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <SkillViewModal
        skill={pendingView}
        onClose={() => setPendingView(null)}
      />
    </div>
  );
}

function filterEntries(
  entries: SkillEntry[],
  query: string,
  source: string,
): SkillEntry[] {
  const needle = query.trim().toLowerCase();
  return entries.filter((entry) => {
    const copies = entry.versions.flatMap((version) => version.copies);
    if (source !== "all" && !copies.some((copy) => skillSourceLabel(copy) === source)) {
      return false;
    }
    if (!needle) return true;
    return copies.some(
      (copy) =>
        copy.name.toLowerCase().includes(needle) ||
        (copy.description ?? "").toLowerCase().includes(needle),
    );
  });
}

/** True while any of `ids` is still enabled. The composer offers a name
 *  as long as one copy of it is enabled, so a row only reads as off once
 *  every copy is; switching it off then disables them all. */
function anyEnabled(ids: string[], disabledIds: string[]): boolean {
  return ids.some((id) => !disabledIds.includes(id));
}

/** Preference ids that the row's switch controls. Invalid skills can't be
 *  enabled, so they're left out. */
function preferenceIds(copies: Skill[]): string[] {
  return [
    ...new Set(
      copies
        .filter((copy) => !copy.validationError)
        .map((copy) => copy.preferenceId ?? copy.id),
    ),
  ];
}

function SkillEntryRow({
  entry,
  allSkills,
  disabledIds,
  onSetDisabled,
  onView,
  onOpenFile,
}: {
  entry: SkillEntry;
  allSkills: Skill[];
  disabledIds: string[];
  onSetDisabled: (ids: string[], disabled: boolean) => void;
  onView: (skill: Skill) => void;
  onOpenFile: (skill: Skill) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const copies = entry.versions.flatMap((version) => version.copies);
  const primary = copies[0];
  const ids = preferenceIds(copies);
  const enabled = anyEnabled(ids, disabledIds);
  const multiple = entry.versions.length > 1;
  const versionsId = `skill-versions-${primary.id}`;

  return (
    <>
      <SkillRow
        skill={primary}
        enabled={enabled}
        onToggleEnabled={() => onSetDisabled(ids, enabled)}
        onView={multiple ? undefined : () => onView(primary)}
        onOpenFile={multiple ? undefined : () => onOpenFile(primary)}
        sources={skillSourceLabels(copies)}
        trailing={
          multiple && (
            <button
              type="button"
              onClick={() => setExpanded((open) => !open)}
              aria-expanded={expanded}
              aria-controls={versionsId}
              className="inline-flex items-center gap-1 rounded-sm border border-status-working/40 bg-status-working/5 px-1.5 py-px text-caption text-status-working hover:bg-status-working/10"
            >
              {entry.versions.length} versions
              <ChevronDown
                className={cn(
                  "size-3 transition-transform duration-150",
                  expanded && "rotate-180",
                )}
                aria-hidden
              />
            </button>
          )
        }
      />
      {multiple && expanded && (
        <ul
          id={versionsId}
          data-testid={`skill-versions-${entry.name}`}
          className="divide-y divide-border/30 border-t border-border/40 bg-surface-1/50 pl-9"
        >
          {entry.versions.map((version) => (
            <li key={version.copies[0].id}>
              <SkillVersionRow
                copies={version.copies}
                allSkills={allSkills}
                disabledIds={disabledIds}
                onSetDisabled={onSetDisabled}
                onView={onView}
                onOpenFile={onOpenFile}
              />
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

function SkillVersionRow({
  copies,
  allSkills,
  disabledIds,
  onSetDisabled,
  onView,
  onOpenFile,
}: {
  copies: Skill[];
  allSkills: Skill[];
  disabledIds: string[];
  onSetDisabled: (ids: string[], disabled: boolean) => void;
  onView: (skill: Skill) => void;
  onOpenFile: (skill: Skill) => void;
}) {
  const skill = copies[0];
  const ids = preferenceIds(copies);
  const enabled = anyEnabled(ids, disabledIds);
  const canToggle = ids.length > 0;
  return (
    <div className="group flex items-center gap-3 py-1.5 pr-3">
      <div className="min-w-0 flex-1">
        <p className="truncate font-mono text-label text-foreground">
          {skillTokenFor(skill, allSkills)}
        </p>
        <p
          className="truncate text-caption text-muted-foreground"
          title={skill.filePath}
        >
          {skill.filePath}
        </p>
      </div>
      <SourceTags sources={skillSourceLabels(copies)} />
      {skill.readable !== false && (
        <div className="flex items-center gap-1 opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-within:opacity-100 pointer-coarse:opacity-100">
          <Button variant="ghost" size="sm" onClick={() => onView(skill)}>
            View
          </Button>
          {Boolean(skill.skillDir) && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => onOpenFile(skill)}
              aria-label={`Open ${skillTokenFor(skill, allSkills)} in editor`}
            >
              <ExternalLink className="size-3" aria-hidden />
            </Button>
          )}
        </div>
      )}
      <Switch
        checked={canToggle && enabled}
        onCheckedChange={() => onSetDisabled(ids, enabled)}
        disabled={!canToggle}
        aria-label={
          enabled
            ? `Remove ${skillTokenFor(skill, allSkills)} from Codemux`
            : `Make ${skillTokenFor(skill, allSkills)} available in Codemux`
        }
      />
    </div>
  );
}
