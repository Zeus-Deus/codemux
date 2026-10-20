import type { LucideIcon } from "lucide-react";
import {
  Archive,
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
  CircleArrowUp,
  Workflow,
  Boxes,
  Blocks,
  SlidersHorizontal,
} from "lucide-react";

export type Section =
  | "plugins"
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
  | "workflows"
  | "providers"
  | "permissions"
  | "skills"
  | "mcp"
  | "hosts"
  | "remote_access"
  | "updates"
  | "browser"
  | "shortcuts"
  | "notifications"
  | "session_restore"
  | "about";

export interface SettingsSectionItem {
  id: Section;
  label: string;
  icon: LucideIcon;
}

export type SettingsPageId =
  | "general"
  | "appearance"
  | "agents"
  | "providers"
  | "usage"
  | "extensions"
  | "workspaces"
  | "tools"
  | "git"
  | "connections"
  | "system";

/** One row of the Settings nav. Its sections render stacked on one page, in
 *  this order, each still addressable by its `Section` id. */
export interface SettingsPage {
  id: SettingsPageId;
  label: string;
  icon: LucideIcon;
  /** Shown under the title of a page that stacks several sections. */
  description: string;
  sections: SettingsSectionItem[];
}

/** Build the Settings pages for the current flag state.
 *
 *  The nav lists few pages so it reads at a glance; each page holds the
 *  related sections that used to be pages of their own, all open. Section
 *  ids stay the unit of navigation: deep links, search results and footer
 *  pins name a section, and Settings opens its page and scrolls to it.
 *
 *  - "Chat interface" (home of the Agent Chat GUI toggle) stays visible
 *    regardless of the flag so users in either mode can find their way
 *    back.
 *  - The chat-only sections (Permissions, Workflows, Usage, Skills) are
 *    only surfaced when the GUI is on; they reach into chat-only data.
 *    Hiding them when off matches the "feature absent, no work performed"
 *    promise of the master toggle. A page left with no sections is
 *    dropped.
 */
export function buildSettingsPages(agentChatEnabled: boolean): SettingsPage[] {
  const chatOnly = (items: SettingsSectionItem[]) => (agentChatEnabled ? items : []);
  const pages: SettingsPage[] = [
    {
      id: "general",
      label: "General",
      icon: SlidersHorizontal,
      description: "Your account, notifications and keyboard shortcuts.",
      sections: [
        { id: "account", label: "Account", icon: UserCircle },
        { id: "notifications", label: "Notifications", icon: Bell },
        { id: "shortcuts", label: "Shortcuts", icon: Keyboard },
      ],
    },
    {
      id: "appearance",
      label: "Appearance",
      icon: Palette,
      description: "",
      sections: [{ id: "appearance", label: "Appearance", icon: Palette }],
    },
    {
      id: "agents",
      label: "Agents",
      icon: Bot,
      description: "How agent sessions look, how chats behave, and what agents may do.",
      sections: [
        { id: "interface", label: "Chat interface", icon: Sparkles },
        // How chats behave once an agent is chosen; Providers covers which
        // agents exist and their accounts.
        { id: "agent", label: "Models & behavior", icon: Bot },
        ...chatOnly([
          { id: "permissions", label: "Permissions", icon: ShieldCheck },
          // Workflows run chat-launched agents, so they need the GUI too.
          { id: "workflows", label: "Workflows", icon: Workflow },
        ]),
      ],
    },
    {
      id: "providers",
      label: "Providers",
      icon: Boxes,
      description: "",
      sections: [{ id: "providers", label: "Providers", icon: Boxes }],
    },
    {
      id: "usage",
      label: "Usage",
      icon: ChartColumn,
      description: "",
      // Reads the agent-chat usage ledger, which only fills when the GUI is
      // on.
      sections: chatOnly([{ id: "usage", label: "Usage", icon: ChartColumn }]),
    },
    {
      id: "extensions",
      label: "Extensions",
      icon: Blocks,
      description: "Skills, MCP servers and plugins that add to what agents and Codemux can do.",
      sections: [
        ...chatOnly([{ id: "skills", label: "Skills", icon: BookOpen }]),
        // Inbound administration stays available in the CLI interface too.
        { id: "mcp", label: "MCP Servers", icon: Plug },
        { id: "plugins", label: "Plugins", icon: Blocks },
      ],
    },
    {
      id: "workspaces",
      label: "Workspaces",
      icon: FolderCog,
      description: "Project scripts, launch presets, and what happens to sessions over time.",
      sections: [
        { id: "projects", label: "Projects", icon: FolderCog },
        { id: "presets", label: "Presets", icon: Zap },
        { id: "session_restore", label: "Session Restore", icon: RotateCcw },
        // The restore/delete surface for everything archived from the
        // sidebar.
        { id: "archive", label: "Archive", icon: Archive },
      ],
    },
    {
      id: "tools",
      label: "Tools",
      icon: TerminalSquare,
      description: "The editor, terminal and browser you and your agents work through.",
      sections: [
        { id: "editor", label: "Editor", icon: Code2 },
        { id: "terminal", label: "Terminal", icon: TerminalSquare },
        { id: "browser", label: "Browser", icon: Globe },
      ],
    },
    {
      id: "git",
      label: "Git",
      icon: GitBranch,
      description: "Branches and commits, and the hosting service each checkout talks to.",
      sections: [
        { id: "git", label: "Git", icon: GitBranch },
        // Which hosting product each checkout talks to, and whether its CLI
        // is installed and signed in: the hosting half of Git.
        { id: "source_control", label: "Source Control", icon: GitPullRequest },
      ],
    },
    {
      id: "connections",
      label: "Connections",
      icon: MonitorSmartphone,
      description: "Reach this machine and its sessions from other devices.",
      // Always visible: the daemon is built in, and remote access is
      // default-off behind its own toggle.
      sections: [
        { id: "hosts", label: "Devices", icon: Laptop },
        { id: "remote_access", label: "Remote Access", icon: MonitorSmartphone },
      ],
    },
    {
      id: "system",
      label: "System",
      icon: Info,
      description: "Updates for Codemux and every coding agent, and details about this build.",
      sections: [
        // Codemux and every coding agent, here and on each device.
        { id: "updates", label: "Updates", icon: CircleArrowUp },
        { id: "about", label: "About", icon: Info },
      ],
    },
  ];
  return pages.filter((page) => page.sections.length > 0);
}

/** Every available section, in the order Settings shows them. */
export function settingsSectionsFor(agentChatEnabled: boolean): SettingsSectionItem[] {
  return buildSettingsPages(agentChatEnabled).flatMap((page) => page.sections);
}

export const SETTINGS_SECTIONS = settingsSectionsFor(true);

/** The page that shows `section`, or undefined when the flags hide it. */
export function settingsPageFor(
  section: string,
  agentChatEnabled: boolean,
): SettingsPage | undefined {
  return buildSettingsPages(agentChatEnabled).find((page) =>
    page.sections.some((item) => item.id === section),
  );
}

export function isSettingsSectionAvailable(
  id: string,
  agentChatEnabled: boolean,
): id is Section {
  return settingsPageFor(id, agentChatEnabled) !== undefined;
}
