import {
  CalendarClock,
  MonitorSmartphone,
  GitPullRequest,
  Plug,
  Settings,
  Palette,
  Terminal,
  Star,
  Code2,
  Bot,
  Zap,
  Globe,
  Keyboard,
  Bell,
  Folder,
  BookOpen,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import {
  SETTINGS_SECTIONS,
  isSettingsSectionAvailable,
  type Section,
} from "./settings-sections";

export type FooterActionId =
  | `codemux.settings.${Section}`
  | "codemux.automations.open"
  | "codemux.devices.open"
  | "codemux.pull-requests.open"
  | "codemux.ports.open";
export interface FooterAction {
  id: FooterActionId;
  label: string;
  icon: LucideIcon;
  section?: Section;
}

// React render adapters stay internal to the footer. Only IDs enter preferences.
export const FOOTER_ACTIONS: readonly FooterAction[] = [
  { id: "codemux.automations.open", label: "Automations", icon: CalendarClock },
  { id: "codemux.devices.open", label: "Devices", icon: MonitorSmartphone },
  {
    id: "codemux.pull-requests.open",
    label: "Pull requests",
    icon: GitPullRequest,
  },
  { id: "codemux.ports.open", label: "Ports", icon: Plug },
  ...SETTINGS_SECTIONS.map((section) => ({
    id: `codemux.settings.${section.id}` as const,
    label: `Settings · ${section.label}`,
    icon: section.icon,
    section: section.id,
  })),
];
export const FOOTER_ICONS = {
  settings: Settings,
  palette: Palette,
  terminal: Terminal,
  star: Star,
  code: Code2,
  agent: Bot,
  lightning: Zap,
  globe: Globe,
  keyboard: Keyboard,
  bell: Bell,
  folder: Folder,
  book: BookOpen,
  automations: CalendarClock,
  devices: MonitorSmartphone,
  "pull-request": GitPullRequest,
  ports: Plug,
} satisfies Record<string, LucideIcon>;
export type FooterIconId = keyof typeof FOOTER_ICONS;
export function isFooterIconId(id: string): id is FooterIconId {
  return Object.prototype.hasOwnProperty.call(FOOTER_ICONS, id);
}
export function getFooterAction(id: string) {
  return FOOTER_ACTIONS.find((action) => action.id === id);
}
export function isFooterActionAvailable(
  action: FooterAction,
  agentChatEnabled: boolean,
  hasDevices: boolean,
) {
  if (action.id === "codemux.devices.open") return hasDevices;
  return (
    !action.section ||
    isSettingsSectionAvailable(action.section, agentChatEnabled)
  );
}
