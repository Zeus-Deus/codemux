import { useCallback, useMemo, useRef, useState } from "react";
import { Check, ChevronRight, Download, FileUp, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import {
  applyTheme,
  ANSI_SLOTS,
  BUILT_IN_THEMES,
  contrastRatio,
  createGeneratedTheme,
  importThemeDetailed,
  normalizeColor,
  parseCustomThemes,
  parseCustomTheme,
  resolveTheme,
  serializeTheme,
  THEME_IMPORT_SOURCE_LABEL,
  THEME_ROLES,
  type AnsiSlot,
  type ThemeDefinition,
  type ThemeImportResult,
  type ThemeRole,
} from "@/lib/themes";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUIStore, type ThemeStudioRequest } from "@/stores/ui-store";
import { ThemePreviewShell } from "./theme-preview-shell";
import { ThemeImportSourcePicker, type ThemeImportSourceKind } from "./theme-import-sources";
import { ThemeMarketplacePanel } from "./theme-marketplace";

const EMPTY_THEME_PAYLOADS: unknown[] = [];

type StudioTab = "generate" | "import";

function humanizeToken(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/^./, (character) => character.toUpperCase());
}

function downloadTheme(theme: ThemeDefinition) {
  const blob = new Blob([serializeTheme(theme)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `${theme.id}.codemux-theme.json`;
  anchor.click();
  URL.revokeObjectURL(url);
}

/**
 * The Theme Studio — a modal over whatever you were on, not a rail.
 *
 * Three things it deliberately does differently from the sheet it replaced:
 *
 *  - **It is a centered modal, so the surface behind stays mounted.** Opened
 *    from Settings ▸ Appearance, Esc returns you to Appearance rather than to
 *    the home screen. The command palette mounts inside Settings for the same
 *    reason — see `openCommandPaletteWith`.
 *  - **It does not touch the running app.** The old sheet applied the
 *    candidate to the real root and reverted on close, which meant a 380px
 *    column of form and an app you had to look past. The preview is now a
 *    panel inside the modal ({@link ThemePreviewShell}), so cancelling has
 *    nothing to undo and the preview can show a diff and a terminal — the
 *    surfaces a palette is actually judged on.
 *  - **Generate and Import are the only two tabs.** Hand-editing roles is the
 *    rare path; as a third peer it turned a two-way choice into a three-way
 *    one, so it is a link at the foot of the left column.
 */
export function ThemeStudio() {
  const request = useUIStore((state) => state.themeStudio);
  const close = useUIStore((state) => state.closeThemeStudio);
  return (
    <Dialog open={request !== null} onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent
        showCloseButton={false}
        className="flex h-[min(720px,calc(100vh-4rem))] max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden rounded-2xl border border-border bg-card p-0 sm:max-w-[1000px]"
      >
        {request && (
          <StudioBody
            key={"editThemeId" in request ? `edit:${request.editThemeId}` : request.mode}
            request={request}
            onClose={close}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function StudioBody({
  request,
  onClose,
}: {
  request: ThemeStudioRequest;
  onClose: () => void;
}) {
  const customPayloads = useSyncedSettingsStore(
    (state) => state.settings?.appearance?.custom_themes ?? EMPTY_THEME_PAYLOADS,
  );
  const themeId = useSyncedSettingsStore((state) => state.settings?.appearance?.theme ?? "default");
  const updateSettings = useSyncedSettingsStore((state) => state.updateSettings);
  const customThemes = useMemo(() => parseCustomThemes(customPayloads), [customPayloads]);
  const activeTheme = useMemo(() => resolveTheme(themeId, customThemes), [themeId, customThemes]);

  // Seeded once per open — the Dialog remounts this body for every request,
  // so the request is genuinely initial state rather than a prop to re-sync.
  const seed = useMemo(() => {
    const editing = "editThemeId" in request
      ? customThemes.find((theme) => theme.id === request.editThemeId) ?? null
      : null;
    if (!editing) {
      return {
        editingThemeId: null,
        tab: ("editThemeId" in request ? "generate" : request.mode) as StudioTab,
        label: "Night Signal",
        background: "#11161a",
        accent: "#e8956a",
        roleDraft: null as ThemeDefinition | null,
        rolesOpen: false,
      };
    }
    const fromSeeds = editing.source === "generated" && editing.seeds;
    return {
      editingThemeId: editing.id,
      tab: "generate" as StudioTab,
      label: editing.label,
      background: fromSeeds ? normalizeColor(editing.seeds!.background) ?? "#11161a" : "#11161a",
      accent: fromSeeds ? normalizeColor(editing.seeds!.accent) ?? "#e8956a" : "#e8956a",
      roleDraft: editing,
      // An imported or hand-edited theme has no seeds to reopen from, so the
      // only honest editor for it is the role list.
      rolesOpen: !fromSeeds,
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const [tab, setTab] = useState<StudioTab>(seed.tab);
  const [editingThemeId] = useState<string | null>(seed.editingThemeId);
  const [label, setLabel] = useState(seed.label);
  const [background, setBackground] = useState(seed.background);
  const [accent, setAccent] = useState(seed.accent);
  const [sourceKind, setSourceKind] = useState<ThemeImportSourceKind>("vscode");
  const [importText, setImportText] = useState("");
  const [imported, setImported] = useState<ThemeImportResult | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [roleDraft, setRoleDraft] = useState<ThemeDefinition | null>(seed.roleDraft);
  const [rolesOpen, setRolesOpen] = useState(seed.rolesOpen);
  const fileRef = useRef<HTMLInputElement>(null);

  const generatedTheme = useMemo(() => {
    try {
      return createGeneratedTheme(label, background, accent, editingThemeId ?? undefined);
    } catch {
      return null;
    }
  }, [accent, background, editingThemeId, label]);

  const editedTheme = useMemo(() => {
    if (!roleDraft) return null;
    const backdrop = normalizeColor(roleDraft.roles.background) ?? "#000000";
    if (THEME_ROLES.some((role) => !normalizeColor(roleDraft.roles[role], backdrop))) return null;
    if (ANSI_SLOTS.some((slot) => !normalizeColor(roleDraft.ansi[slot], backdrop))) return null;
    return parseCustomTheme({ ...roleDraft, id: editingThemeId ?? roleDraft.id, label });
  }, [editingThemeId, label, roleDraft]);

  /** What Save writes, and what the preview panel paints. */
  const candidate = rolesOpen
    ? editedTheme
    : tab === "generate"
      ? generatedTheme
      : imported?.theme ?? null;

  /** Parse on paste. There is no Parse button: the result sentence — the
   *  format and the count — is the feedback, and waiting for a click to show
   *  it just delays the only thing worth knowing. */
  const readSource = useCallback((text: string, fallbackLabel?: string) => {
    setImportText(text);
    if (text.trim() === "") {
      setImported(null);
      setImportError(null);
      return;
    }
    try {
      const result = importThemeDetailed(text, fallbackLabel || label || "Imported theme");
      setImported(result);
      setImportError(null);
      setRoleDraft(result.theme);
      setLabel(result.theme.label);
    } catch (cause) {
      setImported(null);
      setImportError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [label]);

  const loadFile = (file: File | undefined) => {
    if (!file) return;
    file
      .text()
      .then((text) => readSource(text, file.name.replace(/\.(?:jsonc?|code-workspace)$/i, "")))
      .catch((cause) => setImportError(String(cause)));
  };

  const saveTheme = () => {
    if (!candidate) return;
    const next = [
      ...customThemes.filter((theme) => theme.id !== candidate.id && theme.id !== editingThemeId),
      candidate,
    ];
    const settings = useSyncedSettingsStore.getState().settings;
    updateSettings({
      ...settings,
      appearance: { ...settings.appearance, custom_themes: next, theme: candidate.id },
    }).catch(console.error);
    applyTheme(candidate);
    onClose();
  };

  /** The saved theme this session is editing, if any. Export and delete only
   *  mean something once a theme exists on disk. */
  const savedTheme = editingThemeId
    ? customThemes.find((theme) => theme.id === editingThemeId) ?? null
    : null;

  const removeTheme = (theme: ThemeDefinition) => {
    const settings = useSyncedSettingsStore.getState().settings;
    const removingActive = activeTheme.id === theme.id;
    updateSettings({
      ...settings,
      appearance: {
        ...settings.appearance,
        custom_themes: customThemes.filter((candidate) => candidate.id !== theme.id),
        theme: removingActive ? "default" : settings.appearance.theme,
      },
    }).catch(console.error);
    // Deleting what the app is wearing has to leave it wearing something.
    if (removingActive) applyTheme(BUILT_IN_THEMES[0]!);
    onClose();
  };

  const updateRole = (role: ThemeRole, value: string) =>
    setRoleDraft((current) => (current ? { ...current, roles: { ...current.roles, [role]: value } } : current));
  const updateAnsi = (slot: AnsiSlot, value: string) =>
    setRoleDraft((current) => (current ? { ...current, ansi: { ...current.ansi, [slot]: value } } : current));

  const openRoles = () => {
    if (!roleDraft) setRoleDraft(candidate ?? activeTheme);
    setRolesOpen(true);
  };

  const subtitle = rolesOpen
    ? "Every semantic surface, explicit and shareable."
    : tab === "generate"
      ? "Codemux solves contrast for every role. You pick two colors."
      : sourceKind === "vscode"
        ? "Bring a theme you already like. Codemux maps it to Codemux roles."
        : "Drop a file or paste the block. Codemux maps it to Codemux roles.";

  return (
    <>
      {/* Header */}
      <div className="flex flex-none items-center gap-3.5 border-b border-border/60 px-[18px] py-4">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <DialogTitle className="text-[14.5px] font-bold tracking-[-0.01em]">
            Customize theme
          </DialogTitle>
          <span className="text-[11.5px] text-muted-foreground">{subtitle}</span>
        </div>
        {!rolesOpen && (
          <div
            role="radiogroup"
            aria-label="Theme source"
            className="flex flex-none rounded-[9px] border border-border/60 bg-muted/40 p-0.5"
          >
            {(["generate", "import"] as const).map((value) => (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={tab === value}
                onClick={() => setTab(value)}
                className={cn(
                  "inline-flex h-[26px] items-center rounded-[7px] px-3.5 text-[11.5px] font-semibold transition-colors",
                  tab === value
                    ? "bg-foreground text-background"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {value === "generate" ? "Generate" : "Import"}
              </button>
            ))}
          </div>
        )}
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="flex-none text-muted-foreground"
          onClick={onClose}
          aria-label="Close theme studio"
        >
          <X className="size-[13px]" />
        </Button>
      </div>

      {/* Body */}
      <div className="flex min-h-0 flex-1">
        <div className="flex w-[330px] flex-none flex-col gap-4 overflow-y-auto border-r border-border/60 p-[18px]">
          {rolesOpen ? (
            <RoleEditorColumn
              draft={roleDraft}
              label={label}
              onLabel={setLabel}
              onRole={updateRole}
              onAnsi={updateAnsi}
              onBack={() => setRolesOpen(false)}
            />
          ) : tab === "generate" ? (
            <GenerateColumn
              label={label}
              onLabel={setLabel}
              background={background}
              onBackground={setBackground}
              accent={accent}
              onAccent={setAccent}
              theme={generatedTheme}
              onEditRoles={openRoles}
            />
          ) : (
            <ImportColumn
              sourceKind={sourceKind}
              onSourceKind={setSourceKind}
              text={importText}
              onText={readSource}
              result={imported}
              error={importError}
              label={label}
              onLabel={setLabel}
              onChooseFile={() => fileRef.current?.click()}
              onEditRoles={openRoles}
            />
          )}
          <input
            ref={fileRef}
            type="file"
            accept=".json,.jsonc,.txt,.css"
            className="hidden"
            onChange={(event) => loadFile(event.target.files?.[0])}
          />
        </div>

        {/* Live preview — the reason the modal is 1000px wide */}
        <div className="flex min-w-0 flex-1 flex-col gap-2.5 bg-background p-[18px]">
          <div className="flex flex-none items-center gap-2.5">
            <span className="font-mono text-[9.5px] font-semibold tracking-[0.1em] text-muted-foreground uppercase">
              Live preview
            </span>
            <span className="h-px flex-1 bg-border/60" />
            <span className="text-[11px] text-muted-foreground">
              {tab === "import" && !rolesOpen
                ? "updates as it parses"
                : "the whole shell, not a swatch strip"}
            </span>
          </div>
          <div className="min-h-0 flex-1">
            <ThemePreviewShell theme={candidate ?? activeTheme} />
          </div>
        </div>
      </div>

      {/* Footer */}
      <div className="flex flex-none items-center gap-2.5 border-t border-border/60 bg-muted/30 px-[18px] py-3">
        <span className="flex-1 text-[11px] text-muted-foreground">
          Applies to shell, terminal, code and editor.
        </span>
        {savedTheme && (
          <>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-[31px] gap-1.5 text-[11.5px]"
              onClick={() => downloadTheme(savedTheme)}
            >
              <Download className="size-3" /> Export
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="text-muted-foreground hover:text-destructive"
              onClick={() => removeTheme(savedTheme)}
              aria-label={`Delete ${savedTheme.label}`}
            >
              <Trash2 className="size-3" />
            </Button>
          </>
        )}
        <Button type="button" variant="outline" size="sm" className="h-[31px] text-[11.5px]" onClick={onClose}>
          Cancel
        </Button>
        <Button
          type="button"
          size="sm"
          className="h-[31px] gap-1.5 text-[11.5px] font-bold"
          onClick={saveTheme}
          disabled={!candidate}
        >
          <Check className="size-3.5" /> Save and apply
        </Button>
      </div>
    </>
  );
}

// ── Left column: Generate ────────────────────────────────────────────────

function GenerateColumn({
  label,
  onLabel,
  background,
  onBackground,
  accent,
  onAccent,
  theme,
  onEditRoles,
}: {
  label: string;
  onLabel: (value: string) => void;
  background: string;
  onBackground: (value: string) => void;
  accent: string;
  onAccent: (value: string) => void;
  theme: ThemeDefinition | null;
  onEditRoles: () => void;
}) {
  return (
    <>
      <NameField value={label} onChange={onLabel} />

      <div className="flex flex-col gap-2.5">
        <ColumnLabel>Base colors</ColumnLabel>
        <SeedRow label="Background" value={background} onChange={onBackground} />
        <SeedRow label="Accent" value={accent} onChange={onAccent} />
      </div>

      {theme ? (
        <SolvedRoles theme={theme} />
      ) : (
        <p className="rounded-[9px] border border-warning/25 bg-warning/10 px-3 py-2 text-[11px] leading-relaxed text-warning">
          Use valid colors and keep the background dark enough — Codemux themes
          are dark-only for now.
        </p>
      )}

      <EditRolesLink onClick={onEditRoles} />
    </>
  );
}

/**
 * The solved palette plus the contrast it actually achieved.
 *
 * The generator's claim is that it solves contrast for you; printing the two
 * ratios is what makes that checkable rather than a promise.
 */
function SolvedRoles({ theme }: { theme: ThemeDefinition }) {
  const swatches = [
    theme.roles.background, theme.roles.sidebar, theme.roles.card, theme.roles.border,
    theme.roles.ring, theme.roles.mutedForeground, theme.roles.foreground, theme.roles.brandAccent,
    theme.ansi.green, theme.ansi.yellow, theme.ansi.red, theme.ansi.blue,
    theme.ansi.magenta, theme.ansi.cyan, theme.ansi.brightBlack, theme.ansi.black,
  ];
  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-baseline gap-2">
        <ColumnLabel>Solved roles</ColumnLabel>
        <span className="font-mono text-[10px] text-muted-foreground">
          {THEME_ROLES.length} tokens
        </span>
      </div>
      <div className="grid grid-cols-8 gap-[5px]">
        {swatches.map((color, index) => (
          <span
            key={`${color}-${index}`}
            className="h-[22px] rounded-[5px] border border-white/[0.09]"
            style={{ background: normalizeColor(color, normalizeColor(theme.roles.background) ?? "#000") ?? color }}
          />
        ))}
      </div>
      <ContrastReadout theme={theme} />
    </div>
  );
}

function ContrastReadout({ theme }: { theme: ThemeDefinition }) {
  const { body, accent, ok } = useMemo(() => {
    const bg = normalizeColor(theme.roles.background) ?? "#000000";
    const surface = normalizeColor(theme.roles.card, bg) ?? bg;
    const body = contrastRatio(normalizeColor(theme.roles.foreground, bg) ?? "#fff", bg);
    const accent = contrastRatio(normalizeColor(theme.roles.brandAccent, surface) ?? "#fff", surface);
    return { body, accent, ok: body >= 7 && accent >= 3 };
  }, [theme]);
  return (
    <div
      className={cn(
        "flex items-center gap-2 rounded-[9px] border px-2.5 py-2",
        ok
          ? "border-status-open/25 bg-status-open/10"
          : "border-warning/25 bg-warning/10",
      )}
    >
      <Check className={cn("size-3 flex-none", ok ? "text-status-open" : "text-warning")} />
      <span className="text-[11px] text-muted-foreground">
        Body text {body.toFixed(1)}:1 · accent on surface {accent.toFixed(1)}:1
      </span>
    </div>
  );
}

// ── Left column: Import ──────────────────────────────────────────────────

function ImportColumn({
  sourceKind,
  onSourceKind,
  text,
  onText,
  result,
  error,
  label,
  onLabel,
  onChooseFile,
  onEditRoles,
}: {
  sourceKind: ThemeImportSourceKind;
  onSourceKind: (kind: ThemeImportSourceKind) => void;
  text: string;
  onText: (text: string, fallbackLabel?: string) => void;
  result: ThemeImportResult | null;
  error: string | null;
  label: string;
  onLabel: (value: string) => void;
  onChooseFile: () => void;
  onEditRoles: () => void;
}) {
  return (
    <>
      <ThemeImportSourcePicker value={sourceKind} onChange={onSourceKind} />

      {sourceKind === "vscode" ? (
        <ThemeMarketplacePanel onPick={(content, name) => onText(content, name)} />
      ) : (
      <div className="flex flex-col gap-1.5">
        <ColumnLabel>{sourceKind === "file" ? "File" : "Paste it here"}</ColumnLabel>
        <div className="overflow-hidden rounded-[10px] border border-border bg-muted/30">
          <Textarea
            value={text}
            onChange={(event) => onText(event.target.value)}
            onDrop={(event) => {
              const dropped = event.dataTransfer.files?.[0];
              if (!dropped) return;
              event.preventDefault();
              dropped.text().then(onText).catch(console.error);
            }}
            placeholder={
              sourceKind === "shadcn"
                ? ".dark {\n  --background: oklch(0.14 0.01 250);\n  --primary: oklch(0.72 0.14 45);\n}"
                : "Paste the colour-theme JSON, or drop the file below."
            }
            aria-label="Theme source"
            className="min-h-[132px] resize-none rounded-none border-0 bg-transparent font-mono text-[10.5px] leading-[1.7] shadow-none focus-visible:ring-0"
          />
          <div className="flex items-center gap-2 border-t border-border/60 px-2.5 py-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-[25px] gap-1.5 px-2.5 text-[10.5px]"
              onClick={onChooseFile}
            >
              <FileUp className="size-[11px]" /> Choose file
            </Button>
            <span className="text-[10.5px] text-muted-foreground">or drop it here</span>
          </div>
        </div>
      </div>
      )}

      {error && (
        <p className="rounded-[10px] border border-destructive/25 bg-destructive/10 px-2.5 py-2 text-[11px] leading-relaxed text-destructive">
          {error}
        </p>
      )}

      {result && (
        <>
          <div className="flex items-center gap-2.5 rounded-[10px] border border-status-open/25 bg-status-open/10 px-2.5 py-2.5">
            <Check className="size-3 flex-none text-status-open" />
            <span className="flex-1 text-[11px] leading-relaxed text-muted-foreground">
              Recognised a{" "}
              <span className="font-bold text-foreground">
                {THEME_IMPORT_SOURCE_LABEL[result.source]}
              </span>{" "}
              theme · {result.mapped.length} of {THEME_ROLES.length} roles mapped
            </span>
          </div>
          {result.derived.length > 0 && <DerivedRoles result={result} />}
        </>
      )}

      <NameField value={label} onChange={onLabel} />
      <EditRolesLink onClick={onEditRoles} />
    </>
  );
}

/**
 * The roles the source didn't carry, which Codemux invented.
 *
 * Listed rather than counted because "22 of 24" doesn't tell you *which* two
 * are ours — and those are exactly the ones worth a look before you apply it.
 * Capped, with the remainder stated rather than silently dropped.
 */
function DerivedRoles({ result }: { result: ThemeImportResult }) {
  const SHOWN = 4;
  const shown = result.derived.slice(0, SHOWN);
  const rest = result.derived.length - shown.length;
  const backdrop = normalizeColor(result.theme.roles.background) ?? "#000000";
  return (
    <div className="flex flex-col gap-[7px]">
      <ColumnLabel>
        {result.derived.length} role{result.derived.length === 1 ? "" : "s"} filled in for you
      </ColumnLabel>
      {shown.map((role) => (
        <div
          key={role}
          className="flex h-[30px] items-center gap-2.5 rounded-lg border border-border/60 bg-muted/30 px-2.5"
        >
          <span
            className="size-3.5 flex-none rounded"
            style={{ background: normalizeColor(result.theme.roles[role], backdrop) ?? backdrop }}
          />
          <span className="flex-1 truncate text-[11px] text-muted-foreground">
            {humanizeToken(role)}
          </span>
          <span className="font-mono text-[10px] text-muted-foreground/70">derived</span>
        </div>
      ))}
      {rest > 0 && (
        <span className="text-[10.5px] text-muted-foreground/70">
          and {rest} more — all listed under Edit roles by hand.
        </span>
      )}
    </div>
  );
}

// ── Left column: role editor ─────────────────────────────────────────────

function RoleEditorColumn({
  draft,
  label,
  onLabel,
  onRole,
  onAnsi,
  onBack,
}: {
  draft: ThemeDefinition | null;
  label: string;
  onLabel: (value: string) => void;
  onRole: (role: ThemeRole, value: string) => void;
  onAnsi: (slot: AnsiSlot, value: string) => void;
  onBack: () => void;
}) {
  if (!draft) return null;
  return (
    <>
      <button
        type="button"
        onClick={onBack}
        className="inline-flex w-fit items-center gap-1.5 text-[11.5px] font-semibold text-muted-foreground transition-colors hover:text-foreground"
      >
        <ChevronRight className="size-2.5 rotate-180" />
        Back
      </button>
      <NameField value={label} onChange={onLabel} />
      <div className="flex flex-col gap-2">
        <ColumnLabel>Application roles</ColumnLabel>
        {THEME_ROLES.map((role) => (
          <ColorRoleInput
            key={role}
            label={humanizeToken(role)}
            value={draft.roles[role]}
            onChange={(value) => onRole(role, value)}
          />
        ))}
      </div>
      <div className="flex flex-col gap-2">
        <ColumnLabel>ANSI and syntax</ColumnLabel>
        {ANSI_SLOTS.map((slot) => (
          <ColorRoleInput
            key={slot}
            label={humanizeToken(slot)}
            value={draft.ansi[slot]}
            onChange={(value) => onAnsi(slot, value)}
          />
        ))}
      </div>
    </>
  );
}

// ── Shared bits ──────────────────────────────────────────────────────────

function ColumnLabel({ children }: { children: React.ReactNode }) {
  return <span className="text-[11.5px] font-semibold text-muted-foreground">{children}</span>;
}

function NameField({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  return (
    <label className="flex flex-col gap-1.5">
      <ColumnLabel>Name</ColumnLabel>
      <Input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        maxLength={48}
        aria-label="Theme name"
        className="h-[34px] rounded-[9px] text-[12.5px]"
      />
    </label>
  );
}

function SeedRow({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="flex h-[38px] items-center gap-2.5 rounded-[9px] border border-border bg-muted/30 px-2.5">
      <input
        type="color"
        value={normalizeColor(value) ?? "#000000"}
        onChange={(event) => onChange(event.target.value)}
        aria-label={`${label} color`}
        className="size-5 flex-none cursor-pointer rounded-md border-0 bg-transparent p-0"
      />
      <span className="flex-1 text-[11.5px] text-muted-foreground">{label}</span>
      <input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        aria-label={`${label} value`}
        className="w-[76px] bg-transparent text-right font-mono text-[11.5px] text-foreground outline-none"
      />
    </label>
  );
}

function ColorRoleInput({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="flex min-w-0 items-center gap-2 rounded-lg border border-border/70 bg-background/45 px-2 py-1.5">
      <input
        type="color"
        value={normalizeColor(value) ?? "#000000"}
        onChange={(event) => onChange(event.target.value)}
        className="h-5 w-6 shrink-0 cursor-pointer border-0 bg-transparent p-0"
        aria-label={`${label} color`}
      />
      <span className="min-w-0 flex-1 truncate text-[10px] text-muted-foreground">{label}</span>
      <Input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        aria-label={`${label} value`}
        className="h-7 w-28 border-0 px-0 font-mono text-[10px] shadow-none focus-visible:ring-0"
      />
    </label>
  );
}

function EditRolesLink({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="mt-auto inline-flex w-fit items-center gap-1.5 text-[11.5px] font-semibold text-muted-foreground transition-colors hover:text-foreground"
    >
      Edit roles by hand
      <ChevronRight className="size-2.5" />
    </button>
  );
}
