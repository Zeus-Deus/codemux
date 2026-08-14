import { useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  Code2,
  CornerDownRight,
  MessageSquareText,
  RotateCcw,
  TerminalSquare,
  Type,
} from "lucide-react";

import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  discoverInstalledFontFamilies,
  isFontFamilyAvailable,
  isMonospaceFont,
} from "@/lib/font-discovery";
import {
  DEFAULT_CODE_FONT_STACK,
  DEFAULT_INTERFACE_FONT_STACK,
  TYPOGRAPHY_DEFAULTS,
  TYPOGRAPHY_RANGES,
  fontStack,
  normalizeFontFamily,
  resolveTypographySettings,
  type TypographyMode,
} from "@/lib/typography";
import { cn } from "@/lib/utils";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import type { AppearanceSettings } from "@/tauri/types";
import { SegmentedControl, SubsectionHeader } from "./settings-primitives";

type TypographyField =
  | "interface_font_family"
  | "interface_font_size"
  | "conversation_font_family"
  | "conversation_font_size"
  | "code_font_family"
  | "code_font_size"
  | "terminal_font_family"
  | "terminal_font_size";

const SURFACE_COPY = {
  interface: {
    title: "Interface",
    description: "Navigation, controls, labels, and the shape of the whole workspace.",
    icon: Type,
  },
  conversation: {
    title: "Conversation",
    description: "Prompts, user messages, assistant prose, and rendered Markdown.",
    icon: MessageSquareText,
  },
  code: {
    title: "Code",
    description: "Code blocks, diffs, tool output, and the file editor.",
    icon: Code2,
  },
  terminal: {
    title: "Terminal",
    description: "Terminal cells, independent from code when Advanced is enabled.",
    icon: TerminalSquare,
  },
} as const;

export function TypographySettings() {
  const settings = useSyncedSettingsStore((state) => state.settings);
  const appearance = settings.appearance;
  const updateSetting = useSyncedSettingsStore((state) => state.updateSetting);
  const updateSettings = useSyncedSettingsStore((state) => state.updateSettings);
  const typography = useMemo(() => resolveTypographySettings(appearance), [appearance]);

  const updateField = (field: TypographyField, value: string | number | null) => {
    void updateSetting("appearance", field, value).catch(console.error);
  };

  const updateAppearance = (next: Partial<AppearanceSettings>) => {
    void updateSettings({
      ...settings,
      appearance: { ...appearance, ...next },
    }).catch(console.error);
  };

  // A pre-migration blob has no code size of its own and borrows the terminal
  // size. Materialize it the moment the two surfaces are presented as
  // independent, otherwise editing the terminal size drags code along.
  const codeSizeIsBorrowed = appearance.code_font_size == null;

  const updateMode = (mode: TypographyMode) => {
    if (mode === "advanced" && codeSizeIsBorrowed) {
      updateAppearance({ typography_mode: mode, code_font_size: typography.codeSize });
      return;
    }
    void updateSetting("appearance", "typography_mode", mode).catch(console.error);
  };

  const updateDeveloperSize = (size: number) => {
    // The linked view presents one developer size, so persist code + terminal
    // atomically. Advanced values then start from exactly what users saw.
    updateAppearance({ code_font_size: size, terminal_font_size: size });
  };

  const updateTerminalSize = (size: number) => {
    if (codeSizeIsBorrowed) {
      updateAppearance({ code_font_size: typography.codeSize, terminal_font_size: size });
      return;
    }
    updateField("terminal_font_size", size);
  };

  const updateCodeFamily = (family: string | null) => {
    updateAppearance({ shell_font: null, code_font_family: family });
  };

  const updateTerminalFamily = (family: string | null) => {
    updateAppearance({ shell_font: null, terminal_font_family: family });
  };

  // The pickers show what actually renders, so they read the resolved
  // preference rather than the raw field: a migrated blob keeps its choice in
  // `shell_font` until the first write here clears it.
  const storedConversationFamily = normalizeFontFamily(appearance.conversation_font_family);
  const storedTerminalFamily = normalizeFontFamily(
    appearance.terminal_font_family ?? appearance.shell_font,
  );

  const restoreDefaults = () => {
    const nextAppearance: AppearanceSettings = {
      ...appearance,
      shell_font: null,
      typography_mode: TYPOGRAPHY_DEFAULTS.mode,
      interface_font_family: TYPOGRAPHY_DEFAULTS.interfaceFamily,
      interface_font_size: TYPOGRAPHY_DEFAULTS.interfaceSize,
      conversation_font_family: TYPOGRAPHY_DEFAULTS.conversationFamily,
      conversation_font_size: TYPOGRAPHY_DEFAULTS.conversationSize,
      code_font_family: TYPOGRAPHY_DEFAULTS.codeFamily,
      code_font_size: TYPOGRAPHY_DEFAULTS.codeSize,
      terminal_font_family: TYPOGRAPHY_DEFAULTS.terminalFamily,
      terminal_font_size: TYPOGRAPHY_DEFAULTS.terminalSize,
    };
    void updateSettings({ ...settings, appearance: nextAppearance }).catch(console.error);
  };

  const customized =
    typography.mode !== TYPOGRAPHY_DEFAULTS.mode ||
    typography.interfacePreference !== null ||
    typography.codePreference !== null ||
    storedConversationFamily !== null ||
    storedTerminalFamily !== null ||
    typography.interfaceSize !== TYPOGRAPHY_DEFAULTS.interfaceSize ||
    (appearance.conversation_font_size ?? TYPOGRAPHY_DEFAULTS.conversationSize) !==
      TYPOGRAPHY_DEFAULTS.conversationSize ||
    typography.codeSize !== TYPOGRAPHY_DEFAULTS.codeSize ||
    (appearance.terminal_font_size ?? TYPOGRAPHY_DEFAULTS.terminalSize) !==
      TYPOGRAPHY_DEFAULTS.terminalSize;

  return (
    <section className="mt-10" data-testid="typography-settings">
      <SubsectionHeader
        title="Typography"
        description={
          typography.mode === "simple"
            ? "Two carefully linked type systems: one for reading, one for building. Changes apply everywhere immediately."
            : "Tune each reading and developer surface independently. Empty family choices inherit the closest matching font."
        }
        action={
          <SegmentedControl<TypographyMode>
            ariaLabel="Typography controls"
            value={typography.mode}
            onChange={updateMode}
            options={[
              { value: "simple", label: "Simple" },
              { value: "advanced", label: "Advanced" },
            ]}
            size="sm"
          />
        }
      />

      <div className="space-y-3">
        {typography.mode === "simple" ? (
          <>
            <TypographySurfaceCard
              {...SURFACE_COPY.interface}
              title="Interface & conversation"
              description="One typeface across the app; conversation text follows two pixels smaller for a calmer reading rhythm."
              family={typography.interfacePreference}
              defaultFamily="DM Sans"
              defaultStack={DEFAULT_INTERFACE_FONT_STACK}
              size={typography.interfaceSize}
              sizeLabel="Interface font size"
              range={TYPOGRAPHY_RANGES.interface}
              onFamilyChange={(family) => updateField("interface_font_family", family)}
              onSizeChange={(size) => updateField("interface_font_size", size)}
              preview={
                <LinkedInterfacePreview
                  interfaceFamily={typography.interfaceFamily}
                  interfaceSize={typography.interfaceSize}
                  conversationFamily={typography.conversationFamily}
                  conversationSize={typography.conversationSize}
                />
              }
              linkedLabel={`Conversation · ${typography.conversationSize}px`}
            />
            <TypographySurfaceCard
              {...SURFACE_COPY.code}
              title="Developer font"
              description="One monospace face and size shared by code, diffs, the editor, and terminal cells."
              family={typography.codePreference}
              defaultFamily="JetBrains Mono"
              defaultStack={DEFAULT_CODE_FONT_STACK}
              monospace
              size={typography.codeSize}
              sizeLabel="Developer font size"
              range={TYPOGRAPHY_RANGES.code}
              onFamilyChange={updateCodeFamily}
              onSizeChange={updateDeveloperSize}
              preview={
                <LinkedDeveloperPreview
                  codeFamily={typography.codeFamily}
                  codeSize={typography.codeSize}
                  terminalFamily={typography.terminalFamily}
                  terminalSize={typography.terminalSize}
                />
              }
              linkedLabel="Code · terminal"
            />
          </>
        ) : (
          <>
            <TypographySurfaceCard
              {...SURFACE_COPY.interface}
              family={typography.interfacePreference}
              defaultFamily="DM Sans"
              defaultStack={DEFAULT_INTERFACE_FONT_STACK}
              size={typography.interfaceSize}
              sizeLabel="Interface font size"
              range={TYPOGRAPHY_RANGES.interface}
              onFamilyChange={(family) => updateField("interface_font_family", family)}
              onSizeChange={(size) => updateField("interface_font_size", size)}
              preview={
                <InterfacePreview family={typography.interfaceFamily} size={typography.interfaceSize} />
              }
            />
            <TypographySurfaceCard
              {...SURFACE_COPY.conversation}
              family={storedConversationFamily}
              defaultFamily="Follow interface"
              defaultStack={typography.interfaceFamily}
              size={typography.conversationSize}
              sizeLabel="Conversation font size"
              range={TYPOGRAPHY_RANGES.conversation}
              onFamilyChange={(family) => updateField("conversation_font_family", family)}
              onSizeChange={(size) => updateField("conversation_font_size", size)}
              preview={
                <ConversationPreview
                  family={typography.conversationFamily}
                  size={typography.conversationSize}
                />
              }
            />
            <TypographySurfaceCard
              {...SURFACE_COPY.code}
              family={typography.codePreference}
              defaultFamily="JetBrains Mono"
              defaultStack={DEFAULT_CODE_FONT_STACK}
              monospace
              size={typography.codeSize}
              sizeLabel="Code font size"
              range={TYPOGRAPHY_RANGES.code}
              onFamilyChange={updateCodeFamily}
              onSizeChange={(size) => updateField("code_font_size", size)}
              preview={<CodePreview family={typography.codeFamily} size={typography.codeSize} />}
            />
            <TypographySurfaceCard
              {...SURFACE_COPY.terminal}
              family={storedTerminalFamily}
              defaultFamily="Follow code"
              defaultStack={typography.codeFamily}
              monospace
              size={typography.terminalSize}
              sizeLabel="Terminal font size"
              range={TYPOGRAPHY_RANGES.terminal}
              onFamilyChange={updateTerminalFamily}
              onSizeChange={updateTerminalSize}
              preview={
                <TerminalPreview family={typography.terminalFamily} size={typography.terminalSize} />
              }
            />
          </>
        )}
      </div>

      <div className="mt-3 flex items-center justify-between gap-4 px-1">
        <p className="max-w-xl text-[11px] leading-relaxed text-muted-foreground/65">
          Font names sync with your account. If a face is unavailable on another device, Codemux
          falls back safely without changing the saved choice.
        </p>
        {customized ? (
          <button
            type="button"
            onClick={restoreDefaults}
            className="inline-flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          >
            <RotateCcw className="size-3" aria-hidden />
            Restore defaults
          </button>
        ) : null}
      </div>
    </section>
  );
}

function TypographySurfaceCard({
  title,
  description,
  icon: Icon,
  family,
  defaultFamily,
  defaultStack,
  monospace = false,
  size,
  sizeLabel,
  range,
  onFamilyChange,
  onSizeChange,
  preview,
  linkedLabel,
}: {
  title: string;
  description: string;
  icon: typeof Type;
  family: string | null;
  defaultFamily: string;
  defaultStack: string;
  monospace?: boolean;
  size: number;
  sizeLabel: string;
  range: { readonly min: number; readonly max: number };
  onFamilyChange: (family: string | null) => void;
  onSizeChange: (size: number) => void;
  preview: React.ReactNode;
  linkedLabel?: string;
}) {
  return (
    <div className="overflow-hidden rounded-xl border border-border/65 bg-card/35 shadow-[0_1px_0_color-mix(in_srgb,var(--foreground)_3%,transparent)]">
      <div className="flex flex-wrap items-center justify-between gap-4 px-4 py-3.5">
        <div className="flex min-w-[220px] flex-1 items-start gap-3">
          <div className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-lg border border-border/60 bg-background/80 text-muted-foreground">
            <Icon className="size-3.5" strokeWidth={1.7} aria-hidden />
          </div>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <p className="text-[13px] font-semibold text-foreground">{title}</p>
              {linkedLabel ? (
                <span className="rounded-full border border-border/55 bg-muted/35 px-2 py-0.5 font-mono text-[9px] text-muted-foreground/75">
                  {linkedLabel}
                </span>
              ) : null}
            </div>
            <p className="mt-0.5 max-w-md text-[11px] leading-relaxed text-muted-foreground/70">
              {description}
            </p>
          </div>
        </div>
        <div className="flex w-full items-center gap-2 sm:w-auto">
          <FontFamilyPicker
            value={family}
            defaultFamily={defaultFamily}
            defaultStack={defaultStack}
            monospace={monospace}
            onChange={onFamilyChange}
          />
          <FontSizePicker
            value={size}
            label={sizeLabel}
            range={range}
            onChange={onSizeChange}
          />
        </div>
      </div>
      <div className="border-t border-border/50 bg-muted/[0.16] p-3">{preview}</div>
    </div>
  );
}

function FontFamilyPicker({
  value,
  defaultFamily,
  defaultStack,
  monospace,
  onChange,
}: {
  value: string | null;
  defaultFamily: string;
  defaultStack: string;
  monospace: boolean;
  onChange: (family: string | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [families, setFamilies] = useState<readonly string[]>([]);
  const [loading, setLoading] = useState(false);

  const loadFamilies = () => {
    if (families.length > 0 || loading) return;
    setLoading(true);
    void discoverInstalledFontFamilies().then((next) => {
      setFamilies(next);
      setLoading(false);
    });
  };

  const filtered = useMemo(() => {
    const current = normalizeFontFamily(value);
    const all = current && !families.includes(current) ? [current, ...families] : families;
    const available = monospace ? all.filter(isMonospaceFont) : all;
    const needle = query.trim().toLocaleLowerCase();
    return needle ? available.filter((family) => family.toLocaleLowerCase().includes(needle)) : available;
  }, [families, monospace, query, value]);

  const custom = normalizeFontFamily(query);
  const customAllowed =
    custom !== null &&
    !filtered.some((family) => family.toLocaleLowerCase() === custom.toLocaleLowerCase()) &&
    isFontFamilyAvailable(custom) &&
    (!monospace || isMonospaceFont(custom));
  const display = value ?? defaultFamily;

  const selectFamily = (family: string | null) => {
    onChange(family);
    setOpen(false);
    setQuery("");
  };

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) loadFamilies();
        else setQuery("");
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={`Font family: ${display}`}
          className="flex h-8 min-w-0 flex-1 items-center justify-between gap-2 rounded-lg border border-input bg-background/75 px-2.5 text-left text-[12px] text-foreground transition-colors hover:bg-muted/45 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:w-[190px] sm:flex-none"
        >
          <span className="min-w-0 truncate" style={{ fontFamily: fontStack(value, defaultStack) }}>
            {display}
          </span>
          <ChevronDown className="size-3 shrink-0 text-muted-foreground" aria-hidden />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[300px] p-0" onOpenAutoFocus={(event) => event.preventDefault()}>
        <Command shouldFilter={false}>
          <CommandInput
            autoFocus
            placeholder={monospace ? "Search monospace fonts…" : "Search installed fonts…"}
            value={query}
            onValueChange={setQuery}
          />
          <CommandList className="max-h-72 p-1">
            {query.length === 0 ? (
              <CommandItem value="codemux-default" onSelect={() => selectFamily(null)}>
                <span className="min-w-0 flex-1 truncate" style={{ fontFamily: defaultStack }}>
                  {defaultFamily}
                </span>
                <span className="text-[9px] uppercase tracking-[0.08em] text-muted-foreground/55">
                  default
                </span>
                {value === null ? <Check className="size-3.5" aria-hidden /> : null}
              </CommandItem>
            ) : null}
            {customAllowed ? (
              <CommandItem value={`custom-${custom}`} onSelect={() => selectFamily(custom)}>
                <CornerDownRight className="size-3.5 text-muted-foreground" aria-hidden />
                <span className="min-w-0 flex-1 truncate" style={{ fontFamily: fontStack(custom, defaultStack) }}>
                  Use “{custom}”
                </span>
              </CommandItem>
            ) : null}
            {filtered.map((family) => (
              <CommandItem key={family} value={family} onSelect={() => selectFamily(family)}>
                <span
                  className="min-w-0 flex-1 truncate"
                  style={{ fontFamily: fontStack(family, defaultStack) }}
                >
                  {family}
                </span>
                <span className="font-mono text-[10px] text-muted-foreground/45">Ag 01</span>
                {value === family ? <Check className="size-3.5" aria-hidden /> : null}
              </CommandItem>
            ))}
            {loading ? (
              <div className="px-2 py-6 text-center text-[11px] text-muted-foreground">
                Reading fonts on this device…
              </div>
            ) : null}
            {!loading ? <CommandEmpty>No installed font matches that name.</CommandEmpty> : null}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

function FontSizePicker({
  value,
  label,
  range,
  onChange,
}: {
  value: number;
  label: string;
  range: { readonly min: number; readonly max: number };
  onChange: (size: number) => void;
}) {
  return (
    <Select value={String(value)} onValueChange={(next) => onChange(Number(next))}>
      <SelectTrigger className="h-8 w-[76px] bg-background/75 text-[12px]" aria-label={label}>
        <SelectValue>{value}px</SelectValue>
      </SelectTrigger>
      <SelectContent align="end">
        {Array.from({ length: range.max - range.min + 1 }, (_, index) => range.min + index).map(
          (size) => (
            <SelectItem key={size} value={String(size)}>
              {size}px
            </SelectItem>
          ),
        )}
      </SelectContent>
    </Select>
  );
}

function PreviewFrame({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        "overflow-hidden rounded-lg border border-border/55 bg-background/75 shadow-[inset_0_1px_0_color-mix(in_srgb,var(--foreground)_2%,transparent)]",
        className,
      )}
    >
      {children}
    </div>
  );
}

function InterfacePreview({ family, size }: { family: string; size: number }) {
  return (
    <PreviewFrame>
      <div style={{ fontFamily: family, fontSize: size }}>
        <div className="flex items-center gap-2 border-b border-border/45 px-3 py-2">
          <span className="size-2 rounded-[3px] bg-accent-ember" />
          <span className="font-semibold tracking-[-0.01em]">Codemux</span>
          <span className="ml-auto text-[0.72em] text-muted-foreground">main</span>
        </div>
        <div className="flex items-center gap-2.5 px-3 py-2.5">
          <span className="flex size-6 items-center justify-center rounded-md border border-border/60 bg-muted/45 text-[0.68em] font-semibold">
            A
          </span>
          <div className="min-w-0">
            <p className="font-semibold leading-tight">Refine terminal typography</p>
            <p className="mt-0.5 text-[0.76em] text-muted-foreground">Working · 4 files changed</p>
          </div>
          <span className="ml-auto rounded-md bg-muted/55 px-2 py-0.5 text-[0.68em] text-muted-foreground">
            Review
          </span>
        </div>
      </div>
    </PreviewFrame>
  );
}

function ConversationPreview({ family, size }: { family: string; size: number }) {
  return (
    <PreviewFrame>
      <div className="space-y-2.5 px-3 py-3" style={{ fontFamily: family, fontSize: size }}>
        <div className="ml-auto max-w-[72%] rounded-[12px_12px_4px_12px] border border-border/55 bg-card px-3 py-2 leading-relaxed">
          Make the terminal font update without restarting the pane.
        </div>
        <div className="max-w-[88%] leading-relaxed text-foreground/90">
          I’ll update the existing renderer, refit its cell grid, and preserve the current scroll
          position.
        </div>
      </div>
    </PreviewFrame>
  );
}

function CodePreview({ family, size }: { family: string; size: number }) {
  return (
    <PreviewFrame>
      <div className="overflow-x-auto py-2" style={{ fontFamily: family, fontSize: size, lineHeight: 1.55 }}>
        <PreviewCodeLine number="41" tone="muted" text="const term = terminalRef.current;" />
        <PreviewCodeLine number="42" tone="remove" prefix="−" text="term.dispose();" />
        <PreviewCodeLine number="42" tone="add" prefix="+" text="term.options.fontFamily = family;" />
        <PreviewCodeLine number="43" tone="add" prefix="+" text="fitAddon.fit();" />
      </div>
    </PreviewFrame>
  );
}

function PreviewCodeLine({
  number,
  prefix = " ",
  text,
  tone,
}: {
  number: string;
  prefix?: string;
  text: string;
  tone: "muted" | "add" | "remove";
}) {
  return (
    <div
      className={cn(
        "flex min-w-max px-3",
        tone === "add" && "bg-status-open/[0.08] text-status-open",
        tone === "remove" && "bg-status-attention/[0.08] text-status-attention",
        tone === "muted" && "text-muted-foreground",
      )}
    >
      <span className="w-7 shrink-0 select-none text-right opacity-45">{number}</span>
      <span className="w-6 shrink-0 select-none text-center opacity-70">{prefix}</span>
      <span className="pr-4 whitespace-pre">{text}</span>
    </div>
  );
}

function TerminalPreview({ family, size }: { family: string; size: number }) {
  return (
    <PreviewFrame>
      <div
        className="overflow-x-auto bg-[color-mix(in_srgb,var(--background)_84%,black)] px-3 py-2.5"
        style={{ fontFamily: family, fontSize: size, lineHeight: 1.5 }}
      >
        <p className="whitespace-pre">
          <span className="text-status-open">→</span>{" "}
          <span className="text-accent-ember">codemux</span>{" "}
          <span className="text-status-remote">git:(</span>
          <span className="text-status-attention">main</span>
          <span className="text-status-remote">)</span>{" "}
          <span className="text-warning">✗</span> npm check
        </p>
        <p className="whitespace-pre text-muted-foreground">✓ TypeScript · clean</p>
      </div>
    </PreviewFrame>
  );
}

function LinkedInterfacePreview({
  interfaceFamily,
  interfaceSize,
  conversationFamily,
  conversationSize,
}: {
  interfaceFamily: string;
  interfaceSize: number;
  conversationFamily: string;
  conversationSize: number;
}) {
  return (
    <div className="grid gap-2 sm:grid-cols-[0.9fr_1.1fr]">
      <InterfacePreview family={interfaceFamily} size={interfaceSize} />
      <ConversationPreview family={conversationFamily} size={conversationSize} />
    </div>
  );
}

function LinkedDeveloperPreview({
  codeFamily,
  codeSize,
  terminalFamily,
  terminalSize,
}: {
  codeFamily: string;
  codeSize: number;
  terminalFamily: string;
  terminalSize: number;
}) {
  return (
    <div className="grid gap-2 sm:grid-cols-[1.1fr_0.9fr]">
      <CodePreview family={codeFamily} size={codeSize} />
      <TerminalPreview family={terminalFamily} size={terminalSize} />
    </div>
  );
}
