import { useState, useEffect, useCallback, useMemo } from "react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { WindowChrome } from "@/components/layout/window-chrome";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { ScrollArea } from "@/components/ui/scroll-area";
import { basename } from "@/lib/path";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetDescription,
  SheetFooter,
} from "@/components/ui/sheet";
import { Slider } from "@/components/ui/slider";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Archive,
  ArrowLeft,
  Palette,
  Code2,
  TerminalSquare,
  GitBranch,
  Keyboard,
  Bell,
  Bot,
  Zap,
  FolderCog,
  Trash2,
  X,
  Plus,
  UserCircle,
  LogOut,
  Globe,
  RotateCcw,
  ShieldCheck,
  BookOpen,
  Server,
  Sparkles,
  MonitorSmartphone,
  GitPullRequest,
} from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { useAuthStore } from "@/stores/auth-store";
import {
  useSyncedSettingsStore,
  selectTerminalFontSize,
  selectTerminalCursorStyle,
  selectDefaultEditor,
  selectDefaultBaseBranch,
  selectBrowserDefaultViewport,
} from "@/stores/synced-settings-store";
import {
  useSettingsStore,
  SETTINGS_DEFAULTS,
  selectTerminalColorTheme,
  selectPalette,
  selectDensity,
  selectChatCodeWrap,
  selectSidebarShowGitStats,
  selectWorkingIndicator,
  selectWorkingIndicatorColor,
  type AppearancePalette,
  type AppearanceDensity,
  type AutoSettleDays,
  type WorkingIndicatorVariant,
  type WorkingIndicatorColor,
} from "@/stores/settings-store";
import { WorkingIndicator } from "@/components/ui/working-indicator";
import {
  detectEditors,
  setNotificationSoundEnabled,
  setAiCommitMessageEnabled,
  setAiCommitMessageCli,
  setAiCommitMessageModel,
  setAiResolverCli,
  setAiResolverModel,
  setAiResolverStrategy,
  getProjectScripts,
  setProjectScripts,
  getWorkspaceConfig,
  hasCodemuxinclude,
  getBrowserDataSize,
  clearBrowserCookies,
  clearAllBrowserData,
} from "@/tauri/commands";
import type { EditorInfo, PresetStoreSnapshot, TerminalPreset, LaunchMode, AgentChatProviderKind, ModelSelection } from "@/tauri/types";
import { MultiProviderModelPicker } from "@/components/chat/pickers/MultiProviderModelPicker";
import { EditorIcon } from "@/components/icons/editor-icon";
import { PresetIcon } from "@/components/icons/preset-icon";
import {
  getPresets,
  createPreset,
  reorderPresets,
  setPresetPinned,
  setPresetBarVisible,
  deletePreset,
  updatePreset,
} from "@/tauri/commands";
import {
  detectLaunchFamily,
  familyToProviderKind,
  REASONING_FLAG_FAMILIES,
  GEMINI_MODELS,
  type LaunchModel,
  type ReasoningOption,
} from "@/lib/launch-models";
import { LaunchModelPicker } from "@/components/overlays/launch-model-picker";
import { LaunchReasoningPicker } from "@/components/overlays/launch-reasoning-picker";
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import {
  useLaunchGeminiModels,
  useLaunchGeminiModelsInit,
} from "@/stores/gemini-models-store";
import { onPresetsChanged } from "@/tauri/events";
import {
  DndContext,
  PointerSensor,
  KeyboardSensor,
  useSensor,
  useSensors,
  closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical, Star } from "lucide-react";

type Section = "interface" | "account" | "appearance" | "editor" | "terminal" | "presets" | "projects" | "archive" | "git" | "source_control" | "agent" | "permissions" | "skills" | "mcp" | "hosts" | "remote_access" | "browser" | "shortcuts" | "notifications" | "session_restore";

interface NavItem { id: Section; label: string; icon: React.ElementType }
interface NavGroup { label: string; items: NavItem[] }

/** Build the Settings nav groups for the current flag state.
 *  - The "Interface" row (home of the Agent Chat GUI toggle, default
 *    on) lives in PERSONAL and stays visible regardless of the flag
 *    so users in either mode can find their way back.
 *  - The chat-only rows (Permissions, Skills, MCP Servers) are only
 *    surfaced when the GUI is on; they reach into chat-only data.
 *    Hiding them when off matches the "feature absent, no work
 *    performed" promise of the master toggle.
 *  - The pre-existing rows (Account, Appearance, …, Agent) stay
 *    visible regardless. The Account section's Skills-sync subsection
 *    is conditionally rendered separately below.
 */
function buildNavGroups(agentChatEnabled: boolean): NavGroup[] {
  const editorWorkflowItems: NavItem[] = [
    { id: "editor", label: "Editor", icon: Code2 },
    { id: "terminal", label: "Terminal", icon: TerminalSquare },
    { id: "presets", label: "Presets", icon: Zap },
    { id: "projects", label: "Projects", icon: FolderCog },
    // Archived workspaces — the restore/delete surface for everything
    // archived from the sidebar. Sits next to Projects because both
    // manage the workspace lifecycle rather than personal preferences.
    { id: "archive", label: "Archive", icon: Archive },
    { id: "git", label: "Git", icon: GitBranch },
    // Source Control — which hosting product each checkout talks to, and
    // whether that product's CLI is installed and signed in. Sits next to
    // Git because it is the hosting half of the same subject.
    { id: "source_control", label: "Source Control", icon: GitPullRequest },
    { id: "agent", label: "Agent", icon: Bot },
    ...(agentChatEnabled
      ? ([
          { id: "permissions", label: "Permissions", icon: ShieldCheck },
          { id: "skills", label: "Skills", icon: BookOpen },
          { id: "mcp", label: "MCP Servers", icon: Server },
        ] as NavItem[])
      : []),
    { id: "browser", label: "Browser", icon: Globe },
    // Hosts pane — Step 2 of cloud-push. Listed in Editor & Workflow
    // because picking which machine to run on is a workflow decision,
    // not a personal preference. Always visible (no flag gate) since
    // the underlying daemon is now standard built-in behavior.
    { id: "hosts", label: "Devices", icon: Server },
    // Remote Access — expose this desktop to a browser on another device.
    // Sits next to Devices: both are about reaching this machine (or its
    // sessions) from somewhere else. Always visible; the feature itself is
    // default-off behind the section's master toggle.
    { id: "remote_access", label: "Remote Access", icon: MonitorSmartphone },
    { id: "session_restore", label: "Session Restore", icon: RotateCcw },
  ];

  return [
    {
      label: "PERSONAL",
      items: [
        { id: "account", label: "Account", icon: UserCircle },
        { id: "appearance", label: "Appearance", icon: Palette },
        { id: "interface", label: "Interface", icon: Sparkles },
        { id: "notifications", label: "Notifications", icon: Bell },
        { id: "shortcuts", label: "Shortcuts", icon: Keyboard },
      ],
    },
    {
      label: "EDITOR & WORKFLOW",
      items: editorWorkflowItems,
    },
  ];
}

/** All sections that exist regardless of flag — used to validate the
 *  initial-section URL hash. The chat-only sections are
 *  intentionally included here too: a stale URL hash to `?settings=skills`
 *  from a flag-on session falls back to "account" via the validity
 *  check (see `initialSection` below) when the flag is off. */
const ALL_SECTION_IDS: Section[] = [
  "interface",
  "account", "appearance", "editor", "terminal", "presets", "projects",
  "archive", "git", "source_control", "agent", "permissions", "skills", "mcp",
  "hosts", "remote_access", "browser", "shortcuts", "notifications",
  "session_restore",
];

import { KeybindEditor } from "./keybind-editor";
import { ArchiveSection } from "./archive-section";
import { InterfaceSection } from "./interface-section";
import { HostsSection } from "./hosts-section";
import { SourceControlSection } from "./source-control-section";
import { SubsectionHeader } from "./settings-primitives";
import { RemoteAccessSection } from "./remote-access-section";
import { McpSection } from "./mcp-section";
import { PermissionsSection } from "./permissions-section";
import { SkillsSection } from "./skills-section";
import { SmoothScrollingSection } from "./smooth-scrolling-section";
import { SyncSection } from "./sync-section";
import { useFeatureFlags } from "@/stores/feature-flags";

function SettingRow({ label, description, children }: {
  label: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-8 py-4">
      <div className="space-y-1 min-w-0">
        <p className="text-[14px] font-semibold leading-tight text-foreground">{label}</p>
        {description && (
          <p className="text-[12px] text-muted-foreground/80 leading-relaxed">{description}</p>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

function SectionHeader({ title, description }: { title: string; description: string }) {
  return (
    <div className="mb-7">
      <h2 className="text-[21px] font-bold tracking-tight text-foreground">{title}</h2>
      <p className="text-[14px] text-muted-foreground/80 mt-1.5 leading-relaxed max-w-prose">{description}</p>
    </div>
  );
}


/** Section break — adds breathing room between subsections inside a
 *  single settings page. First subsection gets no extra top margin. */
function SectionGroup({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("mt-10 first:mt-0", className)}>{children}</section>
  );
}

/** Calm grouped surface for things like environment-variable lists,
 *  test-connection panels, info banners. Subtle border, very soft
 *  bg — should never compete with content inside it. */
function SettingsCard({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "rounded-lg border border-border/60 bg-muted/30 p-4",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** Form field with label + helper + content. Used by any section
 *  that has free-form inputs (textareas, multi-input forms). Keeps
 *  label/helper rhythm identical across Projects, Git AI agent
 *  pickers, Hosts add-form, etc. */
function FormField({
  label,
  helper,
  caption,
  htmlFor,
  children,
  className,
}: {
  label: string;
  helper?: React.ReactNode;
  caption?: React.ReactNode;
  htmlFor?: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("space-y-2", className)}>
      <div className="space-y-1">
        <label
          htmlFor={htmlFor}
          className="text-[13px] font-medium text-foreground leading-none block"
        >
          {label}
        </label>
        {helper && (
          <p className="text-[12px] text-muted-foreground/85 leading-relaxed">
            {helper}
          </p>
        )}
      </div>
      {children}
      {caption && (
        <p className="text-[11px] text-muted-foreground/70 leading-relaxed">
          {caption}
        </p>
      )}
    </div>
  );
}

/** Segmented control — a bordered pill of mutually-exclusive options
 *  with a neutral foreground-filled active segment (the design system's
 *  "white is the baseline" rule for toggles/selection). */
function SegmentedControl<T extends string>({
  value,
  onChange,
  options,
  ariaLabel,
}: {
  value: T;
  onChange: (value: T) => void;
  options: { value: T; label: string }[];
  ariaLabel?: string;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      className="inline-flex items-center gap-0.5 rounded-lg border border-border bg-muted/30 p-0.5"
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onChange(opt.value)}
            className={cn(
              "rounded-md px-3 py-1 text-[12px] font-medium transition-colors",
              active
                ? "bg-foreground text-background"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}

/** Tile picker for the working-indicator animation variant. Each tile shows
 *  a live preview (in the currently-selected color) + a label. */
const INDICATOR_VARIANTS: { value: WorkingIndicatorVariant; label: string }[] = [
  { value: "braille", label: "Braille" },
  { value: "ring", label: "Ring" },
  { value: "blink", label: "Blink" },
  { value: "sweep", label: "Sweep" },
  { value: "typing", label: "Typing" },
];

function WorkingIndicatorTiles({
  value,
  color,
  onChange,
}: {
  value: WorkingIndicatorVariant;
  color: WorkingIndicatorColor;
  onChange: (value: WorkingIndicatorVariant) => void;
}) {
  return (
    <div
      role="radiogroup"
      aria-label="Working indicator"
      className="flex gap-1.5"
    >
      {INDICATOR_VARIANTS.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            role="radio"
            aria-checked={active}
            aria-label={opt.label}
            onClick={() => onChange(opt.value)}
            className={cn(
              "flex h-[60px] w-[72px] flex-col items-center justify-center gap-1.5 rounded-[10px] border transition-[color,background-color,border-color,transform]",
              active
                ? "border-accent-ember bg-accent-ember/8"
                : "border-border/60 bg-muted/30 hover:bg-muted/50 hover:-translate-y-px",
            )}
          >
            <span className="flex h-5 items-center justify-center">
              <WorkingIndicator variant={opt.value} color={color} preview />
            </span>
            <span className="text-[11px] text-muted-foreground">
              {opt.label}
            </span>
          </button>
        );
      })}
    </div>
  );
}

/** Round color swatches for the working-indicator color. Static token → class
 *  maps (Tailwind can't compose dynamic names). No red — that tone is
 *  reserved for the needs-you dot. */
const INDICATOR_COLORS: {
  value: WorkingIndicatorColor;
  label: string;
  dot: string;
  ring: string;
}[] = [
  { value: "status-working", label: "Amber", dot: "bg-status-working", ring: "ring-status-working" },
  { value: "foreground", label: "White", dot: "bg-foreground", ring: "ring-foreground" },
  { value: "accent-ember", label: "Ember", dot: "bg-accent-ember", ring: "ring-accent-ember" },
  { value: "status-open", label: "Green", dot: "bg-status-open", ring: "ring-status-open" },
  { value: "status-remote", label: "Sky", dot: "bg-status-remote", ring: "ring-status-remote" },
  { value: "accent-violet", label: "Violet", dot: "bg-accent-violet", ring: "ring-accent-violet" },
];

function IndicatorColorSwatches({
  value,
  onChange,
}: {
  value: WorkingIndicatorColor;
  onChange: (value: WorkingIndicatorColor) => void;
}) {
  return (
    <div
      role="radiogroup"
      aria-label="Indicator color"
      className="flex items-center gap-[9px]"
    >
      {INDICATOR_COLORS.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            role="radio"
            aria-checked={active}
            aria-label={opt.label}
            onClick={() => onChange(opt.value)}
            className={cn(
              "size-5 rounded-full transition-transform",
              opt.dot,
              active &&
                cn("ring-2 ring-offset-2 ring-offset-background", opt.ring),
            )}
          />
        );
      })}
    </div>
  );
}

function SettingsNavItem({ icon: Icon, label, active, onClick }: {
  icon: React.ElementType;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "group/nav w-full flex items-center gap-2.5 px-2.5 h-8 rounded-lg text-[13px] font-medium text-left transition-colors duration-150 outline-none focus-visible:ring-1 focus-visible:ring-ring",
        active
          ? "bg-foreground/[0.09] text-foreground"
          : "text-muted-foreground/90 hover:bg-foreground/[0.06] hover:text-foreground",
      )}
    >
      <Icon
        className={cn(
          "h-[15px] w-[15px] shrink-0 transition-colors",
          active
            ? "text-foreground/85"
            : "text-muted-foreground/70 group-hover/nav:text-foreground/80",
        )}
      />
      <span className="truncate">{label}</span>
    </button>
  );
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), units.length - 1);
  return `${(bytes / Math.pow(k, i)).toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
}

/** Options for the default agent-browser viewport. "default" maps to
 *  null (built-in 1280×800 baseline); other values are `WxH` spec
 *  strings stored verbatim in `browser.default_viewport` and consumed
 *  by the Rust side (fresh-daemon apply + `viewport reset` target). */
const DEFAULT_VIEWPORT_OPTIONS = [
  { value: "default", label: "Default (1280×800)" },
  { value: "1920x1080", label: "Full HD (1920×1080)" },
  { value: "2560x1440", label: "QHD (2560×1440)" },
  { value: "3840x2160", label: "4K (3840×2160)" },
] as const;

function BrowserSection() {
  const [dataSize, setDataSize] = useState<number | null>(null);
  const [clearing, setClearing] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const defaultViewport = useSyncedSettingsStore(selectBrowserDefaultViewport);
  const updateSyncedSetting = useSyncedSettingsStore((s) => s.updateSetting);

  const refreshSize = () => {
    getBrowserDataSize().then(setDataSize).catch(() => setDataSize(0));
  };

  useEffect(() => { refreshSize(); }, []);

  const handleClearCookies = async () => {
    if (!confirm("This will clear all saved cookies and site data. You'll need to re-accept cookie consent pages. Continue?")) return;
    setClearing("cookies");
    setMessage(null);
    try {
      await clearBrowserCookies();
      setMessage("Cookies and site data cleared.");
      refreshSize();
    } catch (e) {
      setMessage(`Failed: ${e}`);
    } finally {
      setClearing(null);
    }
  };

  const handleClearAll = async () => {
    if (!confirm("This will completely reset the browser. All cookies, cache, and saved data will be deleted. Continue?")) return;
    setClearing("all");
    setMessage(null);
    try {
      await clearAllBrowserData();
      setMessage("All browser data cleared.");
      refreshSize();
    } catch (e) {
      setMessage(`Failed: ${e}`);
    } finally {
      setClearing(null);
    }
  };

  return (
    <div>
      <SectionHeader
        title="Browser"
        description="Manage the built-in browser profile used by agents and browser panes."
      />
      <div className="space-y-1">
        <SettingRow
          label="Default viewport"
          description="Starting page size for agent browser sessions and the 'viewport reset' target. Match your monitor so agent screenshots look like your own browser."
        >
          <Select
            value={defaultViewport ?? "default"}
            onValueChange={(v) => {
              updateSyncedSetting(
                "browser",
                "default_viewport",
                v === "default" ? null : v,
              ).catch(console.error);
            }}
          >
            <SelectTrigger className="w-48 h-9">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {DEFAULT_VIEWPORT_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>
                  {o.label}
                </SelectItem>
              ))}
              {/* A custom value set elsewhere (CLI, another device) that
                  isn't one of the canned options still renders instead of
                  showing an empty trigger. */}
              {defaultViewport !== null &&
                !DEFAULT_VIEWPORT_OPTIONS.some((o) => o.value === defaultViewport) && (
                  <SelectItem value={defaultViewport}>{defaultViewport}</SelectItem>
                )}
            </SelectContent>
          </Select>
        </SettingRow>
        <Separator />
        <SettingRow
          label="Profile storage"
          description="Total size of cached browser data, screenshots, and session files."
        >
          <span className="text-sm font-mono text-muted-foreground">
            {dataSize === null ? "..." : formatBytes(dataSize)}
          </span>
        </SettingRow>
        <Separator />
        <SettingRow
          label="Clear cookies & site data"
          description="Removes saved cookies and session storage. You'll need to re-accept cookie consent pages."
        >
          <Button
            variant="outline"
            size="sm"
            disabled={clearing !== null}
            onClick={handleClearCookies}
          >
            {clearing === "cookies" ? "Clearing..." : "Clear cookies"}
          </Button>
        </SettingRow>
        <Separator />
        <SettingRow
          label="Clear all browser data"
          description="Completely resets the browser profile. Removes cookies, cache, screenshots, and all saved data."
        >
          <Button
            variant="destructive"
            size="sm"
            disabled={clearing !== null}
            onClick={handleClearAll}
          >
            {clearing === "all" ? "Clearing..." : "Clear all data"}
          </Button>
        </SettingRow>
      </div>
      {message && (
        <SettingsCard className="mt-4 flex items-start gap-3 border-border/50 bg-muted/40">
          <div className="size-1.5 rounded-full bg-success shrink-0 mt-1.5" />
          <p className="text-[12px] text-muted-foreground/90 leading-relaxed">{message}</p>
        </SettingsCard>
      )}
    </div>
  );
}

// Trigger styling that makes the launch model/reasoning pickers render as
// full-width form fields matching the editor's Select dropdowns, instead of
// the composer's rounded-full pills. tailwind-merge lets these override the
// pickers' built-in pill classes (rounded-full, text-[11px], etc.).
const LAUNCH_FIELD_TRIGGER =
  "h-9 w-full justify-between rounded-lg border-input bg-transparent px-3 text-sm font-normal text-foreground dark:bg-input/30 dark:hover:bg-input/50";

/** Wrap a string as a double-quoted shell argument. */
function quotePrompt(value: string): string {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/** Assemble a structured preset's launchable command: base agent command +
 *  the prompt as a trailing positional. Model/reasoning are NOT baked in —
 *  they ride in `model_selection` and are applied at launch. */
function structuredCommandFor(agentCommand: string, prompt: string): string {
  const p = prompt.trim();
  return p ? `${agentCommand} ${quotePrompt(p)}` : agentCommand;
}

function PresetEditorSheet({
  preset,
  open,
  onOpenChange,
  agentOptions,
  isDraft = false,
  onCreate,
}: {
  preset: TerminalPreset | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Builtin CLI agents shown in the Agent dropdown (id, command, icon). */
  agentOptions: TerminalPreset[];
  /** When true, this is an unsaved new preset — nothing is persisted until
   *  the user presses "Create preset". Edits stay local; auto-save is off. */
  isDraft?: boolean;
  onCreate?: (payload: Parameters<typeof createPreset>[0]) => void;
}) {
  // Capability stores — shared with the New Workspace launch picker, so a
  // preset picks models from the exact same live-harvested source.
  const claudeCaps = useProviderCapabilities((s) => s.claude);
  const codexCaps = useProviderCapabilities((s) => s.codex);
  const opencodeCaps = useProviderCapabilities((s) => s.opencode);
  const capsLoaded = useProviderCapabilities((s) => s.loaded);
  const refreshCaps = useProviderCapabilities((s) => s.refresh);
  const geminiModels = useLaunchGeminiModels((s) => s.models);
  useLaunchGeminiModelsInit();

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [commands, setCommands] = useState<string[]>([""]);
  const [launchMode, setLaunchMode] = useState<LaunchMode>("new_tab");
  const [pinned, setPinned] = useState(true);
  const [autoRunOnWorkspace, setAutoRunOnWorkspace] = useState(false);
  const [autoRunOnNewTab, setAutoRunOnNewTab] = useState(false);
  // Structured ("agent launcher") state.
  const [structured, setStructured] = useState(false);
  const [agentCommand, setAgentCommand] = useState("");
  const [modelSelection, setModelSelection] = useState<ModelSelection>({
    model: null,
    reasoning: null,
    context: null,
  });
  const [prompt, setPrompt] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);

  // Sync when preset changes
  useEffect(() => {
    if (!preset) return;
    setName(preset.name);
    setDescription(preset.description ?? "");
    setCommands(preset.commands.length > 0 ? [...preset.commands] : [""]);
    setLaunchMode(preset.launch_mode);
    setPinned(preset.pinned);
    setAutoRunOnWorkspace(preset.auto_run_on_workspace);
    setAutoRunOnNewTab(preset.auto_run_on_new_tab);
    const lc = preset.launch_config ?? null;
    setStructured(lc != null);
    setAgentCommand(lc?.agent_command ?? "");
    setModelSelection(
      lc?.model_selection ?? { model: null, reasoning: null, context: null },
    );
    setPrompt(lc?.prompt ?? "");
    setConfirmDelete(false);
  }, [preset]);

  // ── Launch model/reasoning sourcing (mirrors new-workspace-dialog) ──
  const launchFamily = detectLaunchFamily(agentCommand);
  const launchProviderKind = launchFamily ? familyToProviderKind(launchFamily) : null;
  const launchCaps =
    launchFamily === "claude"
      ? claudeCaps
      : launchFamily === "codex"
        ? codexCaps
        : launchFamily === "opencode"
          ? opencodeCaps
          : null;
  const launchModels = useMemo<LaunchModel[]>(() => {
    if (launchFamily === "gemini") return geminiModels ?? GEMINI_MODELS;
    return (
      launchCaps?.models.map((m) => ({
        id: m.id,
        label: m.label,
        subProvider: m.sub_provider,
      })) ?? []
    );
  }, [launchFamily, launchCaps, geminiModels]);
  const launchModelsLoading =
    launchFamily !== null &&
    launchFamily !== "gemini" &&
    !capsLoaded &&
    launchModels.length === 0;
  const launchCapsModel = useMemo(
    () => launchCaps?.models.find((m) => m.id === modelSelection.model) ?? null,
    [launchCaps, modelSelection.model],
  );
  const reasoningOptions = useMemo<ReasoningOption[]>(() => {
    if (!launchFamily || !REASONING_FLAG_FAMILIES.has(launchFamily)) return [];
    const labels = launchCaps?.effort_label_map ?? {};
    return (launchCapsModel?.effort_levels ?? []).map((lvl) => ({
      value: lvl,
      label: labels[lvl] ?? lvl,
    }));
  }, [launchFamily, launchCaps, launchCapsModel]);
  const launchContextOptions = useMemo<ReasoningOption[]>(
    () =>
      (launchCapsModel?.context_window_options ?? []).map((o) => ({
        value: o.value,
        label: o.label,
      })),
    [launchCapsModel],
  );
  const capsReady = launchFamily === "gemini" || launchCaps !== null;
  const effectiveReasoning =
    !capsReady || reasoningOptions.some((o) => o.value === modelSelection.reasoning)
      ? modelSelection.reasoning
      : null;
  const effectiveContext =
    !capsReady ||
    launchContextOptions.some((o) => o.value === modelSelection.context)
      ? modelSelection.context
      : null;

  // Backstop the app-level capability harvest if a slot hasn't hydrated.
  useEffect(() => {
    if (!open || !launchProviderKind) return;
    const caps =
      launchProviderKind === "claude"
        ? claudeCaps
        : launchProviderKind === "codex"
          ? codexCaps
          : opencodeCaps;
    if (caps === null) void refreshCaps(launchProviderKind);
  }, [open, launchProviderKind, claudeCaps, codexCaps, opencodeCaps, refreshCaps]);

  if (!preset) return null;

  // The agent dropdown selection + icon, derived from the base command.
  const selectedAgentId =
    agentOptions.find((a) => a.commands[0] === agentCommand)?.id ?? "";
  const agentIcon =
    agentOptions.find((a) => a.id === selectedAgentId)?.icon ?? preset.icon;

  // Unified save (no-op for drafts). Immediate handlers (pickers/selects
  // fire before React state settles) pass explicit overrides. Structured
  // presets store the assembled command + launch_config; raw presets store
  // the literal commands. Fields not passed are left untouched on the Rust
  // side, so e.g. the auto-run toggles never wipe launch_config.
  const save = (over?: Partial<{
    launchMode: LaunchMode;
    agentCommand: string;
    modelSelection: ModelSelection;
    prompt: string;
    icon: string | null;
    commands: string[];
    autoRunOnWorkspace: boolean;
    autoRunOnNewTab: boolean;
  }>) => {
    if (isDraft) return;
    const lm = over?.launchMode ?? launchMode;
    const aw = over?.autoRunOnWorkspace ?? autoRunOnWorkspace;
    const an = over?.autoRunOnNewTab ?? autoRunOnNewTab;
    if (structured) {
      const ac = over?.agentCommand ?? agentCommand;
      const ms = over?.modelSelection ?? modelSelection;
      const pr = over?.prompt ?? prompt;
      const cmd = structuredCommandFor(ac, pr);
      const ic =
        over?.icon ??
        (agentOptions.find((a) => a.commands[0] === ac)?.icon ?? preset.icon);
      updatePreset({
        id: preset.id,
        name,
        description: description || null,
        commands: cmd ? [cmd] : [],
        workingDirectory: preset.working_directory,
        launchMode: lm,
        icon: ic,
        autoRunOnWorkspace: aw,
        autoRunOnNewTab: an,
        launchConfig: { agent_command: ac, model_selection: ms, prompt: pr },
      }).catch(console.error);
    } else {
      updatePreset({
        id: preset.id,
        name,
        description: description || null,
        commands: over?.commands ?? commands.filter((c) => c.trim()),
        workingDirectory: preset.working_directory,
        launchMode: lm,
        icon: preset.icon,
        autoRunOnWorkspace: aw,
        autoRunOnNewTab: an,
      }).catch(console.error);
    }
  };

  const handleAgentChange = (agentId: string) => {
    const opt = agentOptions.find((a) => a.id === agentId);
    if (!opt) return;
    const ac = opt.commands[0] ?? "";
    // Model/reasoning are agent-specific — reset them when the agent
    // changes; the user's prompt is preserved.
    const ms: ModelSelection = { model: null, reasoning: null, context: null };
    setAgentCommand(ac);
    setModelSelection(ms);
    save({ agentCommand: ac, modelSelection: ms, icon: opt.icon ?? null });
  };

  const handleModelChange = (model: string | null) => {
    // Picking "Default" (null) clears reasoning/context too — they are
    // attributes of a concrete model.
    const ms: ModelSelection =
      model === null
        ? { model: null, reasoning: null, context: null }
        : { ...modelSelection, model };
    setModelSelection(ms);
    save({ modelSelection: ms });
  };

  const handleReasoningChange = (reasoning: string) => {
    const ms = { ...modelSelection, reasoning };
    setModelSelection(ms);
    save({ modelSelection: ms });
  };

  const handleContextChange = (context: string) => {
    const ms = { ...modelSelection, context };
    setModelSelection(ms);
    save({ modelSelection: ms });
  };

  // Switch between the structured "agent launcher" and raw command editors.
  const handleModeChange = (mode: string) => {
    if (mode === "structured" && !structured) {
      const opt =
        agentOptions.find((a) => a.id === "builtin-claude") ?? agentOptions[0];
      const ac = opt?.commands[0] ?? "";
      const ms: ModelSelection = { model: null, reasoning: null, context: null };
      setStructured(true);
      setAgentCommand(ac);
      setModelSelection(ms);
      if (!isDraft) {
        const cmd = structuredCommandFor(ac, prompt);
        updatePreset({
          id: preset.id,
          name,
          description: description || null,
          commands: cmd ? [cmd] : [],
          workingDirectory: preset.working_directory,
          launchMode,
          icon: opt?.icon ?? preset.icon,
          launchConfig: { agent_command: ac, model_selection: ms, prompt },
        }).catch(console.error);
      }
    } else if (mode === "raw" && structured) {
      const cmd = structuredCommandFor(agentCommand, prompt);
      const cmds = cmd ? [cmd] : [];
      setStructured(false);
      setCommands(cmds.length > 0 ? cmds : [""]);
      if (!isDraft) {
        updatePreset({
          id: preset.id,
          name,
          description: description || null,
          commands: cmds,
          workingDirectory: preset.working_directory,
          launchMode,
          icon: preset.icon,
          clearLaunchConfig: true,
        }).catch(console.error);
      }
    }
  };

  const handleDelete = () => {
    deletePreset(preset.id).catch(console.error);
    onOpenChange(false);
  };

  const handlePinnedChange = (checked: boolean) => {
    setPinned(checked);
    if (isDraft) return;
    setPresetPinned(preset.id, checked).catch(console.error);
  };

  // Snapshot of local editor state shaped for `createPreset` — used when a
  // draft is confirmed.
  const buildCreatePayload = (): Parameters<typeof createPreset>[0] => {
    if (structured) {
      const cmd = structuredCommandFor(agentCommand, prompt);
      return {
        name: name.trim() || "New preset",
        description: description.trim() || null,
        commands: cmd ? [cmd] : [],
        workingDirectory: preset.working_directory ?? null,
        launchMode,
        pinned,
        icon: agentIcon,
        launchConfig: {
          agent_command: agentCommand,
          model_selection: modelSelection,
          prompt,
        },
      };
    }
    return {
      name: name.trim() || "New preset",
      description: description.trim() || null,
      commands: commands.filter((c) => c.trim()),
      workingDirectory: preset.working_directory ?? null,
      launchMode,
      pinned,
      icon: preset.icon,
      launchConfig: null,
    };
  };

  const handleCommandChange = (index: number, value: string) => {
    const next = [...commands];
    next[index] = value;
    setCommands(next);
  };

  const handleCommandBlur = () => save();

  const addCommand = () => setCommands([...commands, ""]);

  const removeCommand = (index: number) => {
    const next = commands.filter((_, i) => i !== index);
    const cleaned = next.length > 0 ? next : [""];
    setCommands(cleaned);
    save({ commands: cleaned.filter((c) => c.trim()) });
  };

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="sm:max-w-xl w-full flex flex-col gap-0 p-0"
        showCloseButton={false}
      >
        {/* Header */}
        <SheetHeader className="border-b p-4">
          <SheetTitle className="flex items-center gap-2 text-sm">
            <PresetIcon icon={agentIcon} className="h-4 w-4 shrink-0" />
            {name || preset.name}
          </SheetTitle>
          <SheetDescription>
            Configure how this preset launches.
          </SheetDescription>
        </SheetHeader>

        {/* Scrollable content */}
        <div className="flex-1 overflow-y-auto p-4 space-y-6">
          {/* Name */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Name</label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              onBlur={() => name !== preset.name && save()}
              placeholder="e.g. Git Pull"
              className="h-9"
            />
          </div>

          {/* Description */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Description</label>
            <Input
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              onBlur={() => save()}
              placeholder="Optional description"
              className="h-9"
            />
          </div>

          {/* Type — agent launcher vs raw command */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Type</label>
            <Select value={structured ? "structured" : "raw"} onValueChange={handleModeChange}>
              <SelectTrigger className="h-9">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="structured">Agent launcher</SelectItem>
                <SelectItem value="raw">Raw command</SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              {structured
                ? "Pick an agent, model, and prompt — Codemux builds the command."
                : "Type the exact shell command(s) to run."}
            </p>
          </div>

          {structured ? (
            <div className="space-y-5">
              {/* Agent */}
              <div className="space-y-2">
                <label className="text-sm font-medium">Agent</label>
                <Select value={selectedAgentId} onValueChange={handleAgentChange}>
                  <SelectTrigger className="h-9">
                    <SelectValue placeholder="Select an agent" />
                  </SelectTrigger>
                  <SelectContent>
                    {agentOptions.map((a) => (
                      <SelectItem key={a.id} value={a.id}>
                        <span className="flex items-center gap-2">
                          <PresetIcon icon={a.icon} className="h-3.5 w-3.5" />
                          {a.name}
                        </span>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              {/* Model + reasoning — the same capability-driven pickers the
                  New Workspace dialog uses, but styled as full-width form
                  fields (via triggerClassName) to match the Select fields in
                  this editor instead of the composer's rounded-full pills.
                  Shown only for agents Codemux can inject a model flag for
                  (Claude/Codex/OpenCode/Gemini); applied at launch via
                  apply_model_selection. */}
              {launchFamily ? (
                <>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">Model</label>
                    <LaunchModelPicker
                      providerKind={launchProviderKind}
                      models={launchModels}
                      loading={launchModelsLoading}
                      selectedModel={modelSelection.model}
                      onModelChange={handleModelChange}
                      triggerClassName={LAUNCH_FIELD_TRIGGER}
                    />
                  </div>
                  {(reasoningOptions.length > 0 ||
                    launchContextOptions.length > 0) && (
                    <div className="space-y-2">
                      <label className="text-sm font-medium">Reasoning</label>
                      <LaunchReasoningPicker
                        reasoningOptions={reasoningOptions}
                        selectedReasoning={effectiveReasoning}
                        defaultReasoning={launchCapsModel?.default_effort ?? null}
                        onReasoningChange={handleReasoningChange}
                        contextOptions={launchContextOptions}
                        selectedContext={effectiveContext}
                        defaultContext={
                          launchCapsModel?.context_window_options.find(
                            (o) => o.is_default,
                          )?.value ?? null
                        }
                        onContextChange={handleContextChange}
                        triggerClassName={LAUNCH_FIELD_TRIGGER}
                      />
                    </div>
                  )}
                  <p className="text-xs text-muted-foreground">
                    Applied at launch. Leave on Default to use the agent's default.
                  </p>
                </>
              ) : (
                <div className="space-y-2">
                  <label className="text-sm font-medium">Model</label>
                  <p className="text-xs text-muted-foreground">
                    Model selection isn't available for this agent.
                  </p>
                </div>
              )}

              {/* Prompt */}
              <div className="space-y-2">
                <label className="text-sm font-medium">Prompt</label>
                <Textarea
                  value={prompt}
                  onChange={(e) => setPrompt(e.target.value)}
                  onBlur={() => save()}
                  placeholder="e.g. pull the latest changes and resolve any conflicts"
                  className="min-h-[72px] text-sm"
                />
                <p className="text-xs text-muted-foreground">
                  Sent to the agent as its first instruction. Leave blank to just launch the agent.
                </p>
              </div>
            </div>
          ) : (
            /* Raw commands */
            <div className="space-y-2">
              <label className="text-sm font-medium">Commands</label>
              <div className="flex flex-col gap-1.5">
                {commands.map((cmd, i) => (
                  <div key={i} className="group/cmd flex items-center gap-2">
                    <Input
                      value={cmd}
                      onChange={(e) => handleCommandChange(i, e.target.value)}
                      onBlur={handleCommandBlur}
                      placeholder="e.g. bun run dev"
                      className="h-9 flex-1 font-mono text-sm"
                    />
                    {commands.length > 1 && (
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        onClick={() => removeCommand(i)}
                        className="shrink-0 opacity-0 group-hover/cmd:opacity-100 transition-opacity hover:bg-destructive/10 hover:text-destructive"
                        aria-label="Remove command"
                      >
                        <X className="h-3.5 w-3.5" />
                      </Button>
                    )}
                  </div>
                ))}
                <Button
                  variant="ghost"
                  size="sm"
                  className="mt-1 w-fit gap-1.5 text-muted-foreground hover:text-foreground"
                  onClick={addCommand}
                >
                  + Add command
                </Button>
              </div>
            </div>
          )}

          {/* Advanced section */}
          <div className="space-y-5 border-t border-border/40 pt-5">
            <p className="text-[11px] font-semibold uppercase tracking-[0.16em] text-muted-foreground/80">
              Advanced
            </p>

            {/* Launch Mode */}
            <div className="space-y-2">
              <label className="text-sm font-medium">Launch Mode</label>
              <Select
                value={launchMode}
                onValueChange={(v: LaunchMode) => {
                  setLaunchMode(v);
                  save({ launchMode: v });
                }}
              >
                <SelectTrigger className="h-9">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="new_tab">Open in new tab</SelectItem>
                  <SelectItem value="split_pane">Split pane</SelectItem>
                </SelectContent>
              </Select>
            </div>

            {/* Auto-run — hidden for unsaved drafts (applies to an existing
                preset; configurable after the preset is created). */}
            {!isDraft && (
            <div className="space-y-3">
              <label className="text-sm font-medium">Auto-run</label>
              <div className="space-y-4">
                <div className="flex items-start gap-3">
                  <Switch
                    checked={autoRunOnWorkspace}
                    onCheckedChange={(checked) => {
                      setAutoRunOnWorkspace(checked);
                      save({ autoRunOnWorkspace: checked });
                    }}
                    className="mt-0.5"
                  />
                  <div className="space-y-0.5">
                    <p className="text-sm font-medium">When creating a workspace</p>
                    <p className="text-xs text-muted-foreground">
                      Automatically launch this preset for new workspaces.
                    </p>
                  </div>
                </div>
                <div className="flex items-start gap-3">
                  <Switch
                    checked={autoRunOnNewTab}
                    onCheckedChange={(checked) => {
                      setAutoRunOnNewTab(checked);
                      save({ autoRunOnNewTab: checked });
                    }}
                    className="mt-0.5"
                  />
                  <div className="space-y-0.5">
                    <p className="text-sm font-medium">When opening a new tab</p>
                    <p className="text-xs text-muted-foreground">
                      Automatically launch this preset for new tabs.
                    </p>
                  </div>
                </div>
              </div>
            </div>
            )}

            {/* Pinned */}
            <div className="flex items-center justify-between">
              <div className="space-y-0.5">
                <p className="text-sm font-medium">Show in preset bar</p>
                <p className="text-xs text-muted-foreground">
                  Pin this preset to the quick-launch bar
                </p>
              </div>
              <Switch checked={pinned} onCheckedChange={handlePinnedChange} />
            </div>
          </div>
        </div>

        {/* Footer */}
        <SheetFooter className="border-t p-4 sm:flex-row sm:items-center sm:justify-between">
          {isDraft ? (
            <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          ) : !preset.is_builtin ? (
            confirmDelete ? (
              <div className="flex items-center gap-2">
                <Button variant="destructive" size="sm" onClick={handleDelete}>
                  Confirm Delete
                </Button>
                <Button variant="ghost" size="sm" onClick={() => setConfirmDelete(false)}>
                  Cancel
                </Button>
              </div>
            ) : (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setConfirmDelete(true)}
                className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              >
                Delete Preset
              </Button>
            )
          ) : (
            <div />
          )}
          <Button
            size="sm"
            className="bg-foreground text-background hover:bg-foreground/90"
            onClick={
              isDraft
                ? () => onCreate?.(buildCreatePayload())
                : () => onOpenChange(false)
            }
          >
            {isDraft ? "Create preset" : "Done"}
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}

export function SettingsView() {
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const settingsSection = useUIStore((s) => s.settingsSection);
  const config = useAppStore((s) => s.appState?.config);
  const storeSet = useSettingsStore((s) => s.set);
  const storeGet = useSettingsStore((s) => s.get);
  const defaultEditor = useSyncedSettingsStore(selectDefaultEditor);
  const cursorStyle = useSyncedSettingsStore(selectTerminalCursorStyle);
  const fontSize = useSyncedSettingsStore(selectTerminalFontSize);
  const baseBranch = useSyncedSettingsStore(selectDefaultBaseBranch);
  const terminalThemeMode = useSettingsStore(selectTerminalColorTheme);
  const palette = useSettingsStore(selectPalette);
  const density = useSettingsStore(selectDensity);
  const showGitStats = useSettingsStore(selectSidebarShowGitStats);
  const chatCodeWrap = useSettingsStore(selectChatCodeWrap);
  const autoSettleDays = useSettingsStore(
    (s) =>
      (s.settings["sidebar.auto_settle_days"] ??
        SETTINGS_DEFAULTS["sidebar.auto_settle_days"]!) as AutoSettleDays,
  );
  const indicatorVariant = useSettingsStore(selectWorkingIndicator);
  const indicatorColor = useSettingsStore(selectWorkingIndicatorColor);
  const autoMcpConfig = storeGet("auto_mcp_config") !== "false";

  const authUser = useAuthStore((s) => s.user);
  const isDevBypass = useAuthStore((s) => s.devBypass);
  const signOut = useAuthStore((s) => s.signOut);
  const syncedSettings = useSyncedSettingsStore((s) => s.settings);
  const updateSyncedSetting = useSyncedSettingsStore((s) => s.updateSetting);

  const activeWorkspace = useAppStore((s) => {
    const st = s.appState;
    return st?.workspaces.find((w) => w.workspace_id === st.active_workspace_id);
  });
  const projectRoot = activeWorkspace?.project_root ?? null;
  const projectName = projectRoot ? basename(projectRoot) || "Project" : "Project";

  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const navGroups = buildNavGroups(enableAgentChat);
  const visibleSectionIds = new Set(navGroups.flatMap((g) => g.items.map((i) => i.id)));
  // The hash from a previous flag-on session might point at a section
  // that's now hidden — fall back to "account" so the user lands on a
  // visible page instead of a blank panel. ALL_SECTION_IDS is used for
  // type-narrowing the URL parameter; visibleSectionIds enforces the
  // current-flag visibility.
  const initialSection: Section =
    settingsSection && (ALL_SECTION_IDS as string[]).includes(settingsSection) && visibleSectionIds.has(settingsSection as Section)
      ? (settingsSection as Section)
      : "account";
  const [activeSection, setActiveSection] = useState<Section>(initialSection);
  const [editors, setEditors] = useState<EditorInfo[]>([]);
  const [presetStore, setPresetStore] = useState<PresetStoreSnapshot | null>(null);
  const [selectedPresetId, setSelectedPresetId] = useState<string | null>(null);
  // An unsaved new preset being edited; persisted only on "Create preset".
  const [draftPreset, setDraftPreset] = useState<TerminalPreset | null>(null);

  // Builtin CLI agents that populate the editor's Agent dropdown. Excludes
  // the Shell (no command) and the Chat Agent (native pane) builtins.
  const agentOptions = useMemo(
    () =>
      presetStore?.presets.filter(
        (p) => p.is_builtin && p.kind === "cli" && p.commands.length > 0,
      ) ?? [],
    [presetStore],
  );

  // Drag-to-reorder presets list. 5px activation distance keeps a
  // plain row click from engaging drag, so clicks still open the
  // editor while drag motion engages sort. The reorder mutation
  // writes to the global preset list — drag in the bar and drag
  // here both flow through the same backend command.
  const presetSensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  const handlePresetDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    if (!presetStore) return;

    const ids = presetStore.presets.map((p) => p.id);
    const fromIndex = ids.indexOf(String(active.id));
    const toIndex = ids.indexOf(String(over.id));
    if (fromIndex < 0 || toIndex < 0) return;

    // Optimistic local update so the row settles into place even
    // before the server emits the next snapshot.
    setPresetStore((prev) => {
      if (!prev) return prev;
      return {
        ...prev,
        presets: arrayMove(prev.presets, fromIndex, toIndex),
      };
    });

    reorderPresets(String(active.id), toIndex).catch((err) => {
      console.error("[settings] reorderPresets failed:", err);
      // Resync from server on error — listener will fire if our
      // optimistic update is stale.
      getPresets().then((s) => setPresetStore(s)).catch(console.error);
    });
  };

  // Project scripts state
  const [setupScripts, setSetupScripts] = useState("");
  const [teardownScripts, setTeardownScripts] = useState("");
  const [runCommand, setRunCommand] = useState("");
  const [worktreeIncludes, setWorktreeIncludes] = useState("");
  const [hasConfigFile, setHasConfigFile] = useState(false);
  const [hasIncludeFile, setHasIncludeFile] = useState(false);

  const setDefaultEditor = (v: string) => {
    updateSyncedSetting("editor", "default_ide", v).catch(console.error);
  };
  const setCursorStyle = (v: string) => {
    updateSyncedSetting("terminal", "cursor_style", v).catch(console.error);
  };
  const setFontSize = (v: number) => {
    updateSyncedSetting("appearance", "terminal_font_size", v).catch(console.error);
  };
  const setBaseBranch = (v: string) => {
    updateSyncedSetting("git", "default_base_branch", v).catch(console.error);
  };
  const setTerminalThemeMode = (v: string) => storeSet("terminal.color_theme", v);
  const setAutoMcpConfig = (v: boolean) => storeSet("auto_mcp_config", v ? "true" : "false");

  // Load project scripts when switching to the projects section
  useEffect(() => {
    if (activeSection !== "projects" || !projectRoot) return;
    getProjectScripts(projectRoot).then((scripts) => {
      if (scripts) {
        setSetupScripts(scripts.setup.join("\n"));
        setTeardownScripts(scripts.teardown.join("\n"));
        setRunCommand(scripts.run ?? "");
        setWorktreeIncludes(scripts.worktree_includes.join("\n"));
      } else {
        setSetupScripts("");
        setTeardownScripts("");
        setRunCommand("");
        setWorktreeIncludes("");
      }
    }).catch(console.error);
    getWorkspaceConfig(projectRoot).then((config) => {
      setHasConfigFile(config !== null);
    }).catch(console.error);
    hasCodemuxinclude(projectRoot).then(setHasIncludeFile).catch(() => setHasIncludeFile(false));
  }, [activeSection, projectRoot]);

  const saveProjectSettings = () => {
    if (!projectRoot) return;
    setProjectScripts(projectRoot, {
      setup: setupScripts.trim() ? setupScripts.trim().split("\n").filter((l) => l.trim()) : [],
      teardown: teardownScripts.trim() ? teardownScripts.trim().split("\n").filter((l) => l.trim()) : [],
      run: runCommand.trim() || null,
      worktree_includes: worktreeIncludes.trim() ? worktreeIncludes.trim().split("\n").filter((l) => l.trim()) : [],
    }).catch(console.error);
  };

  useEffect(() => {
    getPresets().then(setPresetStore).catch(console.error);
    const unlisten = onPresetsChanged((snapshot) => setPresetStore(snapshot));
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // Create a new structured preset (defaults to a Claude launcher) and
  // open its editor. Used by the "New preset" button here and by the
  // preset bar's gear menu (via the `pendingPresetCreate` ui-store flag).
  const handleNewPreset = useCallback(() => {
    // Default the draft to a Claude launcher (first builtin as fallback).
    const claude =
      agentOptions.find((p) => p.id === "builtin-claude") ?? agentOptions[0] ?? null;
    const agentCommand = claude?.commands[0] ?? "claude --dangerously-skip-permissions";
    // Open an unsaved draft — nothing is persisted until the user presses
    // "Create preset" in the editor.
    setSelectedPresetId(null);
    setDraftPreset({
      id: "",
      name: "New preset",
      description: null,
      commands: [agentCommand],
      working_directory: null,
      launch_mode: "new_tab",
      icon: claude?.icon ?? "claude",
      pinned: true,
      is_builtin: false,
      auto_run_on_workspace: false,
      auto_run_on_new_tab: false,
      kind: "cli",
      launch_config: {
        agent_command: agentCommand,
        model_selection: { model: null, reasoning: null, context: null },
        prompt: "",
      },
    });
  }, [agentOptions]);

  // Persist a confirmed draft, then close the editor.
  const handleCreatePreset = useCallback(
    async (payload: Parameters<typeof createPreset>[0]) => {
      try {
        await createPreset(payload);
        setDraftPreset(null);
      } catch (err) {
        console.error("[settings] create preset failed:", err);
      }
    },
    [],
  );

  // Honor a "new preset" request raised elsewhere (e.g. the preset bar's
  // gear menu, which opens settings and sets this flag).
  const pendingPresetCreate = useUIStore((s) => s.pendingPresetCreate);
  useEffect(() => {
    if (!pendingPresetCreate) return;
    // Read+clear from the store so StrictMode's double-invoked effect (or
    // any re-fire) can't create the preset twice — only the first run
    // observes the flag still set.
    if (!useUIStore.getState().pendingPresetCreate) return;
    useUIStore.getState().clearPendingPresetCreate();
    setActiveSection("presets");
    handleNewPreset();
  }, [pendingPresetCreate, handleNewPreset]);

  useEffect(() => {
    detectEditors()
      .then((eds) => {
        setEditors(eds);
        if (eds.length > 0 && !defaultEditor && !useSyncedSettingsStore.getState().isLoading) {
          updateSyncedSetting("editor", "default_ide", eds[0].id).catch(console.error);
        }
      })
      .catch(() => {});
  }, [defaultEditor, updateSyncedSetting]);

  const renderSection = () => {
    switch (activeSection) {
      case "account":
        return (
          <div>
            <SectionHeader
              title="Account"
              description="Your Codemux account details."
            />
            <div className="space-y-1">
              {authUser ? (
                <>
                  <SettingRow label="Email" description="Your sign-in email address.">
                    <span className="font-mono text-[13px] text-muted-foreground">{authUser.email}</span>
                  </SettingRow>
                  <Separator />
                  <SettingRow label="Name" description="Your display name.">
                    <span className="text-sm text-muted-foreground">{authUser.name ?? "—"}</span>
                  </SettingRow>
                  {isDevBypass && (
                    <>
                      <Separator />
                      <SettingRow label="Mode" description="Running in dev bypass mode — no server connection.">
                        <Badge variant="secondary">Dev Mode</Badge>
                      </SettingRow>
                    </>
                  )}
                </>
              ) : (
                <div className="py-4 text-sm text-muted-foreground">
                  Not signed in. Close settings and sign in to manage your account.
                </div>
              )}
            </div>

            {authUser && enableAgentChat && (
              <SectionGroup>
                <SubsectionHeader title="Skills sync" />
                <SyncSection />
              </SectionGroup>
            )}

            {authUser && (
              <SectionGroup>
                <SubsectionHeader title="Session" />
                <SettingsCard className="flex items-center justify-between gap-4 border-destructive/25 bg-destructive/[0.07]">
                  <div className="min-w-0">
                    <p className="text-[14px] font-semibold text-foreground">Sign out of Codemux</p>
                    <p className="text-[12px] text-muted-foreground/80 mt-0.5">
                      You'll need to sign in again to sync settings and use cloud features.
                    </p>
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    className="shrink-0 text-destructive border-destructive/30 hover:bg-destructive/10 hover:text-destructive hover:border-destructive/50 gap-1.5"
                    onClick={() => {
                      signOut();
                      setShowSettings(false);
                    }}
                  >
                    <LogOut className="h-3.5 w-3.5" />
                    Sign out
                  </Button>
                </SettingsCard>
              </SectionGroup>
            )}
          </div>
        );

      case "appearance":
        return (
          <div>
            <SectionHeader
              title="Appearance"
              description="Customize how Codemux looks. Theme changes apply immediately."
            />
            <div className="space-y-1">
              <SettingRow label="Theme preset" description="shadcn preset code used to generate the color system.">
                <Badge variant="secondary" className="font-mono text-xs px-3 py-1">b1HYEHloH</Badge>
              </SettingRow>
              <Separator />
              <SettingRow label="Font family" description="Applied to the entire app shell and terminal.">
                {/* FLAG: display-only — the app shell font is fixed to DM Sans
                    today. Rendered as the design's Select for visual parity;
                    onValueChange is a no-op until a font-swap backend exists. */}
                <Select value="dm-sans" onValueChange={() => {}}>
                  <SelectTrigger className="w-48 h-9">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="dm-sans">DM Sans Variable</SelectItem>
                  </SelectContent>
                </Select>
              </SettingRow>
              <Separator />
              <SettingRow label="Border radius" description="Controls the roundness of all UI elements.">
                <span className="text-sm text-muted-foreground">0.45rem</span>
              </SettingRow>
              <Separator />
              <SettingRow
                label="Resource monitor"
                description="Show the CPU/memory monitor icon in the title bar."
              >
                <Switch
                  checked={syncedSettings.appearance.show_resource_monitor}
                  onCheckedChange={(checked) => {
                    updateSyncedSetting(
                      "appearance",
                      "show_resource_monitor",
                      checked,
                    ).catch(console.error);
                  }}
                />
              </SettingRow>
            </div>

            <SectionGroup>
              <SubsectionHeader
                title="Theme"
                description="Switch the surface palette and overall spacing density. Changes apply immediately across the app."
              />
              <div className="space-y-1">
                <SettingRow
                  label="Color palette"
                  description="Cool keeps a neutral graphite tone (recommended). Warm is the previous ember-tinted surface."
                >
                  <SegmentedControl<AppearancePalette>
                    ariaLabel="Color palette"
                    value={palette}
                    onChange={(value) => storeSet("appearance.palette", value)}
                    options={[
                      { value: "cool", label: "Cool" },
                      { value: "warm", label: "Warm" },
                    ]}
                  />
                </SettingRow>
                <SettingRow
                  label="Density"
                  description="Comfortable gives cards and lists more breathing room. Compact tightens padding and gaps to fit more on screen."
                >
                  <SegmentedControl<AppearanceDensity>
                    ariaLabel="Spacing density"
                    value={density}
                    onChange={(value) => storeSet("appearance.density", value)}
                    options={[
                      { value: "comfortable", label: "Comfortable" },
                      { value: "compact", label: "Compact" },
                    ]}
                  />
                </SettingRow>
                <SettingRow
                  label="Wrap code in chat"
                  description="Soft-wrap long lines in agent chat code blocks. Off keeps each line intact behind a horizontal scroll, which is easier to read for diffs and command output."
                >
                  <Switch
                    checked={chatCodeWrap}
                    onCheckedChange={(checked) =>
                      storeSet("chat.code_wrap", checked ? "true" : "false")
                    }
                  />
                </SettingRow>
              </div>
            </SectionGroup>

            {/* Linux WebKitGTK only — renders null everywhere else. */}
            <SmoothScrollingSection />

            <SectionGroup>
              <SubsectionHeader
                title="Sidebar"
                description="What the workspace inbox shows on each card."
              />
              <div className="space-y-1">
                <SettingRow
                  label="Show git stats"
                  description="Show the ↑ahead and +/− diff numbers on workspace cards. The branch name always shows."
                >
                  <Switch
                    checked={showGitStats}
                    onCheckedChange={(checked) =>
                      storeSet("sidebar.show_git_stats", checked ? "true" : "false")
                    }
                  />
                </SettingRow>
                <SettingRow
                  label="Auto-settle idle work"
                  description="Sweep a workspace card into the Settled section after this many days without agent activity. Cards whose PR merges or closes settle once the agent has also been idle for an hour, so follow-up work stays visible while it is warm. Un-settling a card keeps it active until its agent runs again."
                >
                  <SegmentedControl<AutoSettleDays>
                    ariaLabel="Auto-settle idle work"
                    value={autoSettleDays}
                    onChange={(value) =>
                      storeSet("sidebar.auto_settle_days", value)
                    }
                    options={[
                      { value: "off", label: "Off" },
                      { value: "1", label: "1d" },
                      { value: "3", label: "3d" },
                      { value: "7", label: "7d" },
                      { value: "14", label: "14d" },
                    ]}
                  />
                </SettingRow>
              </div>
            </SectionGroup>

            <SectionGroup>
              <SubsectionHeader
                title="Agents"
                description="The glyph shown in the sidebar while an agent runs."
              />
              <div className="space-y-1">
                <SettingRow
                  label="Working indicator"
                  description="The animation that replaces a workspace's icon while its agent is working."
                >
                  <WorkingIndicatorTiles
                    value={indicatorVariant}
                    color={indicatorColor}
                    onChange={(value) =>
                      storeSet("sidebar.working_indicator", value)
                    }
                  />
                </SettingRow>
                <SettingRow
                  label="Indicator color"
                  description="Tint for the working indicator. Red is reserved for workspaces that need you."
                >
                  <IndicatorColorSwatches
                    value={indicatorColor}
                    onChange={(value) =>
                      storeSet("sidebar.working_indicator_color", value)
                    }
                  />
                </SettingRow>
              </div>

              {/* Live preview at the sidebar row's scale. */}
              <div className="mt-4">
                <p className="mb-1.5 font-mono text-[10px] font-semibold uppercase tracking-[0.14em] text-muted-foreground/55">
                  Preview
                </p>
                <div className="flex max-w-[300px] items-center gap-2 rounded-[10px] border border-border/60 bg-muted/30 px-2.5 py-2">
                  <span className="flex size-5 items-center justify-center shrink-0">
                    <WorkingIndicator
                      variant={indicatorVariant}
                      color={indicatorColor}
                    />
                  </span>
                  <span className="text-[13px] font-semibold text-foreground">
                    Fix scroll pinning on send
                  </span>
                  <span className="ml-auto font-mono text-[11px] text-muted-foreground">
                    6m
                  </span>
                </div>
              </div>
            </SectionGroup>
          </div>
        );

      case "editor":
        return (
          <div>
            <SectionHeader
              title="Editor"
              description="Configure which external editor opens when you click a file."
            />
            <div className="space-y-1">
              <SettingRow label="Default editor" description="Used when opening files from the file tree panel.">
                <Select value={defaultEditor} onValueChange={setDefaultEditor}>
                  <SelectTrigger className="w-48 h-9">
                    <SelectValue placeholder="Select editor" />
                  </SelectTrigger>
                  <SelectContent>
                    {editors.map((ed) => (
                      <SelectItem key={ed.id} value={ed.id}>
                        <span className="flex items-center gap-2">
                          <EditorIcon id={ed.id} className="h-4 w-4" />
                          {ed.name}
                        </span>
                      </SelectItem>
                    ))}
                    {editors.length === 0 && (
                      <SelectItem value="none" disabled>
                        No editors detected
                      </SelectItem>
                    )}
                  </SelectContent>
                </Select>
              </SettingRow>
            </div>

            {editors.length > 0 && (
              <SectionGroup>
                <SubsectionHeader
                  title="Detected editors"
                  description="Editors found on this machine — selecting one above routes file opens to that command."
                />
                <SettingsCard className="divide-y divide-border/40 p-0">
                  {editors.map((ed) => (
                    <div
                      key={ed.id}
                      className="flex items-center justify-between gap-4 px-4 py-2.5"
                    >
                      <span className="flex items-center gap-2 text-[13px] text-foreground">
                        <EditorIcon id={ed.id} className="h-4 w-4" />
                        {ed.name}
                      </span>
                      <code className="text-[11px] text-muted-foreground/85 font-mono bg-background/60 px-2 py-0.5 rounded border border-border/40">
                        {ed.command}
                      </code>
                    </div>
                  ))}
                </SettingsCard>
              </SectionGroup>
            )}
          </div>
        );

      case "terminal":
        return (
          <div>
            <SectionHeader
              title="Terminal"
              description="Configure the terminal emulator behavior and appearance."
            />
            <div className="space-y-1">
              <SettingRow label="Cursor style" description="The shape of the cursor in terminal panes.">
                <Select value={cursorStyle} onValueChange={setCursorStyle}>
                  <SelectTrigger className="w-36 h-9">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="bar">Bar</SelectItem>
                    <SelectItem value="block">Block</SelectItem>
                    <SelectItem value="underline">Underline</SelectItem>
                  </SelectContent>
                </Select>
              </SettingRow>
              <Separator />
              <SettingRow label="Font size" description={`${fontSize}px — adjust the terminal text size.`}>
                <Slider
                  value={[fontSize]}
                  onValueChange={([v]) => setFontSize(v)}
                  min={10}
                  max={22}
                  step={1}
                  className="w-36"
                />
              </SettingRow>
              <Separator />
              <SettingRow label="Color theme" description="How the terminal gets its colors.">
                <Select value={terminalThemeMode} onValueChange={setTerminalThemeMode}>
                  <SelectTrigger className="w-44 h-9">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="app">Match app theme</SelectItem>
                    <SelectItem value="system">System (Omarchy)</SelectItem>
                  </SelectContent>
                </Select>
              </SettingRow>
            </div>
          </div>
        );

      case "presets": {
        const selectedPreset = presetStore?.presets.find((p) => p.id === selectedPresetId) ?? null;
        return (
          <div>
            <SectionHeader
              title="Presets"
              description="Quick-launch presets for CLI agents and tools. Pinned presets appear in the preset bar."
            />
            <div className="space-y-1">
              {presetStore && (
                <SettingRow label="Show preset bar" description="Display the preset quick-launch bar below the tab bar.">
                  <Switch
                    checked={presetStore.bar_visible}
                    onCheckedChange={(checked) => setPresetBarVisible(checked).catch(console.error)}
                  />
                </SettingRow>
              )}
            </div>

            <SectionGroup>
              <SubsectionHeader
                title="Your presets"
                description="Drag the grip to reorder. Click a preset to edit, pin, or delete."
                action={
                  <Button variant="outline" size="sm" className="gap-1.5" onClick={handleNewPreset}>
                    <Plus className="h-3.5 w-3.5" />
                    New preset
                  </Button>
                }
              />
              {presetStore ? (
                <DndContext
                  sensors={presetSensors}
                  collisionDetection={closestCenter}
                  onDragEnd={handlePresetDragEnd}
                >
                  <SortableContext
                    items={presetStore.presets.map((p) => p.id)}
                    strategy={verticalListSortingStrategy}
                  >
                    <div className="overflow-hidden rounded-xl border border-border/60">
                      {presetStore.presets.map((preset) => (
                        <SortablePresetRow
                          key={preset.id}
                          preset={preset}
                          selected={selectedPresetId === preset.id}
                          onSelect={() => setSelectedPresetId(preset.id)}
                          onTogglePin={() =>
                            setPresetPinned(preset.id, !preset.pinned).catch(console.error)
                          }
                          onDelete={() => {
                            deletePreset(preset.id).catch(console.error);
                            if (selectedPresetId === preset.id) setSelectedPresetId(null);
                          }}
                        />
                      ))}
                    </div>
                  </SortableContext>
                </DndContext>
              ) : (
                <p className="text-[13px] text-muted-foreground">Loading presets…</p>
              )}
            </SectionGroup>

            <p className="text-[12px] text-muted-foreground/70 leading-relaxed mt-8">
              Agents in Codemux terminals automatically receive workspace context.{" "}
              <a href="https://docs.codemux.org/agent-awareness" target="_blank" rel="noopener noreferrer" className="underline underline-offset-2 decoration-border hover:decoration-foreground hover:text-foreground transition-colors">
                Learn how to configure it for your tools
              </a>
            </p>

            {/* Editor sheet — renders via portal, not inline. A draft (new,
                unsaved) preset takes precedence over a selected existing one. */}
            <PresetEditorSheet
              preset={draftPreset ?? selectedPreset}
              agentOptions={agentOptions}
              isDraft={draftPreset != null}
              open={draftPreset != null || selectedPreset != null}
              onCreate={handleCreatePreset}
              onOpenChange={(open) => {
                if (!open) {
                  setDraftPreset(null);
                  setSelectedPresetId(null);
                }
              }}
            />
          </div>
        );
      }

      case "git":
        return (
          <div>
            <SectionHeader
              title="Git"
              description="Configure git behavior for workspace creation."
            />
            <div className="space-y-1">
              <SettingRow label="Default base branch" description="Used as the default when creating new feature branches.">
                <Input
                  value={baseBranch}
                  onChange={(e) => setBaseBranch(e.target.value)}
                  className="w-36 h-9"
                />
              </SettingRow>
            </div>

            <SectionGroup>
              <SubsectionHeader
                title="AI Tools"
                description="AI-assisted git workflows. Requires the Claude CLI."
              />
              <div className="space-y-1">
                <SettingRow label="AI commit messages" description="Show the generate button next to the commit input.">
                  <Switch
                    checked={config?.ai_commit_message_enabled ?? true}
                    onCheckedChange={(checked) => {
                      setAiCommitMessageEnabled(checked).catch(console.error);
                      storeSet("ai_commit_message_enabled", String(checked));
                    }}
                  />
                </SettingRow>
                {/* Same `MultiProviderModelPicker` the resolver row below
                    uses (and the agent-chat composer). All three providers
                    are shown — the commit-message backend in
                    `src-tauri/src/ai.rs:generate_commit_message` now
                    dispatches via `build_resolver_argv`, the same builder
                    the merge resolver uses, so claude / codex / opencode
                    all work. Picking a model atomically writes both the
                    CLI and the model so they can never drift out of sync.
                    Reuse buys us favorites carry-over for free: star a
                    model anywhere (chat composer, this row, the resolver
                    row) and it's starred everywhere via the shared
                    `picker-favorites-store`. */}
                <SettingRow label="Agent" description="Which AI agent (and model) generates commit messages.">
                  <MultiProviderModelPicker
                    provider={(config?.ai_commit_message_cli ?? "claude") as AgentChatProviderKind}
                    model={config?.ai_commit_message_model ?? null}
                    onProviderModelChange={(provider, model) => {
                      setAiCommitMessageCli(provider).catch(console.error);
                      storeSet("ai_commit_message_cli", provider);
                      setAiCommitMessageModel(model).catch(console.error);
                      storeSet("ai_commit_message_model", model);
                    }}
                    disabled={!(config?.ai_commit_message_enabled ?? true)}
                  />
                </SettingRow>
              </div>
            </SectionGroup>

            <SectionGroup>
              <SubsectionHeader
                title="Merge Conflict Resolver"
                description="AI-powered merge conflict resolution. When conflicts are detected, the AI works on a temp branch and you review the diff before applying."
              />
              <div className="space-y-1">
                {/* One picker that sets BOTH `ai_resolver_cli` and
                    `ai_resolver_model`. Same component the agent-chat
                    composer uses (provider rail + searchable model list),
                    populated from the same `provider-capabilities-store` —
                    so opencode shows the user's actual configured providers,
                    not a hardcoded list. Replaces the previous "CLI tool"
                    Select + freeform "Model override" Input pair, which had
                    two failure modes: (a) typing a model name that didn't
                    exist for the selected CLI, (b) the CLI selector and
                    model field drifting out of sync. The picker keeps them
                    inherently consistent. */}
                <SettingRow
                  label="Agent"
                  description="Which AI agent (and model) resolves conflicts."
                >
                  <MultiProviderModelPicker
                    provider={(config?.ai_resolver_cli ?? "claude") as AgentChatProviderKind}
                    model={config?.ai_resolver_model ?? null}
                    onProviderModelChange={(provider, model) => {
                      setAiResolverCli(provider).catch(console.error);
                      storeSet("ai_resolver_cli", provider);
                      setAiResolverModel(model).catch(console.error);
                      storeSet("ai_resolver_model", model);
                    }}
                  />
                </SettingRow>
                <SettingRow label="Strategy" description="How the AI should approach conflict resolution.">
                  <Select
                    value={config?.ai_resolver_strategy ?? "smart_merge"}
                    onValueChange={(v) => {
                      setAiResolverStrategy(v).catch(console.error);
                      storeSet("ai_resolver_strategy", v);
                    }}
                  >
                    <SelectTrigger className="w-48 h-9">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="smart_merge">Smart merge</SelectItem>
                      <SelectItem value="keep_both">Keep both</SelectItem>
                      <SelectItem value="prefer_ours">Prefer my branch</SelectItem>
                      <SelectItem value="prefer_theirs">Prefer target</SelectItem>
                    </SelectContent>
                  </Select>
                </SettingRow>
              </div>
            </SectionGroup>
          </div>
        );

      case "shortcuts":
        return <KeybindEditor />;

      case "agent":
        return (
          <div>
            <SectionHeader
              title="Agent"
              description="Configure how Codemux integrates with AI coding agents."
            />
            <div className="space-y-1">
              <SettingRow
                label="Auto-configure MCP for workspaces"
                description="Automatically write .mcp.json so agents discover Codemux tools. Disable if you manage MCP config manually."
              >
                <Switch
                  checked={autoMcpConfig}
                  onCheckedChange={(checked) => {
                    setAutoMcpConfig(checked);
                    storeSet("auto_mcp_config", String(checked));
                  }}
                />
              </SettingRow>
              {/* Run-start rollback checkpoint (issue #80). Only shown
                  when the Agent Chat GUI is on — the snapshot fires on
                  chat-session start, so with the GUI off the toggle
                  would do nothing. */}
              {enableAgentChat && (
                <>
                  <Separator />
                  <SettingRow
                    label="Checkpoint before agent runs"
                    description="When an agent chat session starts, snapshot the working tree in the background so you can roll back everything the run changed. Never delays the agent's response."
                  >
                    <Switch
                      checked={syncedSettings.agent_chat?.checkpoints_enabled ?? false}
                      onCheckedChange={(checked) => {
                        updateSyncedSetting("agent_chat", "checkpoints_enabled", checked).catch(console.error);
                      }}
                    />
                  </SettingRow>
                  <Separator />
                  {/* GUI-mode background browser viewport pin. Like the
                      checkpoint toggle, only meaningful with the Agent
                      Chat GUI on — the peek popover it affects is a
                      GUI-chrome surface. */}
                  <SettingRow
                    label="Desktop-size background browser"
                    description="Pin the agent's background browser to a real desktop viewport (the Browser section's default viewport, 1280×800 out of the box) so pages render at full size in the peek popover, scaled to fit."
                  >
                    <Switch
                      checked={syncedSettings.agent_chat?.background_browser_desktop_viewport ?? true}
                      onCheckedChange={(checked) => {
                        updateSyncedSetting("agent_chat", "background_browser_desktop_viewport", checked).catch(console.error);
                      }}
                    />
                  </SettingRow>
                </>
              )}
            </div>
          </div>
        );

      case "interface":
        return <InterfaceSection />;

      case "permissions":
        // Defensive guard: the nav row is hidden when the Agent Chat
        // GUI is off, but a stale URL hash can still route here.
        // Render the Interface toggle instead so the user lands
        // somewhere useful and learns how to turn the GUI back on.
        return enableAgentChat ? (
          <PermissionsSection projectRoot={projectRoot} />
        ) : (
          <InterfaceSection />
        );

      case "skills":
        return enableAgentChat ? (
          <SkillsSection projectRoot={projectRoot} />
        ) : (
          <InterfaceSection />
        );

      case "mcp":
        return enableAgentChat ? (
          <McpSection projectRoot={projectRoot} />
        ) : (
          <InterfaceSection />
        );

      case "archive":
        return (
          <div>
            <SectionHeader
              title="Archive"
              description="Workspaces archived from the sidebar. Files, branches, and worktrees stay on disk until you delete them here — unarchive to bring a workspace back (files, branch, and worktree are preserved; the pane layout starts fresh)."
            />
            <ArchiveSection />
          </div>
        );

      case "browser":
        return <BrowserSection />;

      case "hosts":
        return (
          <div>
            <SectionHeader
              title="Devices"
              description="Any machine you can SSH into — a home desktop, an always-on box, a cloud server. Push a workspace to any device, pull it back from any device. SSH credentials stay on your machine; only the device name and SSH target sync across your account."
            />
            <HostsSection />
          </div>
        );

      case "source_control":
        return (
          <div>
            <SectionHeader
              title="Source Control"
              description="Codemux reads pull and merge requests through each product's own command-line tool. This is where you check that tooling is installed and signed in, and where you tell Codemux which product a self-hosted server runs."
            />
            <SourceControlSection />
          </div>
        );

      case "remote_access":
        return <RemoteAccessSection />;

      case "projects": {
        const ENV_VARS: Array<[string, string]> = [
          ["$CODEMUX_ROOT_PATH", "Main repo root"],
          ["$CODEMUX_WORKSPACE_PATH", "Workspace / worktree directory"],
          ["$CODEMUX_BRANCH", "Workspace branch name"],
          ["$CODEMUX_PORT", "Allocated port for this workspace"],
          ["$CODEMUX_WORKSPACE_NAME", "Workspace title"],
          ["$CODEMUX_WORKSPACE_ID", "Workspace ID"],
        ];
        return (
          <div>
            <SectionHeader
              title="Projects"
              description={`Automate your workspace lifecycle for ${projectName}. Changes are saved automatically.`}
            />
            {hasConfigFile && (
              <SettingsCard className="mb-6 flex items-start gap-3 border-border/50 bg-muted/40">
                <div className="size-1.5 rounded-full bg-warning shrink-0 mt-1.5" />
                <p className="text-[12px] text-muted-foreground/90 leading-relaxed">
                  A <code className="font-mono text-[11px] bg-background/60 border border-border/40 px-1.5 py-0.5 rounded">.codemux/config.json</code> file was found.
                  File-based configuration takes precedence over these settings.
                </p>
              </SettingsCard>
            )}

            <div className="space-y-8">
              <FormField
                label="Worktree includes"
                helper="Files matching these patterns are copied from the main project into new worktrees. One pattern per line."
                caption={
                  <>
                    Create a <code className="font-mono text-[11px] bg-muted/60 border border-border/40 px-1.5 py-0.5 rounded">.codemuxinclude</code> file
                    in your project root to share patterns with your team. When empty, defaults to{" "}
                    <code className="font-mono text-[11px] bg-muted/60 border border-border/40 px-1.5 py-0.5 rounded">.env .env.* .env.local</code>.{" "}
                    <a href="https://docs.codemux.org" target="_blank" rel="noopener noreferrer" className="underline underline-offset-2 decoration-border hover:decoration-foreground hover:text-foreground transition-colors">Learn more</a>
                  </>
                }
              >
                {hasIncludeFile && (
                  <SettingsCard className="border-border/50 bg-muted/40 py-2.5 px-3 mb-2">
                    <p className="text-[12px] text-muted-foreground/90 leading-relaxed">
                      This project has a <code className="font-mono text-[11px] bg-background/60 border border-border/40 px-1.5 py-0.5 rounded">.codemuxinclude</code> file —
                      those patterns take priority over the settings below.
                    </p>
                  </SettingsCard>
                )}
                <Textarea
                  className="font-mono text-[13px] min-h-[90px] leading-relaxed"
                  placeholder={".env\n.env.*\n.env.local"}
                  value={worktreeIncludes}
                  onChange={(e) => setWorktreeIncludes(e.target.value)}
                  onBlur={saveProjectSettings}
                />
              </FormField>

              <FormField
                label="Setup"
                helper="Runs when a new workspace is created. One command per line."
              >
                <Textarea
                  className="font-mono text-[13px] min-h-[90px] leading-relaxed"
                  placeholder="e.g. npm install"
                  value={setupScripts}
                  onChange={(e) => setSetupScripts(e.target.value)}
                  onBlur={saveProjectSettings}
                />
              </FormField>

              <FormField
                label="Teardown"
                helper="Runs when a workspace is deleted. One command per line."
              >
                <Textarea
                  className="font-mono text-[13px] min-h-[90px] leading-relaxed"
                  placeholder="e.g. docker compose down"
                  value={teardownScripts}
                  onChange={(e) => setTeardownScripts(e.target.value)}
                  onBlur={saveProjectSettings}
                />
              </FormField>

              <FormField
                label="Run"
                helper={
                  <>
                    A command to start your dev server, triggered via{" "}
                    <kbd className="text-[11px] bg-muted/60 border border-border/40 px-1.5 py-0.5 rounded font-mono">Ctrl+Shift+G</kbd>.
                  </>
                }
              >
                <Input
                  className="font-mono text-[13px] h-9"
                  placeholder="e.g. npm run dev"
                  value={runCommand}
                  onChange={(e) => setRunCommand(e.target.value)}
                  onBlur={saveProjectSettings}
                />
              </FormField>
            </div>

            <SectionGroup>
              <SubsectionHeader
                title="Environment variables"
                description="Available to all scripts above. Reference them with $NAME in commands."
              />
              <SettingsCard className="divide-y divide-border/40 p-0">
                {ENV_VARS.map(([name, desc]) => (
                  <div
                    key={name}
                    className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)] items-center gap-4 px-4 py-2"
                  >
                    <code className="font-mono text-[12px] text-foreground/90 truncate">{name}</code>
                    <span className="text-[12px] text-muted-foreground/85 truncate">{desc}</span>
                  </div>
                ))}
              </SettingsCard>
            </SectionGroup>
          </div>
        );
      }

      case "notifications":
        return (
          <div>
            <SectionHeader
              title="Notifications"
              description="Control how Codemux notifies you about events."
            />
            <div className="space-y-1">
              <SettingRow
                label="Notification sounds"
                description="Play a sound when an agent finishes or needs attention."
              >
                <Switch
                  checked={config?.notification_sound_enabled ?? false}
                  onCheckedChange={(checked) => {
                    setNotificationSoundEnabled(checked).catch(console.error);
                    updateSyncedSetting("notifications", "sound_enabled", checked).catch(console.error);
                  }}
                />
              </SettingRow>
              <Separator />
              <SettingRow
                label="Desktop notifications"
                description="Show system notifications via D-Bus when events occur."
              >
                <Switch
                  checked={syncedSettings.notifications.desktop_enabled}
                  onCheckedChange={(checked) => {
                    // TODO: wire to actual desktop notification system when implemented
                    updateSyncedSetting("notifications", "desktop_enabled", checked).catch(console.error);
                  }}
                />
              </SettingRow>
            </div>
          </div>
        );

      case "session_restore":
        return (
          <div>
            <SectionHeader
              title="Session Restore"
              description="Restore terminal scrollback and agent sessions when reopening Codemux."
            />
            <div className="space-y-1">
              <SettingRow
                label="Enable session restore"
                description="Save and restore terminal scrollback across app restarts."
              >
                <Switch
                  checked={syncedSettings.session_restore.enabled}
                  onCheckedChange={(checked) => {
                    updateSyncedSetting("session_restore", "enabled", checked).catch(console.error);
                  }}
                />
              </SettingRow>
              <Separator />
              <SettingRow
                label="Scrollback lines"
                description={`${syncedSettings.session_restore.scrollback_lines.toLocaleString()} lines saved per terminal pane.`}
              >
                <Slider
                  value={[syncedSettings.session_restore.scrollback_lines]}
                  onValueChange={([v]) => {
                    updateSyncedSetting("session_restore", "scrollback_lines", v).catch(console.error);
                  }}
                  min={1000}
                  max={50000}
                  step={1000}
                  className="w-36"
                />
              </SettingRow>
              <Separator />
              <SettingRow
                label="Max disk usage"
                description={`${syncedSettings.session_restore.max_total_mb} MB maximum for all saved scrollback.`}
              >
                <Slider
                  value={[syncedSettings.session_restore.max_total_mb]}
                  onValueChange={([v]) => {
                    updateSyncedSetting("session_restore", "max_total_mb", v).catch(console.error);
                  }}
                  min={10}
                  max={500}
                  step={10}
                  className="w-36"
                />
              </SettingRow>
            </div>
          </div>
        );
    }
  };

  // Resolve the human-readable label for the active nav row so the
  // header can show "Settings › Section" — a quiet breadcrumb that
  // tells the user where they are without taking visual weight away
  // from the actual content.
  const activeLabel = navGroups
    .flatMap((g) => g.items)
    .find((i) => i.id === activeSection)?.label ?? null;

  return (
    <div className="relative flex h-screen flex-col bg-background">
      <WindowChrome />
      {/* Header. `WindowChrome` is an absolute 28px drag-region strip
          with z-50 that overlays the top of the page; if header content
          sits in y=0..28 the drag region eats its clicks. We give the
          header `pt-7` (= 28px) so the back button and breadcrumb are
          rendered ENTIRELY below the drag strip and their full hit area
          stays clickable. The remaining `pb-2` keeps the bar visually
          tight without growing the chrome unnecessarily. */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 pt-7 pb-2">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Close settings"
          className="text-muted-foreground hover:text-foreground hover:bg-muted/50"
          onClick={() => setShowSettings(false)}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="flex items-center gap-2 text-[14px]">
          <span className="font-semibold tracking-tight text-foreground">Settings</span>
          {activeLabel && (
            <>
              <span className="text-muted-foreground/40">/</span>
              <span className="font-semibold text-muted-foreground">{activeLabel}</span>
            </>
          )}
        </div>
      </div>

      {/* Body */}
      <div className="flex flex-1 min-h-0">
        {/* Left nav — refined-minimal pill rows matching the new
            sidebar aesthetic: inset margins, soft muted hover, calm
            type hierarchy. Group separation is whitespace alone (no
            dividers) so the nav reads as one continuous list. Mono
            group captions echo the design system's metadata voice. */}
        <nav className="w-60 shrink-0 border-r border-border bg-background py-4">
          <div className="space-y-5">
            {navGroups.map((group) => (
              <div key={group.label}>
                <p className="px-4 pb-1.5 font-mono text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground/55">
                  {group.label}
                </p>
                <div className="space-y-px px-3">
                  {group.items.map((item) => (
                    <SettingsNavItem
                      key={item.id}
                      icon={item.icon}
                      label={item.label}
                      active={activeSection === item.id}
                      onClick={() => setActiveSection(item.id)}
                    />
                  ))}
                </div>
              </div>
            ))}
          </div>
        </nav>

        {/* Content */}
        <ScrollArea className="flex-1 bg-card">
          <div className="mx-auto max-w-3xl px-11 pt-8 pb-20">
            {renderSection()}
          </div>
        </ScrollArea>
      </div>
    </div>
  );
}

interface SortablePresetRowProps {
  preset: TerminalPreset;
  selected: boolean;
  onSelect: () => void;
  onTogglePin: () => void;
  onDelete: () => void;
}

function SortablePresetRow({
  preset,
  selected,
  onSelect,
  onTogglePin,
  onDelete,
}: SortablePresetRowProps) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: preset.id });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : undefined,
    zIndex: isDragging ? 1 : undefined,
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={cn(
        "group/preset flex items-center gap-3 pl-2 pr-2.5 py-2.5 border-b border-border/40 last:border-b-0 cursor-pointer transition-colors duration-150",
        selected ? "bg-muted/60" : "hover:bg-muted/30",
      )}
      onClick={onSelect}
    >
      <button
        type="button"
        className="p-1 rounded text-muted-foreground/30 hover:text-muted-foreground hover:bg-muted/60 cursor-grab active:cursor-grabbing touch-none opacity-0 group-hover/preset:opacity-100 transition-opacity"
        aria-label="Drag to reorder"
        title="Drag to reorder"
        onClick={(e) => e.stopPropagation()}
        {...attributes}
        {...listeners}
      >
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      {/* Glyph tile — the agent icon seated in a rounded tile, per the
          design's preset rows. */}
      <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-muted/60">
        <PresetIcon icon={preset.icon} className="h-3.5 w-3.5" />
      </span>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="text-[13px] font-medium truncate text-foreground">{preset.name}</span>
          {preset.is_builtin && (
            <Badge variant="secondary" className="text-[9px] px-1.5 py-0 h-4 font-normal uppercase tracking-wider">
              built-in
            </Badge>
          )}
        </div>
        {preset.commands.length > 0 && (
          <code className="text-[11px] text-muted-foreground/70 font-mono truncate block mt-0.5">
            {preset.commands[0]}
          </code>
        )}
      </div>
      <div className="flex items-center gap-0.5 shrink-0">
        {/* Delete sits to the LEFT of the pin so the pin stays anchored to
            the row's right edge — keeping it aligned across builtin rows
            (pin only) and custom rows (pin + delete). */}
        {!preset.is_builtin && (
          <Button
            variant="ghost"
            size="icon-xs"
            title="Delete preset"
            className="h-7 w-7 opacity-0 group-hover/preset:opacity-100 transition-opacity hover:bg-destructive/10 hover:text-destructive"
            onClick={(e) => {
              e.stopPropagation();
              onDelete();
            }}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon-xs"
          title={preset.pinned ? "Unpin from bar" : "Pin to bar"}
          className={cn(
            "h-7 w-7 transition-opacity",
            preset.pinned ? "opacity-100" : "opacity-60 group-hover/preset:opacity-100",
          )}
          onClick={(e) => {
            e.stopPropagation();
            onTogglePin();
          }}
        >
          {preset.pinned ? (
            <Star className="h-3.5 w-3.5 fill-current text-foreground" />
          ) : (
            <Star className="h-3.5 w-3.5 text-muted-foreground" />
          )}
        </Button>
      </div>
    </div>
  );
}
