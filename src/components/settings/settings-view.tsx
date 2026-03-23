import { useState, useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { ScrollArea } from "@/components/ui/scroll-area";
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
} from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { detectEditors, setNotificationSoundEnabled } from "@/tauri/commands";
import type { EditorInfo } from "@/tauri/types";

type Section = "appearance" | "editor" | "terminal" | "git" | "shortcuts" | "notifications";

const NAV_ITEMS: { id: Section; label: string; icon: React.ElementType }[] = [
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "editor", label: "Editor", icon: Code2 },
  { id: "terminal", label: "Terminal", icon: TerminalSquare },
  { id: "git", label: "Git", icon: GitBranch },
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

export function SettingsView() {
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const config = useAppStore((s) => s.appState?.config);
  const [activeSection, setActiveSection] = useState<Section>("appearance");
  const [editors, setEditors] = useState<EditorInfo[]>([]);
  const [defaultEditor, setDefaultEditor] = useState("");
  const [cursorStyle, setCursorStyle] = useState("bar");
  const [fontSize, setFontSize] = useState(13);
  const [baseBranch, setBaseBranch] = useState("main");
  const [terminalThemeMode, setTerminalThemeMode] = useState("app");

  useEffect(() => {
    detectEditors()
      .then((eds) => {
        setEditors(eds);
        if (eds.length > 0) setDefaultEditor(eds[0].id);
      })
      .catch(() => {});
  }, []);

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
                        {ed.name}
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
                          <span className="text-sm">{ed.name}</span>
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
                  onCheckedChange={(checked) =>
                    setNotificationSoundEnabled(checked).catch(console.error)
                  }
                />
              </SettingRow>
              <Separator />
              <SettingRow
                label="Desktop notifications"
                description="Show system notifications via D-Bus when events occur."
              >
                <Switch defaultChecked />
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
              <button
                key={item.id}
                className={`flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-sm transition-colors ${
                  activeSection === item.id
                    ? "bg-accent text-foreground font-medium"
                    : "text-muted-foreground hover:text-foreground hover:bg-accent/50"
                }`}
                onClick={() => setActiveSection(item.id)}
              >
                <Icon className="h-4 w-4 shrink-0" />
                {item.label}
              </button>
            );
          })}
        </nav>

        {/* Content */}
        <ScrollArea className="flex-1">
          <div className="max-w-2xl p-8">
            {renderSection()}
          </div>
        </ScrollArea>
      </div>
    </div>
  );
}
