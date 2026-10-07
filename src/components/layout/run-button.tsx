import { useState, useEffect, useRef, type FormEvent } from "react";
import { Check, ChevronDown, Play, Settings } from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  Popover,
  PopoverAnchor,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { Eyebrow } from "@/components/ui/eyebrow";
import { Input } from "@/components/ui/input";
import {
  BAND_CONTROL_HOVER,
  BAND_CONTROL_RADIUS,
} from "@/components/layout/titlebar-control-style";
import { useActiveWorkspaceProjectRoot } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { cn } from "@/lib/utils";
import {
  detectRunCandidates,
  getProjectScripts,
  getWorkspaceConfig,
  runProjectDevCommand,
  setProjectScripts,
} from "@/tauri/commands";
import type { RunCandidate } from "@/tauri/types";

interface RunButtonProps {
  workspaceId: string;
  /** Rendering shape. `"legacy"` (default) is the original ghost
   *  [Run/Set Run + badge] button + standalone gear pair — used by the
   *  legacy (flag-off) PresetBar, which must stay byte-identical. `"split"` is
   *  the GUI-chrome split button: main segment runs (green play glyph), caret
   *  segment configures, no standalone gear. Only the GUI branch of
   *  `title-bar.tsx` passes `"split"`. */
  variant?: "legacy" | "split";
}

export function RunButton({ workspaceId, variant = "legacy" }: RunButtonProps) {
  const [runCommand, setRunCommand] = useState<string | null>(null);
  // A `.codemux/config.json` overrides the DB scripts the popover writes.
  const [hasConfigFile, setHasConfigFile] = useState(false);
  const [popoverOpen, setPopoverOpen] = useState(false);
  // Whether the popover was last used with the pointer. Focus then stays put
  // on close instead of returning to the caret, where it would pop the
  // caret's tooltip right after a pick. Keyboard users still get focus back.
  const usedPointerRef = useRef(false);
  // Subscribe to the primitive project_root, not the whole workspace
  // object — full-snapshot rebuilds on every backend tick churn the
  // workspace ref and would re-render this button on every tick.
  const projectRoot = useActiveWorkspaceProjectRoot();
  const showSettings = useUIStore((s) => s.showSettings);

  // Fetch on mount, on project switch, and when settings closes (the user
  // may have edited the run command there).
  useEffect(() => {
    if (showSettings) return;

    if (!projectRoot) {
      setRunCommand(null);
      setHasConfigFile(false);
      return;
    }

    let cancelled = false;
    Promise.all([
      getWorkspaceConfig(projectRoot).catch(() => null),
      getProjectScripts(projectRoot).catch(() => null),
    ]).then(([fileConfig, dbScripts]) => {
      if (cancelled) return;
      // A config file replaces the DB scripts wholesale, as in
      // `read_effective_config` on the backend.
      const cmd = fileConfig ? fileConfig.run : (dbScripts?.run ?? null);
      setRunCommand(cmd && cmd.trim() ? cmd.trim() : null);
      setHasConfigFile(fileConfig !== null);
    });

    return () => { cancelled = true; };
  }, [projectRoot, showSettings]);

  const setShowSettings = useUIStore.getState().setShowSettings;

  const handleRun = () => {
    runProjectDevCommand(workspaceId).catch(console.error);
  };

  const handleConfigure = () => {
    setShowSettings(true, "projects");
  };

  const shortcutBadge = (
    <kbd className="ml-1 text-caption leading-none bg-muted px-1 py-0.5 rounded-sm border border-border text-muted-foreground font-sans">
      Ctrl+Shift+G
    </kbd>
  );

  const isConfigured = !!runCommand;

  if (variant === "split") {
    // GUI-chrome split button: the main segment runs, or opens the run
    // command popover while nothing is set; the caret always opens it.
    // The popover offers detected scripts, a typed command and a link to
    // Settings, so setting up Run never leaves the workspace.
    //
    // The two segments carry no border, no resting fill and no divider —
    // only a hover fill, exactly like the panel toggle at the other end of
    // the band. A chip outline here would make Run the one boxed control
    // in a row of borderless ones; without it the green play glyph and the
    // semibold label are what mark it as the primary action. The inline
    // keyboard-shortcut badge is gone — the shortcut lives in the main
    // segment's tooltip instead of eating horizontal space.
    const mainTooltip = isConfigured
      ? `${runCommand} · Ctrl+Shift+G`
      : "Set Run · Ctrl+Shift+G";
    return (
      <Popover open={popoverOpen} onOpenChange={setPopoverOpen}>
        <PopoverAnchor asChild>
          <div className="flex h-7 shrink-0 items-center gap-[2px]">
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  onClick={isConfigured ? handleRun : () => setPopoverOpen(true)}
                  className={cn(
                    "flex h-7 items-center gap-1.5 px-2 text-label font-semibold text-foreground",
                    BAND_CONTROL_RADIUS,
                    BAND_CONTROL_HOVER,
                    !isConfigured && "text-muted-foreground",
                  )}
                >
                  <Play className="h-[11px] w-[11px] shrink-0 text-status-open" fill="currentColor" />
                  <span>{isConfigured ? "Run" : "Set Run"}</span>
                </button>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={4}>
                {mainTooltip}
              </TooltipContent>
            </Tooltip>

            <Tooltip>
              <PopoverTrigger asChild>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    aria-label="Configure run command"
                    className={cn(
                      "flex h-7 w-5 items-center justify-center text-muted-foreground",
                      BAND_CONTROL_RADIUS,
                      BAND_CONTROL_HOVER,
                      "data-[state=open]:bg-muted data-[state=open]:text-foreground dark:data-[state=open]:bg-muted/50",
                    )}
                  >
                    <ChevronDown className="size-3" />
                  </button>
                </TooltipTrigger>
              </PopoverTrigger>
              <TooltipContent side="bottom" sideOffset={4}>
                Configure run command
              </TooltipContent>
            </Tooltip>
          </div>
        </PopoverAnchor>
        <PopoverContent
          align="end"
          sideOffset={6}
          className="w-80 max-w-[calc(100vw-24px)] p-0"
          onPointerDownCapture={() => {
            usedPointerRef.current = true;
          }}
          onKeyDownCapture={() => {
            usedPointerRef.current = false;
          }}
          onCloseAutoFocus={(e) => {
            if (usedPointerRef.current) e.preventDefault();
            usedPointerRef.current = false;
          }}
        >
          {projectRoot && (
            <RunCommandForm
              projectRoot={projectRoot}
              workspaceId={workspaceId}
              currentCommand={runCommand}
              hasConfigFile={hasConfigFile}
              onSaved={(cmd) => {
                setRunCommand(cmd);
                setPopoverOpen(false);
              }}
            />
          )}
          <div className={cn("p-1.5", projectRoot && "border-t")}>
            <button
              type="button"
              onClick={() => {
                setPopoverOpen(false);
                handleConfigure();
              }}
              className={POPOVER_ROW}
            >
              <Settings className="size-3.5 shrink-0 text-muted-foreground" />
              <span>More settings…</span>
            </button>
          </div>
        </PopoverContent>
      </Popover>
    );
  }

  return (
    <div className="flex items-center shrink-0 gap-0.5">
      {/* Run button — primary action */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="xs"
            className={`gap-1 ${!isConfigured ? "text-muted-foreground" : ""}`}
            onClick={isConfigured ? handleRun : handleConfigure}
          >
            <Play className="size-3.5" />
            <span>{isConfigured ? "Run" : "Set Run"}</span>
            {shortcutBadge}
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          {isConfigured ? runCommand : "Configure run command"}
        </TooltipContent>
      </Tooltip>

      {/* Gear button — opens settings, always available */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon-xs" onClick={handleConfigure}>
            <Settings className="size-4" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          {isConfigured ? "Edit run command" : "Configure run command"}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}

/** One 32px clickable row inside the run command popover. */
const POPOVER_ROW =
  "flex h-8 w-full min-w-0 items-center gap-2.5 rounded-md px-2 text-left text-body-sm text-foreground transition-colors duration-100 hover:bg-surface-2 focus-visible:bg-surface-2 disabled:pointer-events-none disabled:opacity-50";

interface RunCommandFormProps {
  projectRoot: string;
  workspaceId: string;
  currentCommand: string | null;
  hasConfigFile: boolean;
  onSaved: (command: string) => void;
}

/** Detected run commands plus a free-text field. Picking or submitting one
 *  saves it as the project's run command and starts it. */
function RunCommandForm({
  projectRoot,
  workspaceId,
  currentCommand,
  hasConfigFile,
  onSaved,
}: RunCommandFormProps) {
  const [candidates, setCandidates] = useState<RunCandidate[]>([]);
  const [draft, setDraft] = useState(currentCommand ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Focus the field with the caret after the current command. This runs
  // before the popover's own auto-focus, which then leaves focus alone, so
  // the field is focused even before detection returns and its text is not
  // selected.
  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }, []);

  useEffect(() => {
    if (hasConfigFile) return;
    let cancelled = false;
    detectRunCandidates(projectRoot)
      .then((found) => {
        if (!cancelled) setCandidates(found);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [projectRoot, hasConfigFile]);

  // Saving to the DB would be ignored while a config file exists, so point
  // at the file instead of offering choices that would not take effect.
  if (hasConfigFile) {
    return (
      <p className="px-3 py-2.5 text-body-sm leading-relaxed text-muted-foreground">
        {currentCommand ? (
          <>
            This project sets its run command in{" "}
            <code className="font-mono text-label">.codemux/config.json</code>.
          </>
        ) : (
          <>
            This project uses{" "}
            <code className="font-mono text-label">.codemux/config.json</code>.
            Add a <code className="font-mono text-label">run</code> command
            there.
          </>
        )}
      </p>
    );
  }

  const saveAndRun = async (command: string) => {
    const trimmed = command.trim();
    if (!trimmed || saving) return;
    setSaving(true);
    setError(null);
    try {
      // Keep the project's other scripts; only the run command changes. A
      // failed read aborts the save rather than writing empty scripts.
      const existing = await getProjectScripts(projectRoot);
      await setProjectScripts(projectRoot, {
        setup: existing?.setup ?? [],
        teardown: existing?.teardown ?? [],
        worktree_includes: existing?.worktree_includes ?? [],
        run: trimmed,
      });
    } catch (e) {
      setError(String(e));
      setSaving(false);
      return;
    }
    onSaved(trimmed);
    runProjectDevCommand(workspaceId).catch(console.error);
  };

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    void saveAndRun(draft);
  };

  return (
    <>
      {candidates.length > 0 && (
        <div className="flex flex-col p-1.5">
          <Eyebrow className="px-2 pb-1 pt-1">Detected</Eyebrow>
          {candidates.map((c) => {
            const isCurrent = c.command === currentCommand;
            return (
              <button
                key={c.command}
                type="button"
                disabled={saving}
                // The saved command gets a check instead of the play glyph.
                aria-current={isCurrent ? "true" : undefined}
                onClick={() => void saveAndRun(c.command)}
                className={POPOVER_ROW}
              >
                {isCurrent ? (
                  <Check className="size-3 shrink-0 text-foreground" />
                ) : (
                  <Play className="size-3 shrink-0 text-muted-foreground" />
                )}
                <span className="min-w-0 flex-1 truncate font-mono text-label">
                  {c.command}
                </span>
                <span className="shrink-0 font-mono text-caption text-muted-foreground/60">
                  {c.source}
                </span>
              </button>
            );
          })}
        </div>
      )}
      <form
        onSubmit={handleSubmit}
        className={cn("flex flex-col gap-2 p-3", candidates.length > 0 && "border-t")}
      >
        <Eyebrow asChild>
          <label htmlFor="run-command-input">Run command</label>
        </Eyebrow>
        <div className="flex gap-1.5">
          <Input
            id="run-command-input"
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="e.g. npm run dev"
            spellCheck={false}
            autoComplete="off"
            className="font-mono"
          />
          <Button type="submit" disabled={!draft.trim() || saving}>
            Run
          </Button>
        </div>
        {error && <p className="text-label text-destructive">{error}</p>}
      </form>
    </>
  );
}
