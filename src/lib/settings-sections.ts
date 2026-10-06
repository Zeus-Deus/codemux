import type { LucideIcon } from "lucide-react";
import {
  Archive,
  Puzzle,
  Palette,
  Code2,
  TerminalSquare,
  GitBranch,
  Keyboard,
  Bell,
  Bot,
  Zap,
  FolderCog,
  UserCircle,
  Globe,
  RotateCcw,
  ShieldCheck,
  BookOpen,
  Info,
  Laptop,
  Plug,
  Sparkles,
  MonitorSmartphone,
  GitPullRequest,
  ChartColumn,
} from "lucide-react";

export type Section =
  | "addons"
  | "usage"
  | "interface"
  | "account"
  | "appearance"
  | "editor"
  | "terminal"
  | "presets"
  | "projects"
  | "archive"
  | "git"
  | "source_control"
  | "agent"
  | "permissions"
  | "skills"
  | "mcp"
  | "hosts"
  | "remote_access"
  | "browser"
  | "shortcuts"
  | "notifications"
  | "session_restore"
  | "about";

interface NavItem {
  id: Section;
  label: string;
  icon: LucideIcon;
}
interface NavGroup {
  label: string;
  items: NavItem[];
}

/** Build the Settings nav groups for the current flag state.
 *
 *  Groups follow what a row is *about*, so a setting can be guessed from
 *  the nav without knowing Codemux internals: who you are and how the app
 *  looks (Personal), how agents behave (Agents), the lifecycle of
 *  workspaces (Workspaces), what you and agents work through (Tools), and
 *  machine-level plumbing (System).
 *
 *  - "Interface" (home of the Agent Chat GUI toggle) stays visible
 *    regardless of the flag so users in either mode can find their way
 *    back.
 *  - The chat-only rows (Usage, Permissions, Skills, MCP Servers) are only
 *    surfaced when the GUI is on; they reach into chat-only data. Hiding
 *    them when off matches the "feature absent, no work performed"
 *    promise of the master toggle.
 */
export function buildNavGroups(agentChatEnabled: boolean): NavGroup[] {
  return [
    {
      label: "PERSONAL",
      items: [
        { id: "account", label: "Account", icon: UserCircle },
        { id: "appearance", label: "Appearance", icon: Palette },
        { id: "notifications", label: "Notifications", icon: Bell },
        { id: "shortcuts", label: "Shortcuts", icon: Keyboard },
      ],
    },
    {
      label: "AGENTS",
      items: [
        { id: "agent", label: "Agent", icon: Bot },
        { id: "interface", label: "Interface", icon: Sparkles },
        ...(agentChatEnabled
          ? ([
              { id: "permissions", label: "Permissions", icon: ShieldCheck },
              { id: "skills", label: "Skills", icon: BookOpen },
              { id: "mcp", label: "MCP Servers", icon: Plug },
              // Reads the agent-chat usage ledger, which only fills when
              // the GUI is on.
              { id: "usage", label: "Usage", icon: ChartColumn },
            ] as NavItem[])
          : []),
      ],
    },
    {
      label: "WORKSPACES",
      items: [
        { id: "projects", label: "Projects", icon: FolderCog },
        { id: "presets", label: "Presets", icon: Zap },
        // The restore/delete surface for everything archived from the
        // sidebar.
        { id: "archive", label: "Archive", icon: Archive },
        { id: "session_restore", label: "Session Restore", icon: RotateCcw },
      ],
    },
    {
      label: "TOOLS",
      items: [
        { id: "editor", label: "Editor", icon: Code2 },
        { id: "terminal", label: "Terminal", icon: TerminalSquare },
        { id: "browser", label: "Browser", icon: Globe },
        { id: "git", label: "Git", icon: GitBranch },
        // Which hosting product each checkout talks to, and whether its CLI
        // is installed and signed in: the hosting half of Git.
        { id: "source_control", label: "Source Control", icon: GitPullRequest },
      ],
    },
    {
      label: "SYSTEM",
      items: [
        { id: "addons", label: "Add-ons", icon: Puzzle },
        // Devices and Remote Access are both about reaching this machine (or
        // its sessions) from somewhere else. Always visible: the daemon is
        // built in, and remote access is default-off behind its own toggle.
        { id: "hosts", label: "Devices", icon: Laptop },
        { id: "remote_access", label: "Remote Access", icon: MonitorSmartphone },
        { id: "about", label: "About", icon: Info },
      ],
    },
  ];
}

export const SETTINGS_SECTIONS = buildNavGroups(true).flatMap(
  (group) => group.items,
);

export function isSettingsSectionAvailable(
  id: string,
  agentChatEnabled: boolean,
): id is Section {
  return buildNavGroups(agentChatEnabled).some((group) =>
    group.items.some((section) => section.id === id),
  );
}
