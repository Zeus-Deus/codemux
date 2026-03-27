import { useState, useEffect } from "react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { ScrollArea } from "@/components/ui/scroll-area";
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
  Pin,
  PinOff,
  Trash2,
  X,
} from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import {
  useSettingsStore,
  selectTerminalFontSize,
  selectTerminalCursorStyle,
  selectTerminalColorTheme,
  selectDefaultEditor,
  selectDefaultBaseBranch,
} from "@/stores/settings-store";
import {
  detectEditors,
  setNotificationSoundEnabled,
  setAiCommitMessageEnabled,
  setAiCommitMessageModel,
  setAiResolverEnabled,
  setAiResolverCli,
  setAiResolverModel,
  setAiResolverStrategy,
  getProjectScripts,
  setProjectScripts,
  getWorkspaceConfig,
} from "@/tauri/commands";
import type { EditorInfo, PresetStoreSnapshot, TerminalPreset, LaunchMode } from "@/tauri/types";
import { EditorIcon } from "@/components/icons/editor-icon";
import { PresetIcon } from "@/components/icons/preset-icon";
import {
  getPresets,
  setPresetPinned,
  setPresetBarVisible,
  deletePreset,
  updatePreset,
} from "@/tauri/commands";
import { onPresetsChanged } from "@/tauri/events";

type Section = "appearance" | "editor" | "terminal" | "presets" | "projects" | "git" | "agent" | "shortcuts" | "notifications";

const NAV_ITEMS: { id: Section; label: string; icon: React.ElementType }[] = [
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "editor", label: "Editor", icon: Code2 },
  { id: "terminal", label: "Terminal", icon: TerminalSquare },
  { id: "presets", label: "Presets", icon: Zap },
  { id: "projects", label: "Projects", icon: FolderCog },
  { id: "git", label: "Git", icon: GitBranch },
  { id: "agent", label: "Agent", icon: Bot },
  { id: "shortcuts", label: "Shortcuts", icon: Keyboard },
  { id: "notifications", label: "Notifications", icon: Bell },
];

const SHORTCUTS = [
  { category: "General", items: [
    { action: "Command palette", keys: "Ctrl+K" },
    { action: "Toggle sidebar", keys: "Ctrl+B" },
  ]},
  { category: "Workspaces", items: [
    { action: "Next workspace", keys: "Ctrl+]" },
    { action: "Previous workspace", keys: "Ctrl+[" },
    { action: "Run dev command", keys: "Ctrl+Shift+G" },
  ]},
  { category: "Tabs", items: [
    { action: "New terminal tab", keys: "Ctrl+T" },
    { action: "Close tab", keys: "Ctrl+W" },
    { action: "Switch to tab 1–9", keys: "Ctrl+1–9" },
  ]},
  { category: "Panes", items: [
    { action: "Split pane right", keys: "Ctrl+Shift+D" },
    { action: "Close pane", keys: "Ctrl+Shift+W" },
    { action: "Focus next pane", keys: "Ctrl+Shift+J" },
    { action: "Focus previous pane", keys: "Ctrl+Shift+K" },
  ]},
  { category: "Terminal", items: [
    { action: "Copy selection", keys: "Ctrl+Shift+C" },
    { action: "Paste", keys: "Ctrl+Shift+V" },
    { action: "Backward kill word", keys: "Ctrl+Backspace" },
  ]},
];

function SettingRow({ label, description, children }: {
  label: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between py-4">
      <div className="space-y-1 pr-8">
        <p className="text-sm font-medium leading-none">{label}</p>
        {description && (
          <p className="text-sm text-muted-foreground">{description}</p>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

function SectionHeader({ title, description }: { title: string; description: string }) {
  return (
    <div className="mb-6">
      <h2 className="text-base font-semibold tracking-tight">{title}</h2>
      <p className="text-sm text-muted-foreground mt-1">{description}</p>
    </div>
  );
}

function PresetEditorSheet({
  preset,
  open,
  onOpenChange,
}: {
  preset: TerminalPreset | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [commands, setCommands] = useState<string[]>([""]);
  const [launchMode, setLaunchMode] = useState<LaunchMode>("new_tab");
  const [pinned, setPinned] = useState(true);
  const [autoRunOnWorkspace, setAutoRunOnWorkspace] = useState(false);
  const [autoRunOnNewTab, setAutoRunOnNewTab] = useState(false);
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
    setConfirmDelete(false);
  }, [preset]);

  if (!preset) return null;

  const save = (updates: Partial<{
    name: string;
    description: string | null;
    commands: string[];
    launchMode: LaunchMode;
  }>) => {
    updatePreset({
      id: preset.id,
      name: updates.name ?? name,
      description: updates.description !== undefined ? updates.description : (description || null),
      commands: updates.commands ?? commands.filter((c) => c.trim()),
      workingDirectory: preset.working_directory,
      launchMode: updates.launchMode ?? launchMode,
      icon: preset.icon,
    }).catch(console.error);
  };

  const handleDelete = () => {
    deletePreset(preset.id).catch(console.error);
    onOpenChange(false);
  };

  const handlePinnedChange = (checked: boolean) => {
    setPinned(checked);
    setPresetPinned(preset.id, checked).catch(console.error);
  };

  const handleCommandChange = (index: number, value: string) => {
    const next = [...commands];
    next[index] = value;
    setCommands(next);
  };

  const handleCommandBlur = () => {
    save({ commands: commands.filter((c) => c.trim()) });
  };

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
            <PresetIcon icon={preset.icon} className="h-4 w-4 shrink-0" />
            {preset.name}
          </SheetTitle>
          <SheetDescription>
            Configure commands, targeting, and launch options.
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
              onBlur={() => name !== preset.name && save({ name })}
              placeholder="e.g. Dev Server"
              className="h-9"
            />
          </div>

          {/* Description */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Description</label>
            <Input
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              onBlur={() => save({ description: description || null })}
              placeholder="Optional description"
              className="h-9"
            />
          </div>

          {/* Commands */}
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

            {/* Auto-run */}
            <div className="space-y-3">
              <label className="text-sm font-medium">Auto-run</label>
              <div className="space-y-4">
                <div className="flex items-start gap-3">
                  <Switch
                    checked={autoRunOnWorkspace}
                    onCheckedChange={(checked) => {
                      setAutoRunOnWorkspace(checked);
                      updatePreset({
                        id: preset.id,
                        name,
                        description: description || null,
                        commands: commands.filter((c) => c.trim()),
                        workingDirectory: preset.working_directory,
                        launchMode,
                        icon: preset.icon,
                        autoRunOnWorkspace: checked,
                        autoRunOnNewTab: autoRunOnNewTab,
                      }).catch(console.error);
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
                      updatePreset({
                        id: preset.id,
                        name,
                        description: description || null,
                        commands: commands.filter((c) => c.trim()),
                        workingDirectory: preset.working_directory,
                        launchMode,
                        icon: preset.icon,
                        autoRunOnWorkspace: autoRunOnWorkspace,
                        autoRunOnNewTab: checked,
                      }).catch(console.error);
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
          {!preset.is_builtin ? (
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
          <Button size="sm" onClick={() => onOpenChange(false)}>
            Done
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
  const defaultEditor = useSettingsStore(selectDefaultEditor);
  const cursorStyle = useSettingsStore(selectTerminalCursorStyle);
  const fontSize = useSettingsStore(selectTerminalFontSize);
  const baseBranch = useSettingsStore(selectDefaultBaseBranch);
  const terminalThemeMode = useSettingsStore(selectTerminalColorTheme);
  const autoMcpConfig = storeGet("auto_mcp_config") !== "false";

  const activeWorkspace = useAppStore((s) => {
    const st = s.appState;
    return st?.workspaces.find((w) => w.workspace_id === st.active_workspace_id);
  });
  const projectRoot = activeWorkspace?.project_root ?? null;
  const projectName = projectRoot ? projectRoot.split("/").pop() ?? "Project" : "Project";

  const initialSection = (settingsSection && NAV_ITEMS.some((n) => n.id === settingsSection) ? settingsSection : "appearance") as Section;
  const [activeSection, setActiveSection] = useState<Section>(initialSection);
  const [editors, setEditors] = useState<EditorInfo[]>([]);
  const [presetStore, setPresetStore] = useState<PresetStoreSnapshot | null>(null);
  const [selectedPresetId, setSelectedPresetId] = useState<string | null>(null);

  // Project scripts state
  const [setupScripts, setSetupScripts] = useState("");
  const [teardownScripts, setTeardownScripts] = useState("");
  const [runCommand, setRunCommand] = useState("");
  const [hasConfigFile, setHasConfigFile] = useState(false);

  const setDefaultEditor = (v: string) => storeSet("editor.default", v);
  const setCursorStyle = (v: string) => storeSet("terminal.cursor_style", v);
  const setFontSize = (v: number) => storeSet("terminal.font_size", String(v));
  const setBaseBranch = (v: string) => storeSet("git.default_base_branch", v);
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
      } else {
        setSetupScripts("");
        setTeardownScripts("");
        setRunCommand("");
      }
    }).catch(console.error);
    getWorkspaceConfig(projectRoot).then((config) => {
      setHasConfigFile(config !== null);
    }).catch(console.error);
  }, [activeSection, projectRoot]);

  const saveProjectScripts = (setup: string, teardown: string, run: string) => {
    if (!projectRoot) return;
    setProjectScripts(projectRoot, {
      setup: setup.trim() ? setup.trim().split("\n").filter((l) => l.trim()) : [],
      teardown: teardown.trim() ? teardown.trim().split("\n").filter((l) => l.trim()) : [],
      run: run.trim() || null,
    }).catch(console.error);
  };

  useEffect(() => {
    getPresets().then(setPresetStore).catch(console.error);
    const unlisten = onPresetsChanged((snapshot) => setPresetStore(snapshot));
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  useEffect(() => {
    detectEditors()
      .then((eds) => {
        setEditors(eds);
        if (eds.length > 0 && !defaultEditor) {
          storeSet("editor.default", eds[0].id);
        }
      })
      .catch(() => {});
  }, [defaultEditor, storeSet]);

  const renderSection = () => {
    switch (activeSection) {
      case "appearance":
        return (
          <div>
            <SectionHeader
              title="Appearance"
              description="Customize how Codemux looks. Theme changes apply immediately."
            />
            <div className="space-y-1">
              <SettingRow label="Theme preset" description="shadcn preset code used to generate the color system.">
                <Badge variant="secondary" className="font-mono text-xs px-3 py-1">b3kIbNYVW</Badge>
              </SettingRow>
              <Separator />
              <SettingRow label="Font family" description="Applied to the entire app shell and terminal.">
                <span className="text-sm text-muted-foreground">JetBrains Mono Variable</span>
              </SettingRow>
              <Separator />
              <SettingRow label="Border radius" description="Controls the roundness of all UI elements.">
                <span className="text-sm text-muted-foreground">0.45rem</span>
              </SettingRow>
            </div>
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
              {editors.length > 0 && (
                <>
                  <Separator />
                  <div className="py-4">
                    <p className="text-sm font-medium mb-3">Detected editors</p>
                    <div className="space-y-2">
                      {editors.map((ed) => (
                        <div key={ed.id} className="flex items-center justify-between">
                          <span className="flex items-center gap-2 text-sm">
                            <EditorIcon id={ed.id} className="h-4 w-4" />
                            {ed.name}
                          </span>
                          <code className="text-xs text-muted-foreground font-mono bg-muted px-2 py-0.5 rounded">
                            {ed.command}
                          </code>
                        </div>
                      ))}
                    </div>
                  </div>
                </>
              )}
            </div>
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
              title="Terminal Presets"
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
              <Separator />
              {presetStore ? (
                <div className="space-y-2 pt-2">
                  <p className="text-xs text-muted-foreground mb-3">Click a preset to edit details.</p>
                  {presetStore.presets.map((preset) => (
                    <div
                      key={preset.id}
                      className={`flex items-center gap-3 px-3 py-2 rounded-lg border cursor-pointer transition-colors duration-150 ${
                        selectedPresetId === preset.id
                          ? "border-primary/40 bg-primary/5"
                          : "border-border/50 bg-card/50 hover:bg-accent/30"
                      }`}
                      onClick={() => setSelectedPresetId(preset.id)}
                    >
                      <PresetIcon icon={preset.icon} className="h-5 w-5 shrink-0" />
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2">
                          <span className="text-sm font-medium truncate">{preset.name}</span>
                          {preset.is_builtin && (
                            <Badge variant="secondary" className="text-[10px] px-1.5 py-0">
                              built-in
                            </Badge>
                          )}
                        </div>
                        {preset.commands.length > 0 && (
                          <code className="text-xs text-muted-foreground font-mono truncate block mt-0.5">
                            {preset.commands[0]}
                          </code>
                        )}
                      </div>
                      <div className="flex items-center gap-1 shrink-0">
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          title={preset.pinned ? "Unpin from bar" : "Pin to bar"}
                          onClick={(e) => {
                            e.stopPropagation();
                            setPresetPinned(preset.id, !preset.pinned).catch(console.error);
                          }}
                        >
                          {preset.pinned ? (
                            <Pin className="h-3.5 w-3.5 text-foreground" />
                          ) : (
                            <PinOff className="h-3.5 w-3.5 text-muted-foreground" />
                          )}
                        </Button>
                        {!preset.is_builtin && (
                          <Button
                            variant="ghost"
                            size="icon-xs"
                            title="Delete preset"
                            className="hover:bg-destructive/80"
                            onClick={(e) => {
                              e.stopPropagation();
                              deletePreset(preset.id).catch(console.error);
                              if (selectedPresetId === preset.id) setSelectedPresetId(null);
                            }}
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </Button>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">Loading presets...</p>
              )}
            </div>

            {/* Editor sheet — renders via portal, not inline */}
            <PresetEditorSheet
              preset={selectedPreset}
              open={!!selectedPreset}
              onOpenChange={(open) => { if (!open) setSelectedPresetId(null); }}
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

            <div className="mt-8">
              <h3 className="text-sm font-medium mb-1">AI Tools</h3>
              <p className="text-xs text-muted-foreground mb-4">
                AI-assisted git workflows. Requires the Claude CLI.
              </p>
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
                <SettingRow label="Model override" description="Leave empty to use the Claude CLI default.">
                  <Input
                    value={config?.ai_commit_message_model ?? ""}
                    onChange={(e) => {
                      setAiCommitMessageModel(e.target.value || null).catch(console.error);
                      storeSet("ai_commit_message_model", e.target.value || "");
                    }}
                    placeholder="Default"
                    className="w-36 h-9"
                    disabled={!(config?.ai_commit_message_enabled ?? true)}
                  />
                </SettingRow>
              </div>
            </div>

            <div className="mt-8">
              <h3 className="text-sm font-medium mb-1">Merge Conflict Resolver</h3>
              <p className="text-xs text-muted-foreground mb-4">
                AI-powered merge conflict resolution. Creates a safe temp branch, resolves conflicts, then lets you review before applying.
              </p>
              <div className="space-y-1">
                <SettingRow label="Enable resolver" description="Show 'Resolve with AI' button in the conflicts section.">
                  <Switch
                    checked={config?.ai_resolver_enabled ?? false}
                    onCheckedChange={(checked) => {
                      setAiResolverEnabled(checked).catch(console.error);
                      storeSet("ai_resolver_enabled", String(checked));
                    }}
                  />
                </SettingRow>
                <SettingRow label="CLI tool" description="Which AI CLI to use for resolving conflicts.">
                  <Select
                    value={config?.ai_resolver_cli ?? "claude"}
                    onValueChange={(v) => {
                      setAiResolverCli(v).catch(console.error);
                      storeSet("ai_resolver_cli", v);
                    }}
                    disabled={!(config?.ai_resolver_enabled ?? false)}
                  >
                    <SelectTrigger className="w-36 h-9">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="claude">Claude Code</SelectItem>
                      <SelectItem value="codex">Codex</SelectItem>
                      <SelectItem value="opencode">OpenCode</SelectItem>
                    </SelectContent>
                  </Select>
                </SettingRow>
                <SettingRow label="Model override" description="Leave empty to use the CLI default.">
                  <Input
                    value={config?.ai_resolver_model ?? ""}
                    onChange={(e) => {
                      setAiResolverModel(e.target.value || null).catch(console.error);
                      storeSet("ai_resolver_model", e.target.value || "");
                    }}
                    placeholder="Default"
                    className="w-36 h-9"
                    disabled={!(config?.ai_resolver_enabled ?? false)}
                  />
                </SettingRow>
                <SettingRow label="Strategy" description="How the AI should approach conflict resolution.">
                  <Select
                    value={config?.ai_resolver_strategy ?? "smart_merge"}
                    onValueChange={(v) => {
                      setAiResolverStrategy(v).catch(console.error);
                      storeSet("ai_resolver_strategy", v);
                    }}
                    disabled={!(config?.ai_resolver_enabled ?? false)}
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
            </div>
          </div>
        );

      case "shortcuts":
        return (
          <div>
            <SectionHeader
              title="Keyboard Shortcuts"
              description="All available shortcuts. Custom keybinds coming in a future update."
            />
            <div className="space-y-6">
              {SHORTCUTS.map((group) => (
                <div key={group.category}>
                  <p className="text-xs font-medium uppercase tracking-wider text-muted-foreground mb-2">
                    {group.category}
                  </p>
                  <div className="space-y-0">
                    {group.items.map((item) => (
                      <div
                        key={item.action}
                        className="flex items-center justify-between py-2.5"
                      >
                        <span className="text-sm">{item.action}</span>
                        <kbd className="text-xs font-mono text-muted-foreground bg-muted px-2 py-1 rounded-md border border-border">
                          {item.keys}
                        </kbd>
                      </div>
                    ))}
                  </div>
                  <Separator className="mt-2" />
                </div>
              ))}
            </div>
          </div>
        );

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
            </div>
          </div>
        );

      case "projects":
        return (
          <div>
            <SectionHeader
              title="Scripts"
              description={`Automate your workspace lifecycle for ${projectName}. Changes are saved automatically.`}
            />
            {hasConfigFile && (
              <div className="mb-4 rounded-md border border-border bg-muted/50 px-4 py-3 text-sm text-muted-foreground">
                A <code className="font-mono text-xs bg-muted px-1 py-0.5 rounded">.codemux/config.json</code> file was found.
                File-based configuration takes precedence over these settings.
              </div>
            )}
            <div className="space-y-6">
              <div>
                <label className="text-sm font-medium">Setup</label>
                <p className="text-sm text-muted-foreground mb-2">
                  Runs when a new workspace is created. One command per line.
                </p>
                <Textarea
                  className="font-mono text-sm min-h-[80px]"
                  placeholder="e.g. npm install"
                  value={setupScripts}
                  onChange={(e) => setSetupScripts(e.target.value)}
                  onBlur={() => saveProjectScripts(setupScripts, teardownScripts, runCommand)}
                />
              </div>
              <div>
                <label className="text-sm font-medium">Teardown</label>
                <p className="text-sm text-muted-foreground mb-2">
                  Runs when a workspace is deleted. One command per line.
                </p>
                <Textarea
                  className="font-mono text-sm min-h-[80px]"
                  placeholder="e.g. docker compose down"
                  value={teardownScripts}
                  onChange={(e) => setTeardownScripts(e.target.value)}
                  onBlur={() => saveProjectScripts(setupScripts, teardownScripts, runCommand)}
                />
              </div>
              <div>
                <label className="text-sm font-medium">Run</label>
                <p className="text-sm text-muted-foreground mb-2">
                  A command to start your dev server, triggered via <kbd className="text-xs bg-muted px-1 py-0.5 rounded border border-border">Ctrl+Shift+G</kbd>.
                </p>
                <Input
                  className="font-mono text-sm"
                  placeholder="e.g. npm run dev"
                  value={runCommand}
                  onChange={(e) => setRunCommand(e.target.value)}
                  onBlur={() => saveProjectScripts(setupScripts, teardownScripts, runCommand)}
                />
              </div>
              <Separator />
              <div className="text-sm text-muted-foreground space-y-1">
                <p className="font-medium text-foreground">Environment variables</p>
                <p><code className="font-mono text-xs">$CODEMUX_ROOT_PATH</code> — main repo root</p>
                <p><code className="font-mono text-xs">$CODEMUX_WORKSPACE_PATH</code> — workspace/worktree directory</p>
                <p><code className="font-mono text-xs">$COMPOSE_PROJECT_NAME</code> — auto-set to project folder name</p>
                <p><code className="font-mono text-xs">$CODEMUX_WORKSPACE_NAME</code> — workspace title</p>
                <p><code className="font-mono text-xs">$CODEMUX_WORKSPACE_ID</code> — workspace ID</p>
              </div>
              <div className="rounded-md border border-border bg-muted/50 px-4 py-3 text-sm text-muted-foreground">
                <p className="font-medium text-foreground mb-1">Docker Compose with worktrees</p>
                <p>
                  Codemux automatically sets <code className="font-mono text-xs bg-muted px-1 py-0.5 rounded">COMPOSE_PROJECT_NAME</code> to
                  your project folder name so all worktrees share the same Docker containers and volumes.
                </p>
              </div>
            </div>
          </div>
        );

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
                    storeSet("notification_sound_enabled", String(checked));
                  }}
                />
              </SettingRow>
              <Separator />
              <SettingRow
                label="Desktop notifications"
                description="Show system notifications via D-Bus when events occur."
              >
                <Switch checked disabled aria-label="Desktop notifications (not yet implemented)" />
              </SettingRow>
            </div>
          </div>
        );
    }
  };

  return (
    <div className="flex h-screen flex-col bg-background">
      {/* Header */}
      <div className="flex h-12 shrink-0 items-center gap-3 border-b border-border px-4">
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={() => setShowSettings(false)}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <h1 className="text-sm font-semibold">Settings</h1>
      </div>

      {/* Body */}
      <div className="flex flex-1 min-h-0">
        {/* Left nav */}
        <nav className="w-52 shrink-0 border-r border-border p-3 space-y-1">
          {NAV_ITEMS.map((item) => {
            const Icon = item.icon;
            return (
              <Button
                key={item.id}
                variant="ghost"
                className={cn(
                  "w-full justify-start gap-2.5 px-3 py-2 h-auto text-sm",
                  activeSection === item.id
                    ? "bg-accent text-foreground font-medium"
                    : "text-muted-foreground hover:text-foreground hover:bg-accent/50",
                )}
                onClick={() => setActiveSection(item.id)}
              >
                <Icon className="h-4 w-4 shrink-0" />
                {item.label}
              </Button>
            );
          })}
        </nav>

        {/* Content */}
        <ScrollArea className="flex-1 bg-card">
          <div className="max-w-2xl p-8">
            {renderSection()}
          </div>
        </ScrollArea>
      </div>
    </div>
  );
}
