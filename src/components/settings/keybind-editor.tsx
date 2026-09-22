import { useState, useEffect, useCallback, useRef } from "react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { RotateCcw, ChevronDown, Search } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  KEYBIND_REGISTRY,
  KEYBIND_CATEGORIES,
  CATEGORY_LABELS,
  getNativeEntryForCombo,
  type KeybindCategory,
} from "@/lib/keybind-registry";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { normalizeKeyCombo, isModifierOnly } from "@/lib/keybind-utils";
import { useResolvedKeybinds, type ResolvedEntry } from "@/hooks/use-resolved-keybinds";
import { setKeybindRecordingMode } from "@/hooks/use-keyboard-shortcuts";
import {
  useSyncedSettingsStore,
  selectKeyboardShortcuts,
} from "@/stores/synced-settings-store";
import { eyebrowVariants } from "@/components/ui/eyebrow";

/** How long to wait for a keypress before showing the timeout hint */
const RECORDING_TIMEOUT_MS = 4000;

/** Shortcuts that should warn when being unbound via conflict override */
const CRITICAL_IDS = new Set(["commandPalette", "openSettings", "closeOverlay"]);

interface PendingConflict {
  combo: string;
  targetId: string;
  conflictIds: string[];
  /** Owned by a native shortcut: can't be overridden, only cancelled. */
  reserved?: boolean;
}

export function KeybindEditor() {
  const [search, setSearch] = useState("");
  const [recordingId, setRecordingId] = useState<string | null>(null);
  const [recordingTimedOut, setRecordingTimedOut] = useState(false);
  const [pendingConflict, setPendingConflict] = useState<PendingConflict | null>(null);

  const { keybindMap, reverseMap } = useResolvedKeybinds();
  const overrides = useSyncedSettingsStore(selectKeyboardShortcuts);
  const updateSetting = useSyncedSettingsStore((s) => s.updateSetting);
  const hasAnyOverrides = Object.keys(overrides).length > 0;

  /** Read the latest overrides from the store (avoids stale closures). */
  const freshOverrides = () =>
    useSyncedSettingsStore.getState().settings.keyboard.shortcuts;

  const saveOverride = useCallback(
    (id: string, combo: string) => {
      const next = { ...freshOverrides(), [id]: combo };
      updateSetting("keyboard", "shortcuts", next).catch(console.error);
    },
    [updateSetting],
  );

  const removeOverride = useCallback(
    (id: string) => {
      const next = { ...freshOverrides() };
      delete next[id];
      updateSetting("keyboard", "shortcuts", next).catch(console.error);
    },
    [updateSetting],
  );

  const resetAll = useCallback(() => {
    updateSetting("keyboard", "shortcuts", {}).catch(console.error);
  }, [updateSetting]);

  // ── Recording mode key capture ──
  useEffect(() => {
    if (!recordingId) return;
    setKeybindRecordingMode(true);
    setRecordingTimedOut(false);

    const timeout = setTimeout(() => setRecordingTimedOut(true), RECORDING_TIMEOUT_MS);

    const handler = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();

      if (isModifierOnly(e)) return;

      // Escape cancels recording
      if (e.key === "Escape") {
        setRecordingId(null);
        setPendingConflict(null);
        return;
      }

      const combo = normalizeKeyCombo(e);
      if (!combo) return;

      // The desktop app consumes native shortcuts before the page sees them,
      // so an action bound to one would never fire.
      const native = getNativeEntryForCombo(combo);
      if (native) {
        setPendingConflict({ combo, targetId: recordingId, conflictIds: [native.id], reserved: true });
        return;
      }

      // Check for conflicts
      const existing = reverseMap.get(combo);
      const conflictIds = existing?.filter((id) => id !== recordingId) ?? [];
      if (conflictIds.length > 0) {
        setPendingConflict({ combo, targetId: recordingId, conflictIds });
      } else {
        saveOverride(recordingId, combo);
        setRecordingId(null);
      }
    };

    window.addEventListener("keydown", handler, { capture: true });
    return () => {
      clearTimeout(timeout);
      window.removeEventListener("keydown", handler, { capture: true });
      setKeybindRecordingMode(false);
    };
  }, [recordingId, reverseMap, saveOverride]);

  const confirmConflict = useCallback(() => {
    if (!pendingConflict || pendingConflict.reserved) return;
    const next = { ...freshOverrides() };
    for (const id of pendingConflict.conflictIds) {
      next[id] = "";
    }
    next[pendingConflict.targetId] = pendingConflict.combo;
    updateSetting("keyboard", "shortcuts", next).catch(console.error);
    setRecordingId(null);
    setPendingConflict(null);
  }, [pendingConflict, updateSetting]);

  const cancelConflict = useCallback(() => {
    setPendingConflict(null);
    setRecordingId(null);
  }, []);

  // Filter by search query. Native shortcuts belong to the desktop window, so
  // a browser on another device doesn't list them.
  const lowerSearch = search.toLowerCase();
  const remote = isRemoteClient();
  const filteredEntries = (category: KeybindCategory) =>
    KEYBIND_REGISTRY.filter(
      (e) =>
        e.category === category &&
        !(remote && e.native) &&
        (lowerSearch === "" ||
          e.label.toLowerCase().includes(lowerSearch) ||
          e.defaultKeys.toLowerCase().includes(lowerSearch) ||
          (keybindMap.get(e.id)?.activeKeys ?? "").toLowerCase().includes(lowerSearch)),
    );

  return (
    <div>
      <div className="mb-8 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-[1.0625rem] font-semibold tracking-tight text-foreground">
            Keyboard Shortcuts
          </h2>
          <p className="text-body text-muted-foreground/85 mt-1.5 leading-relaxed max-w-prose">
            Click a shortcut to rebind it. Press Escape to cancel.
          </p>
        </div>
        {hasAnyOverrides && (
          <Button
            variant="outline"
            size="sm"
            onClick={resetAll}
            className="shrink-0"
          >
            <RotateCcw className="size-3.5" />
            Reset all
          </Button>
        )}
      </div>

      <div className="relative mb-6">
        <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground/60 pointer-events-none" />
        <Input
          placeholder="Search shortcuts…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="h-9 pl-9 text-body"
        />
      </div>

      <div className="space-y-1">
        {KEYBIND_CATEGORIES.map((cat) => {
          const entries = filteredEntries(cat);
          if (entries.length === 0) return null;
          return (
            <CategoryGroup key={cat} category={cat}>
              {entries.map((entry) => {
                const resolved = keybindMap.get(entry.id);
                if (!resolved) return null;
                return (
                  <KeybindRow
                    key={entry.id}
                    entry={resolved}
                    isRecording={recordingId === entry.id}
                    recordingTimedOut={recordingId === entry.id && recordingTimedOut}
                    pendingConflict={
                      pendingConflict?.targetId === entry.id
                        ? pendingConflict
                        : null
                    }
                    keybindMap={keybindMap}
                    onStartRecording={() => {
                      setPendingConflict(null);
                      setRecordingId(entry.id);
                    }}
                    onReset={() => removeOverride(entry.id)}
                    onConfirmConflict={confirmConflict}
                    onCancelConflict={cancelConflict}
                  />
                );
              })}
            </CategoryGroup>
          );
        })}
      </div>
    </div>
  );
}

function CategoryGroup({
  category,
  children,
}: {
  category: KeybindCategory;
  children: React.ReactNode;
}) {
  return (
    <Collapsible defaultOpen className="border-b border-border/40 last:border-b-0 pb-1.5">
      <CollapsibleTrigger className={cn(
          eyebrowVariants(),
          "group flex items-center gap-1.5 w-full pt-5 pb-2 hover:text-foreground transition-colors duration-150",
        )}>
        <ChevronDown className="size-3 transition-transform duration-150 group-data-[state=closed]:-rotate-90 opacity-60" />
        {CATEGORY_LABELS[category]}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="space-y-px">{children}</div>
      </CollapsibleContent>
    </Collapsible>
  );
}

function KeybindRow({
  entry,
  isRecording,
  recordingTimedOut,
  pendingConflict,
  keybindMap,
  onStartRecording,
  onReset,
  onConfirmConflict,
  onCancelConflict,
}: {
  entry: ResolvedEntry;
  isRecording: boolean;
  recordingTimedOut: boolean;
  pendingConflict: PendingConflict | null;
  keybindMap: Map<string, ResolvedEntry>;
  onStartRecording: () => void;
  onReset: () => void;
  onConfirmConflict: () => void;
  onCancelConflict: () => void;
}) {
  const badgeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (isRecording && badgeRef.current) {
      badgeRef.current.focus();
    }
  }, [isRecording]);

  const activeKeys = entry.activeKeys;
  const isUnbound = activeKeys === "";

  return (
    <div className="group/kb flex items-start justify-between gap-4 py-2 px-2 -mx-2 rounded-md hover:bg-muted/30 transition-colors duration-150">
      <div className="space-y-0.5 min-w-0 flex-1 pt-1">
        <span className="text-body text-foreground">{entry.label}</span>
        {entry.description && (
          <p className="text-body-sm text-muted-foreground/75 leading-relaxed">
            {entry.description}
          </p>
        )}

        {/* Recording timeout hint */}
        {isRecording && recordingTimedOut && !pendingConflict && (
          <p className="text-body-sm text-muted-foreground/80 mt-1.5 leading-relaxed">
            Some shortcuts (e.g. Ctrl+W, Ctrl+T) are captured by the system and
            can't be recorded. Press Escape to cancel.
          </p>
        )}

        {/* A native shortcut owns this combo: nothing to override. */}
        {pendingConflict?.reserved && (() => {
          const owner = keybindMap.get(pendingConflict.conflictIds[0]);
          return (
          <div className="mt-2 flex items-center gap-2 flex-wrap rounded-md border border-warning/30 bg-warning/10 px-2.5 py-2">
            <span className="text-body-sm text-warning">
              {owner?.activeKeys ?? pendingConflict.combo} is reserved for{" "}
              <span className="font-medium">{owner?.label ?? pendingConflict.conflictIds[0]}</span>
            </span>
            <Button variant="ghost" size="xs" className="ml-auto" onClick={onCancelConflict}>
              Cancel
            </Button>
          </div>
          );
        })()}

        {/* Conflict warning */}
        {pendingConflict && !pendingConflict.reserved && (() => {
          const affectsCritical = pendingConflict.conflictIds.some((id) => CRITICAL_IDS.has(id));
          return (
            <div className="space-y-1.5 mt-2 rounded-md border border-warning/30 bg-warning/10 px-2.5 py-2">
              <div className="flex items-center gap-2 flex-wrap">
                <span className="text-body-sm text-warning">
                  Already used by{" "}
                  <span className="font-medium">
                    {pendingConflict.conflictIds
                      .map((id) => keybindMap.get(id)?.label ?? id)
                      .join(", ")}
                  </span>
                </span>
                <div className="flex items-center gap-1 ml-auto">
                  <Button
                    variant="outline"
                    size="xs"
                    onClick={onConfirmConflict}
                  >
                    Override
                  </Button>
                  <Button
                    variant="ghost"
                    size="xs"
                    onClick={onCancelConflict}
                  >
                    Cancel
                  </Button>
                </div>
              </div>
              {affectsCritical && (
                <p className="text-label text-warning/80">
                  This will unbind a navigation shortcut. You can always reach settings from the menu.
                </p>
              )}
            </div>
          );
        })()}
      </div>

      <div className="flex items-center gap-1 shrink-0 pt-0.5">
        {/* Reset button (only for customized bindings) */}
        {entry.isCustom && !isRecording && (
          <button
            onClick={onReset}
            className="opacity-0 group-hover/kb:opacity-100 text-muted-foreground/70 hover:text-foreground transition-opacity duration-150 p-1 rounded-sm hover:bg-muted/60"
            title="Reset to default"
          >
            <RotateCcw className="size-3" />
          </button>
        )}

        {/* Key combo badge. Native shortcuts are fixed, so theirs is inert. */}
        {entry.native ? (
          <span
            title="Handled by the desktop app, so it works even when the interface is frozen. Can't be changed."
            className="text-body-sm font-mono px-2.5 h-7 inline-flex items-center justify-center rounded-md border min-w-[88px] tracking-tight text-foreground/85 border-border/60 bg-muted/40 cursor-default"
          >
            {activeKeys}
          </span>
        ) : (
          <button
            ref={badgeRef}
            onClick={isRecording ? undefined : onStartRecording}
            className={cn(
              "text-body-sm font-mono px-2.5 h-7 inline-flex items-center justify-center rounded-md border min-w-[88px] tracking-tight transition-[color,background-color,border-color] duration-150",
              isRecording
                ? "border-primary/40 bg-primary/10 text-primary-foreground motion-safe:animate-pulse cursor-default"
                : isUnbound
                  ? "text-muted-foreground/50 border-dashed border-border bg-transparent hover:border-border hover:bg-muted/40 cursor-pointer"
                  : entry.isCustom
                    ? "text-foreground border-primary/30 bg-primary/5 hover:bg-primary/10 cursor-pointer"
                    : "text-foreground/85 border-border/60 bg-muted/40 hover:bg-muted hover:border-border cursor-pointer",
            )}
          >
            {isRecording
              ? recordingTimedOut
                ? "Not captured"
                : "Press keys\u2026"
              : isUnbound
                ? "\u2014"
                : activeKeys}
          </button>
        )}
      </div>
    </div>
  );
}
