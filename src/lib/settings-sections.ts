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
  Server,
  Sparkles,
  MonitorSmartphone,
  GitPullRequest,
  ChartColumn,
} from "lucide-react";

export type Section =
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
  | "session_restore";

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
export function buildNavGroups(agentChatEnabled: boolean): NavGroup[] {
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
          // Usage sits with the other chat-only rows: it reads the
          // agent-chat usage ledger, which only fills when the GUI is on.
          { id: "usage", label: "Usage", icon: ChartColumn },
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

export const SETTINGS_SECTIONS = buildNavGroups(true).flatMap(
  (group) => group.items,
);

export function isSettingsSection(id: string): id is Section {
  return SETTINGS_SECTIONS.some((section) => section.id === id);
}

export function isSettingsSectionAvailable(
  id: string,
  agentChatEnabled: boolean,
): id is Section {
  return buildNavGroups(agentChatEnabled).some((group) =>
    group.items.some((section) => section.id === id),
  );
}
