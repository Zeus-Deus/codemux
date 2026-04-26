import { Fragment, useEffect, useMemo, useRef, useState } from "react";

import { cn } from "@/lib/utils";
import { buildSkillCommands } from "@/lib/agent-chat/skill-commands";
import { segmentForHighlight } from "@/lib/agent-chat/skill-tokens";
import {
  buildModeCommands,
  filterSlashItems,
  findSlashAtCursor,
  nextModeInCycle,
  type SlashAnchor,
  type SlashCommandItem,
} from "@/lib/agent-chat/slash-commands";
import type { ChatMode } from "@/stores/agent-chat-store";
import {
  selectActiveSkills,
  useSkillsStore,
} from "@/stores/skills-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  PermissionModeOption,
} from "@/tauri/types";

import { ComposerFooter } from "./ComposerFooter";
import type { ActivePillMode } from "./pickers/ModePill";
import { SlashCommandPopup } from "./SlashCommandPopup";

interface Props {
  draft: string;
  cwd: string | null;
  provider: AgentChatProviderKind;
  model: string | null;
  permissionMode: string | null;
  effort: string | null;
  contextWindow: string | null;
  activeModel: ChatModelInfo | null;
  effortLabelMap: Record<string, string>;
  permissionModes: PermissionModeOption[] | null;
  ultrathinkInBodyText: boolean;
  streaming: boolean;
  sessionReady: boolean;
  showProviderPicker: boolean;
  /** Composer-level Cursor-style mode pill. Swaps the placeholder,
   *  hides the permission picker, and toggles the mode selector
   *  (dropdown → pill). */
  mode: ChatMode;
  /** When set, renders a muted inline banner above the textarea. Used
   *  for draft-send retry affordance (§8). `null` hides the banner. */
  errorMessage?: string | null;
  /** When false, hides the Stop button even while streaming. The draft
   *  surface uses this during materialise because there is no live
   *  session to interrupt. Defaults to true. */
  showStopButton?: boolean;
  /** Optional replacement for the Zone 1 strip (the cwd / "Home"
   *  label above the textarea). The draft surface uses this to thread
   *  a project picker in when the draft target is Home. Pass `null`
   *  (or omit) to keep the default cwd label. */
  zone1Override?: React.ReactNode;
  onDraftChange: (draft: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  onProviderChange: (provider: AgentChatProviderKind) => void;
  onModelChange: (model: string) => void;
  onPermissionModeChange: (mode: string) => void;
  onEffortChange: (effort: string) => void;
  onContextWindowChange: (value: string) => void;
  onModeActivate: (mode: ActivePillMode) => void;
  onModeRemove: () => void;
}

const MAX_ROWS_APPROX_PX = 32 + 7 * 20; // ~8 rows

export function Composer({
  draft,
  cwd,
  provider,
  model,
  permissionMode,
  effort,
  contextWindow,
  activeModel,
  effortLabelMap,
  permissionModes,
  ultrathinkInBodyText,
  streaming,
  sessionReady,
  showProviderPicker,
  mode,
  errorMessage = null,
  showStopButton = true,
  zone1Override = null,
  onDraftChange,
  onSubmit,
  onStop,
  onProviderChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onModeActivate,
  onModeRemove,
}: Props) {
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Auto-grow textarea up to ~8 rows.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const desired = Math.min(el.scrollHeight, MAX_ROWS_APPROX_PX);
    el.style.height = `${desired}px`;
  }, [draft]);

  // ─── Slash command popup state ────────────────────────────────────
  // The popup is purely a discoverability surface: when the user types
  // `/` the popup opens; arrow keys move the highlight; Enter activates;
  // Esc closes leaving the typed `/` intact so the user can still type
  // a literal slash.
  const [slashAnchor, setSlashAnchor] = useState<SlashAnchor | null>(null);
  const [slashHighlighted, setSlashHighlighted] = useState<string | null>(null);
  // IME composition guard — slash detection must not fire mid-composition,
  // otherwise dead-key sequences for non-Latin keyboards trigger the
  // popup unexpectedly.
  const composingRef = useRef(false);

  const modeCommands = useMemo(
    () => buildModeCommands({ activeMode: mode, onActivate: onModeActivate }),
    [mode, onModeActivate],
  );

  // ─── Skills (Cursor-style inline tokens) ─────────────────────────
  // Lazy-load on first popup open. Picking a skill expands the typed
  // `/<query>` to the full `/<skill-name>` in the textarea — the slash
  // command stays as literal text, syntax-highlighted by the mirror
  // overlay below. At send time the parent parses the text against the
  // skills registry and injects matched skill bodies as a per-turn
  // prefix (no separate chip / staging state needed).
  // `selectActiveSkills` already filters out disabled ids — Composer
  // never sees disabled skills, so highlight + picker + send-time
  // injection all stay consistent.
  const skills = useSkillsStore(selectActiveSkills);
  const loadSkills = useSkillsStore((s) => s.loadSkills);
  const skillsLoading = useSkillsStore((s) => s.loading);
  const skillsLoaded = useSkillsStore((s) => s.loaded);
  const skillsError = useSkillsStore((s) => s.error);

  // The popup-side `onInvoke` handler is a no-op signal: the actual
  // textarea mutation happens inside `handleSlashSelect` based on the
  // item's id prefix. Modes still need their `onSelect` activator.
  const skillItems = useMemo(
    () => buildSkillCommands({ skills, onInvoke: () => {} }),
    [skills],
  );

  // Highlight segments for the mirror overlay. Recomputed on every
  // keystroke; cheap (one regex pass + map) for realistic draft sizes.
  const highlightSegments = useMemo(
    () => segmentForHighlight(draft, skills),
    [draft, skills],
  );

  const allSlashItems = useMemo(
    () => [...modeCommands, ...skillItems],
    [modeCommands, skillItems],
  );

  // Surface skill-loading + skill-error in the popup footer so the user
  // never sees a silent empty `SKILLS` group on first open or a quiet
  // failure when the backend scan errors. Loading shows only while the
  // skills haven't loaded yet — refreshes after the first success
  // populate in the background without a UI hint.
  const slashPopupFooter = useMemo(() => {
    if (skillsError) {
      return { tone: "error" as const, message: `Skills: ${skillsError}` };
    }
    if (skillsLoading && !skillsLoaded) {
      return { tone: "muted" as const, message: "Loading skills…" };
    }
    return null;
  }, [skillsError, skillsLoading, skillsLoaded]);

  const filteredItems = useMemo(
    () => filterSlashItems(allSlashItems, slashAnchor?.query ?? ""),
    [allSlashItems, slashAnchor?.query],
  );

  // Keep highlighted item valid: if the current highlight no longer
  // matches the filter, jump to the first visible item. This also
  // initialises the highlight when the popup first opens.
  useEffect(() => {
    if (!slashAnchor) return;
    const visible = filteredItems.map((i) => i.id);
    if (slashHighlighted && visible.includes(slashHighlighted)) return;
    setSlashHighlighted(visible[0] ?? null);
  }, [slashAnchor, filteredItems, slashHighlighted]);

  // Popup visibility is driven by *anchor presence only*, not by
  // filter results. An empty filter shows the "No commands match"
  // empty state rather than unmounting; auto-close happens via
  // `findSlashAtCursor` returning null (cursor moved out of slash
  // context) or explicit Esc.
  const slashOpen = slashAnchor !== null;

  // First-open lazy load. The store guards against double-fetch via TTL +
  // in-flight loading flag, so re-firing this effect on every open is
  // harmless and keeps the popup snappy after the initial scan.
  useEffect(() => {
    if (!slashOpen) return;
    void loadSkills(cwd ?? null);
  }, [slashOpen, cwd, loadSkills]);

  const closeSlash = () => {
    setSlashAnchor(null);
    setSlashHighlighted(null);
  };

  const handleSlashSelect = (item: SlashCommandItem) => {
    if (slashAnchor) {
      const consumedLength = 1 + slashAnchor.query.length;
      const before = draft.slice(0, slashAnchor.start);
      const after = draft.slice(slashAnchor.start + consumedLength);

      if (item.id.startsWith("skill:")) {
        // Cursor-style inline expansion. Replace the typed `/<query>`
        // with the full `/<skill-name> ` (trailing space so the user can
        // keep typing context after the token without an extra
        // keystroke). The mirror overlay highlights it; send-time
        // parsing resolves it to the skill body.
        const skillName = item.command.replace(/^\//, "");
        const insertion = `/${skillName}${after.startsWith(" ") ? "" : " "}`;
        const next = before + insertion + after;
        onDraftChange(next);
        // Cursor lands right after the inserted token (and the space we
        // added, if any). Schedule for after onDraftChange propagates.
        const newCursor = (before + insertion).length;
        requestAnimationFrame(() => {
          const el = textareaRef.current;
          if (!el) return;
          el.focus();
          el.setSelectionRange(newCursor, newCursor);
        });
      } else {
        // Mode picks (and any future non-text-token items) strip the
        // typed `/<query>` because the activation is state-only.
        onDraftChange(before + after);
        requestAnimationFrame(() => textareaRef.current?.focus());
      }
    } else {
      requestAnimationFrame(() => textareaRef.current?.focus());
    }
    closeSlash();
    item.onSelect();
  };

  const canSubmit = sessionReady && !streaming && draft.trim().length > 0;

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Shift+Tab cycles modes regardless of popup state. preventDefault
    // is critical — the browser would otherwise move focus out of the
    // textarea via native tab navigation.
    if (e.shiftKey && e.key === "Tab") {
      e.preventDefault();
      const next = nextModeInCycle(mode);
      if (next === "default") {
        onModeRemove();
      } else {
        onModeActivate(next as ActivePillMode);
      }
      return;
    }

    if (slashOpen) {
      // Esc always closes — even when there are no items to pick.
      if (e.key === "Escape") {
        e.preventDefault();
        closeSlash();
        return;
      }
      // When the filter returned no matches, the popup is still
      // visible (showing "No commands match"), but keyboard nav has
      // nothing to do. Fall through to the normal Enter-to-submit
      // path so the user can still send the message-with-slash-text
      // without first dismissing the popup.
      if (filteredItems.length > 0) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          const ids = filteredItems.map((i) => i.id);
          const idx = slashHighlighted ? ids.indexOf(slashHighlighted) : -1;
          const next = ids[(idx + 1) % ids.length];
          if (next) setSlashHighlighted(next);
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          const ids = filteredItems.map((i) => i.id);
          const idx = slashHighlighted ? ids.indexOf(slashHighlighted) : 0;
          const next = ids[(idx - 1 + ids.length) % ids.length];
          if (next) setSlashHighlighted(next);
          return;
        }
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          const item = filteredItems.find((i) => i.id === slashHighlighted);
          if (item) handleSlashSelect(item);
          return;
        }
      }
    }

    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (canSubmit) onSubmit();
    }
  };

  // ─── Slash detection on text change ──────────────────────────────
  // Pure cursor-aware detection — handles slash-at-position-0,
  // slash-after-whitespace, and refuses slash-inside-word. See
  // `findSlashAtCursor` for the full table of cases.
  const handleTextareaChange = (
    e: React.ChangeEvent<HTMLTextAreaElement>,
  ) => {
    const value = e.target.value;
    const cursor = e.target.selectionStart ?? value.length;
    onDraftChange(value);
    if (composingRef.current) return;
    const anchor = findSlashAtCursor(value, cursor);
    if (anchor) {
      setSlashAnchor(anchor);
    } else {
      setSlashAnchor(null);
      setSlashHighlighted(null);
    }
  };

  // Selection changes (arrow keys, mouse click) can move the cursor
  // out of a slash command without changing the text — close the popup
  // when that happens.
  const handleSelect = (e: React.SyntheticEvent<HTMLTextAreaElement>) => {
    if (composingRef.current) return;
    const el = e.currentTarget;
    const anchor = findSlashAtCursor(el.value, el.selectionStart ?? 0);
    if (!anchor) {
      if (slashAnchor !== null) closeSlash();
      return;
    }
    if (
      !slashAnchor ||
      anchor.start !== slashAnchor.start ||
      anchor.query !== slashAnchor.query
    ) {
      setSlashAnchor(anchor);
    }
  };

  return (
    <div className="w-full px-4 pb-3">
      <div className="mx-auto w-full max-w-2xl">
        {zone1Override !== null
          ? <div className="pb-1">{zone1Override}</div>
          : cwd && (
              <div className="px-3 pb-1 text-[11px] text-muted-foreground/70 truncate font-mono">
                {cwd}
              </div>
            )}
        <div
          className={cn(
            "relative",
            "rounded-xl bg-muted/30 ring-1 ring-border/60 focus-within:ring-muted-foreground/60",
            "transition-shadow",
          )}
        >
          <SlashCommandPopup
            items={filteredItems}
            highlightedId={slashHighlighted}
            onHighlightChange={setSlashHighlighted}
            onSelect={handleSlashSelect}
            open={slashOpen}
            footerNote={slashPopupFooter}
          />
          {errorMessage && (
            <div
              role="alert"
              className="px-3 pt-2 text-[11px] text-destructive/90 leading-tight"
            >
              <span>Send failed: {errorMessage}. </span>
              <span className="text-muted-foreground/80">
                Press Enter to retry.
              </span>
            </div>
          )}
          <div className="relative">
            {/*
              Mirror overlay: renders the same text as the textarea but
              with `/skill-name` tokens wrapped in a colored span. The
              textarea on top has transparent text + a visible caret, so
              the user sees the highlighted mirror through the
              transparent layer. Critical that the two layers share
              identical padding/font/line-height so cursor and mirror
              stay glued together at every position.
            */}
            <div
              aria-hidden
              data-testid="composer-highlight-mirror"
              className={cn(
                "pointer-events-none absolute inset-0 px-3 py-2.5",
                "whitespace-pre-wrap break-words",
                "text-sm text-foreground",
                "overflow-hidden",
              )}
            >
              {highlightSegments.map((seg, i) =>
                seg.kind === "skill" ? (
                  <span
                    key={i}
                    className="text-amber-500 dark:text-amber-400"
                  >
                    {seg.text}
                  </span>
                ) : (
                  <Fragment key={i}>{seg.text}</Fragment>
                ),
              )}
              {/* Trailing newline doesn't render unless followed by a
                  glyph — pad with a zero-width space so the mirror's
                  height matches the textarea's after a fresh Enter. */}
              {draft.endsWith("\n") || draft === "" ? "​" : null}
            </div>
            <textarea
              ref={textareaRef}
              value={draft}
              onChange={handleTextareaChange}
              onSelect={handleSelect}
              onKeyDown={handleKeyDown}
              onCompositionStart={() => {
                composingRef.current = true;
              }}
              onCompositionEnd={(e) => {
                composingRef.current = false;
                // After composition ends, run detection on the now-final
                // value so popup state catches up with what was typed.
                const el = e.currentTarget;
                const anchor = findSlashAtCursor(
                  el.value,
                  el.selectionStart ?? el.value.length,
                );
                if (anchor) setSlashAnchor(anchor);
                else closeSlash();
              }}
              placeholder={
                sessionReady ? placeholderForMode(mode) : "Starting session…"
              }
              rows={1}
              className={cn(
                "relative w-full resize-none bg-transparent px-3 py-2.5",
                // Transparent text — the colored mirror behind shows
                // through. Caret stays visible via `caret-foreground`.
                "text-sm text-transparent caret-foreground",
                "placeholder:text-muted-foreground/60",
                "outline-none",
              )}
              style={{ maxHeight: `${MAX_ROWS_APPROX_PX}px` }}
            />
          </div>
          <ComposerFooter
            provider={provider}
            model={model}
            permissionMode={permissionMode}
            effort={effort}
            contextWindow={contextWindow}
            activeModel={activeModel}
            effortLabelMap={effortLabelMap}
            permissionModes={permissionModes}
            ultrathinkInBodyText={ultrathinkInBodyText}
            streaming={streaming}
            canSubmit={canSubmit}
            showProviderPicker={showProviderPicker}
            showStopButton={showStopButton}
            mode={mode}
            onModeActivate={onModeActivate}
            onModeRemove={onModeRemove}
            onProviderChange={onProviderChange}
            onModelChange={onModelChange}
            onPermissionModeChange={onPermissionModeChange}
            onEffortChange={onEffortChange}
            onContextWindowChange={onContextWindowChange}
            onSubmit={onSubmit}
            onStop={onStop}
            controlsDisabled={!sessionReady}
          />
        </div>
      </div>
    </div>
  );
}

/** Placeholder text per Cursor-style mode. `default` keeps the
 *  existing copy; Plan/Ask/Debug swap in mode-specific prompts so
 *  users see at a glance what the pill changes about their next
 *  message. */
function placeholderForMode(mode: ChatMode): string {
  switch (mode) {
    case "plan":
      return "Plan and design before coding…";
    case "ask":
      return "Ask questions without making changes…";
    case "debug":
      return "Debug and troubleshoot issues…";
    case "default":
      return "Message the agent…";
  }
}
