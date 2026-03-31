import { useState, useEffect, useCallback, useRef } from "react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { RotateCcw, ChevronDown } from "lucide-react";
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

  const saveOverride = useCallback(
    (id: string, combo: string) => {
      const next = { ...overrides, [id]: combo };
      updateSetting("keyboard", "shortcuts", next).catch(console.error);
    },
    [overrides, updateSetting],
  );

  const removeOverride = useCallback(
    (id: string) => {
      const next = { ...overrides };
      delete next[id];
      updateSetting("keyboard", "shortcuts", next).catch(console.error);
    },
    [overrides, updateSetting],
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
    // Unbind the conflicting actions
    const next = { ...overrides };
    for (const id of pendingConflict.conflictIds) {
      next[id] = "";
    }
    next[pendingConflict.targetId] = pendingConflict.combo;
    updateSetting("keyboard", "shortcuts", next).catch(console.error);
    setRecordingId(null);
    setPendingConflict(null);
  }, [pendingConflict, overrides, updateSetting]);

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
      <div className="flex items-center justify-between mb-6">
        <div>
          <h2 className="text-base font-semibold tracking-tight">
            Keyboard Shortcuts
          </h2>
          <p className="text-sm text-muted-foreground mt-1">
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
            <RotateCcw className="h-3.5 w-3.5 mr-1.5" />
            Reset All
          </Button>
        )}
      </div>

      <Input
        placeholder="Search shortcuts..."
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        className="mb-4"
      />

      <div className="space-y-2">
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
    <Collapsible defaultOpen>
      <CollapsibleTrigger className="flex items-center gap-1.5 w-full py-2 text-xs font-medium uppercase tracking-wider text-muted-foreground hover:text-foreground transition-colors group">
        <ChevronDown className="h-3.5 w-3.5 transition-transform group-data-[state=closed]:-rotate-90" />
        {CATEGORY_LABELS[category]}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="space-y-0">{children}</div>
        <Separator className="mt-2" />
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
    <div className="flex items-center justify-between py-2.5 group">
      <div className="space-y-0.5 pr-4">
        <span className="text-sm">{entry.label}</span>
        {entry.description && (
          <p className="text-xs text-muted-foreground">{entry.description}</p>
        )}

        {/* Recording timeout hint */}
        {isRecording && recordingTimedOut && !pendingConflict && (
          <p className="text-xs text-muted-foreground mt-1">
            Some shortcuts (e.g. Ctrl+W, Ctrl+T) are captured by the system and
            can't be recorded. Press Escape to cancel.
          </p>
        )}

        {/* Conflict warning */}
        {pendingConflict && (() => {
          const affectsCritical = pendingConflict.conflictIds.some((id) => CRITICAL_IDS.has(id));
          return (
            <div className="space-y-1 mt-1.5">
              <div className="flex items-center gap-2">
                <span className="text-xs text-amber-500">
                  Already used by{" "}
                  {pendingConflict.conflictIds
                    .map((id) => keybindMap.get(id)?.label ?? id)
                    .join(", ")}
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-5 px-2 text-[11px]"
                  onClick={onConfirmConflict}
                >
                  Override
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-5 px-2 text-[11px]"
                  onClick={onCancelConflict}
                >
                  Cancel
                </Button>
              </div>
              {affectsCritical && (
                <p className="text-xs text-amber-500/80">
                  This will unbind a navigation shortcut. You can always reach settings from the menu.
                </p>
              )}
            </div>
          );
        })()}
      </div>

      <div className="flex items-center gap-1.5 shrink-0">
        {/* Reset button (only for customized bindings) */}
        {entry.isCustom && !isRecording && (
          <button
            onClick={onReset}
            className="opacity-0 group-hover:opacity-100 text-muted-foreground hover:text-foreground transition-opacity p-0.5"
            title="Reset to default"
          >
            <RotateCcw className="h-3 w-3" />
          </button>
        )}

        {/* Key combo badge */}
        <button
          ref={badgeRef}
          onClick={isRecording ? undefined : onStartRecording}
          className={
            isRecording
              ? "text-xs font-mono px-2.5 py-1 rounded-md border border-accent bg-accent/10 text-accent-foreground animate-pulse cursor-default min-w-[80px] text-center"
              : isUnbound
                ? "text-xs font-mono text-muted-foreground/50 px-2.5 py-1 rounded-md border border-border/50 bg-muted/50 hover:border-border hover:bg-muted transition-colors cursor-pointer min-w-[80px] text-center"
                : `text-xs font-mono text-muted-foreground px-2.5 py-1 rounded-md border bg-muted hover:border-foreground/20 transition-colors cursor-pointer min-w-[80px] text-center ${entry.isCustom ? "border-accent/50" : "border-border"}`
          }
        >
          {isRecording
            ? recordingTimedOut
              ? "Not captured"
              : "Press keys..."
            : isUnbound
              ? "\u2014"
              : activeKeys}
        </button>
      </div>
    </div>
  );
}
