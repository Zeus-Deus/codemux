import { useState, useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ArrowLeft } from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { detectEditors, setNotificationSoundEnabled } from "@/tauri/commands";
import type { EditorInfo } from "@/tauri/types";

type Section = "appearance" | "editor" | "terminal" | "git" | "shortcuts" | "notifications";

const NAV_ITEMS: { id: Section; label: string }[] = [
  { id: "appearance", label: "Appearance" },
  { id: "editor", label: "Editor" },
  { id: "terminal", label: "Terminal" },
  { id: "git", label: "Git" },
  { id: "shortcuts", label: "Shortcuts" },
  { id: "notifications", label: "Notifications" },
];

const SHORTCUTS = [
  { category: "General", items: [
    { action: "Command palette", keys: "Ctrl+K" },
    { action: "Toggle sidebar", keys: "Ctrl+B" },
    { action: "Open settings", keys: "—" },
  ]},
  { category: "Workspaces", items: [
    { action: "Next workspace", keys: "Ctrl+]" },
    { action: "Previous workspace", keys: "Ctrl+[" },
  ]},
  { category: "Tabs", items: [
    { action: "New terminal tab", keys: "Ctrl+T" },
    { action: "Close tab", keys: "Ctrl+W" },
    { action: "Switch to tab 1-9", keys: "Ctrl+1–9" },
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

function SectionHeading({ children }: { children: React.ReactNode }) {
  return <h3 className="text-sm font-medium text-foreground mb-3">{children}</h3>;
}

function SettingRow({ label, description, children }: {
  label: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between py-2">
      <div className="space-y-0.5">
        <Label className="text-sm">{label}</Label>
        {description && <p className="text-xs text-muted-foreground">{description}</p>}
      </div>
      <div className="shrink-0 ml-4">{children}</div>
    </div>
  );
}

export function SettingsView() {
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const config = useAppStore((s) => s.appState?.config);
  const [activeSection, setActiveSection] = useState<Section>("appearance");
  const [editors, setEditors] = useState<EditorInfo[]>([]);
  const [defaultEditor, setDefaultEditor] = useState<string>("");
  const [cursorStyle, setCursorStyle] = useState("bar");
  const [fontSize, setFontSize] = useState("13");
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

  const handleClose = () => setShowSettings(false);

  const renderSection = () => {
    switch (activeSection) {
      case "appearance":
        return (
          <div className="space-y-4">
            <SectionHeading>Appearance</SectionHeading>
            <SettingRow label="Theme preset" description="shadcn preset code">
              <Badge variant="secondary" className="font-mono text-xs">b3kIbNYVW</Badge>
            </SettingRow>
            <Separator />
            <SettingRow label="Font" description="JetBrains Mono Variable (monospace)">
              <Badge variant="outline" className="text-xs">Built-in</Badge>
            </SettingRow>
            <SettingRow label="Border radius" description="From preset">
              <span className="text-xs text-muted-foreground">0.45rem</span>
            </SettingRow>
          </div>
        );

      case "editor":
        return (
          <div className="space-y-4">
            <SectionHeading>Editor</SectionHeading>
            <SettingRow label="Default editor" description="Used when opening files from the file tree">
              <Select value={defaultEditor} onValueChange={setDefaultEditor}>
                <SelectTrigger className="w-48 h-8 text-xs">
                  <SelectValue placeholder="Select editor" />
                </SelectTrigger>
                <SelectContent>
                  {editors.map((ed) => (
                    <SelectItem key={ed.id} value={ed.id} className="text-xs">
                      {ed.name}
                    </SelectItem>
                  ))}
                  {editors.length === 0 && (
                    <SelectItem value="none" disabled className="text-xs">
                      No editors detected
                    </SelectItem>
                  )}
                </SelectContent>
              </Select>
            </SettingRow>
            {editors.length > 0 && (
              <>
                <Separator />
                <div className="space-y-1">
                  <Label className="text-xs text-muted-foreground">Detected editors</Label>
                  {editors.map((ed) => (
                    <div key={ed.id} className="flex items-center justify-between py-1">
                      <span className="text-xs">{ed.name}</span>
                      <span className="text-[10px] text-muted-foreground font-mono">{ed.command}</span>
                    </div>
                  ))}
                </div>
              </>
            )}
          </div>
        );

      case "terminal":
        return (
          <div className="space-y-4">
            <SectionHeading>Terminal</SectionHeading>
            <SettingRow label="Cursor style">
              <Select value={cursorStyle} onValueChange={setCursorStyle}>
                <SelectTrigger className="w-32 h-8 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="bar" className="text-xs">Bar</SelectItem>
                  <SelectItem value="block" className="text-xs">Block</SelectItem>
                  <SelectItem value="underline" className="text-xs">Underline</SelectItem>
                </SelectContent>
              </Select>
            </SettingRow>
            <SettingRow label="Font size">
              <Input
                type="number"
                min={10}
                max={24}
                value={fontSize}
                onChange={(e) => setFontSize(e.target.value)}
                className="w-20 h-8 text-xs"
              />
            </SettingRow>
            <Separator />
            <SettingRow label="Terminal theme" description="How the terminal gets its colors">
              <Select value={terminalThemeMode} onValueChange={setTerminalThemeMode}>
                <SelectTrigger className="w-40 h-8 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="app" className="text-xs">Match app theme</SelectItem>
                  <SelectItem value="system" className="text-xs">System (Omarchy)</SelectItem>
                </SelectContent>
              </Select>
            </SettingRow>
          </div>
        );

      case "git":
        return (
          <div className="space-y-4">
            <SectionHeading>Git</SectionHeading>
            <SettingRow label="Default base branch" description="Used when creating new branches">
              <Input
                value={baseBranch}
                onChange={(e) => setBaseBranch(e.target.value)}
                className="w-32 h-8 text-xs"
              />
            </SettingRow>
          </div>
        );

      case "shortcuts":
        return (
          <div className="space-y-4">
            <SectionHeading>Keyboard Shortcuts</SectionHeading>
            <p className="text-xs text-muted-foreground mb-3">
              Read-only for now. Custom keybinds coming later.
            </p>
            {SHORTCUTS.map((group) => (
              <div key={group.category} className="space-y-1">
                <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                  {group.category}
                </span>
                {group.items.map((item) => (
                  <div
                    key={item.action}
                    className="flex items-center justify-between py-1 px-1"
                  >
                    <span className="text-xs">{item.action}</span>
                    <kbd className="text-[10px] font-mono text-muted-foreground bg-muted px-1.5 py-0.5 rounded">
                      {item.keys}
                    </kbd>
                  </div>
                ))}
                <Separator className="my-2" />
              </div>
            ))}
          </div>
        );

      case "notifications":
        return (
          <div className="space-y-4">
            <SectionHeading>Notifications</SectionHeading>
            <SettingRow label="Notification sounds" description="Play sound when agent needs attention">
              <Switch
                checked={config?.notification_sound_enabled ?? false}
                onCheckedChange={(checked) =>
                  setNotificationSoundEnabled(checked).catch(console.error)
                }
              />
            </SettingRow>
            <SettingRow label="Desktop notifications" description="Show system notifications via D-Bus">
              <Switch defaultChecked />
            </SettingRow>
          </div>
        );
    }
  };

  return (
    <div className="flex h-full flex-col bg-background">
      {/* Header */}
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-3">
        <Button variant="ghost" size="icon-xs" onClick={handleClose}>
          <ArrowLeft className="h-3.5 w-3.5" />
        </Button>
        <span className="text-sm font-medium">Settings</span>
      </div>

      {/* Body */}
      <div className="flex flex-1 min-h-0">
        {/* Left nav */}
        <nav className="w-44 shrink-0 border-r border-border p-2 space-y-0.5">
          {NAV_ITEMS.map((item) => (
            <button
              key={item.id}
              className={`w-full rounded-md px-2.5 py-1.5 text-left text-xs transition-colors ${
                activeSection === item.id
                  ? "bg-accent text-foreground"
                  : "text-muted-foreground hover:text-foreground hover:bg-accent/50"
              }`}
              onClick={() => setActiveSection(item.id)}
            >
              {item.label}
            </button>
          ))}
        </nav>

        {/* Content */}
        <ScrollArea className="flex-1">
          <div className="max-w-lg p-6">{renderSection()}</div>
        </ScrollArea>
      </div>
    </div>
  );
}
