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
  type KeybindCategory,
} from "@/lib/keybind-registry";
import { normalizeKeyCombo, isModifierOnly } from "@/lib/keybind-utils";
import { useResolvedKeybinds, type ResolvedEntry } from "@/hooks/use-resolved-keybinds";
import { setKeybindRecordingMode } from "@/hooks/use-keyboard-shortcuts";
import {
  useSyncedSettingsStore,
  selectKeyboardShortcuts,
} from "@/stores/synced-settings-store";

/** How long to wait for a keypress before showing the timeout hint */
const RECORDING_TIMEOUT_MS = 4000;

/** Shortcuts that should warn when being unbound via conflict override */
const CRITICAL_IDS = new Set(["commandPalette", "openSettings", "closeOverlay"]);

export function KeybindEditor() {
  const [search, setSearch] = useState("");
  const [recordingId, setRecordingId] = useState<string | null>(null);
  const [recordingTimedOut, setRecordingTimedOut] = useState(false);
  const [pendingConflict, setPendingConflict] = useState<{
    combo: string;
    targetId: string;
    conflictIds: string[];
  } | null>(null);

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
    if (!pendingConflict) return;
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

  // Filter by search query
  const lowerSearch = search.toLowerCase();
  const filteredEntries = (category: KeybindCategory) =>
    KEYBIND_REGISTRY.filter(
      (e) =>
        e.category === category &&
        (lowerSearch === "" ||
          e.label.toLowerCase().includes(lowerSearch) ||
          e.defaultKeys.toLowerCase().includes(lowerSearch) ||
          (keybindMap.get(e.id)?.activeKeys ?? "").toLowerCase().includes(lowerSearch)),
    );

  return (
    <div>
      <div className="mb-8 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-[17px] font-semibold tracking-tight text-foreground">
            Keyboard Shortcuts
          </h2>
          <p className="text-[13px] text-muted-foreground/85 mt-1.5 leading-relaxed max-w-prose">
            Click a shortcut to rebind it. Press Escape to cancel.
          </p>
        </div>
        {hasAnyOverrides && (
          <Button
            variant="outline"
            size="sm"
            onClick={resetAll}
            className="shrink-0 h-8 gap-1.5 text-[12px]"
          >
            <RotateCcw className="h-3.5 w-3.5" />
            Reset all
          </Button>
        )}
      </div>

      <div className="relative mb-6">
        <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground/60 pointer-events-none" />
        <Input
          placeholder="Search shortcuts…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="h-9 pl-9 text-[13px]"
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
      <CollapsibleTrigger className="group flex items-center gap-1.5 w-full pt-5 pb-2 text-[10px] font-semibold uppercase tracking-[0.14em] text-muted-foreground/60 hover:text-foreground transition-colors">
        <ChevronDown className="h-3 w-3 transition-transform duration-150 group-data-[state=closed]:-rotate-90 opacity-60" />
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
  pendingConflict: { combo: string; targetId: string; conflictIds: string[] } | null;
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
    <div className="group/kb flex items-start justify-between gap-4 py-2 px-2 -mx-2 rounded-md hover:bg-muted/30 transition-colors">
      <div className="space-y-0.5 min-w-0 flex-1 pt-1">
        <span className="text-[13px] text-foreground">{entry.label}</span>
        {entry.description && (
          <p className="text-[11.5px] text-muted-foreground/75 leading-relaxed">
            {entry.description}
          </p>
        )}

        {/* Recording timeout hint */}
        {isRecording && recordingTimedOut && !pendingConflict && (
          <p className="text-[11.5px] text-muted-foreground/80 mt-1.5 leading-relaxed">
            Some shortcuts (e.g. Ctrl+W, Ctrl+T) are captured by the system and
            can't be recorded. Press Escape to cancel.
          </p>
        )}

        {/* Conflict warning */}
        {pendingConflict && (() => {
          const affectsCritical = pendingConflict.conflictIds.some((id) => CRITICAL_IDS.has(id));
          return (
            <div className="space-y-1.5 mt-2 rounded-md border border-warning/30 bg-warning/10 px-2.5 py-2">
              <div className="flex items-center gap-2 flex-wrap">
                <span className="text-[11.5px] text-warning">
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
                    size="sm"
                    className="h-6 px-2 text-[11px]"
                    onClick={onConfirmConflict}
                  >
                    Override
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-6 px-2 text-[11px]"
                    onClick={onCancelConflict}
                  >
                    Cancel
                  </Button>
                </div>
              </div>
              {affectsCritical && (
                <p className="text-[11px] text-warning/80">
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
            className="opacity-0 group-hover/kb:opacity-100 text-muted-foreground/70 hover:text-foreground transition-opacity p-1 rounded hover:bg-muted/60"
            title="Reset to default"
          >
            <RotateCcw className="h-3 w-3" />
          </button>
        )}

        {/* Key combo badge */}
        <button
          ref={badgeRef}
          onClick={isRecording ? undefined : onStartRecording}
          className={cn(
            "text-[11.5px] font-mono px-2.5 h-7 inline-flex items-center justify-center rounded-md border min-w-[88px] tracking-tight transition-all",
            isRecording
              ? "border-primary/40 bg-primary/10 text-primary-foreground animate-pulse cursor-default"
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
      </div>
    </div>
  );
}
