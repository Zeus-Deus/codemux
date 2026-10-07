import { isRemoteClient } from "@/components/remote/is-remote-client";
import { buildNavGroups, type Section } from "./settings-sections";

/**
 * The searchable index behind the Settings search field.
 *
 * Settings renders one section at a time, so the rows of every other page are
 * not in the DOM to be searched. This list stands in for them: one entry per
 * row or subsection a person might look for, keyed by the text the page shows.
 * That label is also how navigation finds the row to scroll to, so it must
 * match the rendered text exactly; `SettingsPanel.test.tsx` checks every entry
 * against its page.
 */
export interface SettingsSearchEntry {
  section: Section;
  /** The row's visible label; also the scroll anchor. */
  label: string;
  /** Words people search with that the label does not contain. */
  keywords?: string;
  /** The page hides this row on the web remote client. */
  desktopOnly?: boolean;
}

/** Synonyms for whole pages, matched alongside the page name. */
const SECTION_KEYWORDS: Partial<Record<Section, string>> = {
  account: "profile sign in login user",
  appearance: "theme look style colors",
  notifications: "alerts sounds",
  shortcuts: "keyboard keybinds keybindings hotkeys bindings",
  agent: "ai claude codex models",
  interface: "agent chat gui classic cli mode",
  permissions: "allow deny ask tool rules approvals",
  skills: "skill sync import export",
  mcp: "model context protocol servers tools",
  usage: "cost tokens spend limits quota",
  projects: "scripts worktree repository",
  presets: "launcher quick launch agents",
  archive: "archived workspaces restore delete",
  session_restore: "scrollback history restart",
  editor: "ide vscode cursor zed open files",
  terminal: "shell console",
  browser: "web preview viewport cookies",
  git: "branch commit merge",
  source_control: "github gitlab forgejo gh glab pull request hosting",
  addons: "extensions plugins add-ons",
  hosts: "devices machines hosts ssh cloud remote",
  remote_access: "phone mobile pairing qr web browser access",
  about: "version update diagnostics logs reset help bug report",
};

export const SETTINGS_SEARCH_ENTRIES: readonly SettingsSearchEntry[] = [
  // Account
  { section: "account", label: "Email" },
  { section: "account", label: "Skills sync", keywords: "devices sync" },
  { section: "account", label: "Session", keywords: "sign out log out logout" },

  // Appearance
  { section: "appearance", label: "Typography", keywords: "font size interface zoom text" },
  { section: "appearance", label: "Developer font", keywords: "monospace code terminal font family" },
  { section: "appearance", label: "Resource monitor", keywords: "cpu memory title bar" },
  { section: "appearance", label: "Theme", keywords: "colors palette dark light custom studio" },
  { section: "appearance", label: "Density", keywords: "compact comfortable spacing padding" },
  { section: "appearance", label: "Wrap code in chat", keywords: "soft wrap code blocks lines" },
  { section: "appearance", label: "Sidebar", keywords: "workspace cards inbox" },
  { section: "appearance", label: "Show git stats", keywords: "ahead diff numbers branch" },
  { section: "appearance", label: "Auto-settle idle work", keywords: "settled sweep idle inbox" },
  { section: "appearance", label: "Match the orb to the activity", keywords: "orb animation indicator working agents" },

  // Notifications
  { section: "notifications", label: "Notification sounds", keywords: "sound audio chime" },
  { section: "notifications", label: "Desktop notifications", keywords: "alert popup" },

  // Agent
  { section: "agent", label: "Utility agent", keywords: "handoff model haiku luna cheap" },
  { section: "agent", label: "Default Hermes profile", keywords: "hermes acp" },
  { section: "agent", label: "Auto-configure MCP for workspaces", keywords: ".mcp.json mcp config" },
  { section: "agent", label: "Per-turn revert checkpoints", keywords: "checkpoints undo rewind snapshot" },
  { section: "agent", label: "Resume automatically after usage limits reset", keywords: "auto resume rate limit" },
  { section: "agent", label: "Desktop-size background browser", keywords: "viewport peek screenshot" },

  // Interface
  { section: "interface", label: "Agent Chat GUI", keywords: "classic cli terminal mode" },

  // Projects
  { section: "projects", label: "Worktree includes", keywords: ".codemuxinclude copy env files" },
  { section: "projects", label: "Setup", keywords: "setup script install bootstrap" },
  { section: "projects", label: "Teardown", keywords: "cleanup script" },
  { section: "projects", label: "Run", keywords: "dev server command start" },
  { section: "projects", label: "Environment variables", keywords: "env" },

  // Presets
  { section: "presets", label: "Show preset bar", keywords: "quick launch bar" },
  { section: "presets", label: "Your presets", keywords: "launcher agent command" },

  // Session Restore
  { section: "session_restore", label: "Enable session restore", keywords: "restore terminals restart" },
  { section: "session_restore", label: "Scrollback lines", keywords: "history buffer" },
  { section: "session_restore", label: "Max disk usage", keywords: "storage space size" },

  // Editor
  { section: "editor", label: "Default editor", keywords: "ide vscode cursor zed open files" },

  // Terminal
  { section: "terminal", label: "Font", keywords: "terminal font size family" },
  { section: "terminal", label: "Cursor style", keywords: "caret block bar underline" },
  { section: "terminal", label: "Color theme", keywords: "ansi colors omarchy" },

  // Browser
  { section: "browser", label: "Default viewport", keywords: "size resolution screen" },
  { section: "browser", label: "Profile storage", keywords: "cache size disk" },
  { section: "browser", label: "Clear cookies & site data", keywords: "cookies session storage" },
  { section: "browser", label: "Clear all browser data", keywords: "reset cache profile" },

  // Git
  { section: "git", label: "Default base branch", keywords: "main master branch" },
  { section: "git", label: "AI Tools" },
  { section: "git", label: "AI commit messages", keywords: "generate commit" },
  { section: "git", label: "Merge Conflict Resolver", keywords: "conflicts merge rebase" },

  // About
  { section: "about", label: "Version", keywords: "build channel release" },
  { section: "about", label: "Updates", keywords: "check update upgrade" },
  { section: "about", label: "Performance diagnostics", keywords: "perf debug report bug slow" },
  { section: "about", label: "Logs", keywords: "log file folder debug", desktopOnly: true },
  { section: "about", label: "Reset settings", keywords: "defaults restore factory", desktopOnly: true },
];

export interface SettingsSearchResult {
  section: Section;
  sectionLabel: string;
  /** The row to scroll to, or null for a result that is the page itself. */
  anchor: string | null;
  /** What the result row shows. */
  label: string;
  /** The synonym that matched when the label holds none of the query's
   *  words, so the result can say why it is listed. */
  matchedKeyword?: string;
}

export interface SettingsSearchGroup {
  section: Section;
  sectionLabel: string;
  results: SettingsSearchResult[];
}

function tokens(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

/** How well `label` answers the query: a prefix beats a word start beats a
 *  match that came only from keywords or the page name. */
function rank(label: string, needles: string[]): number {
  const text = label.toLowerCase();
  const first = needles[0];
  if (text.startsWith(first)) return 0;
  if (new RegExp(`\\b${escapeRegExp(first)}`).test(text)) return 1;
  if (text.includes(first)) return 2;
  return 3;
}

/** The first keyword holding one of the query's words, or undefined when the
 *  label already shows a match. */
function keywordHint(label: string, keywords: string, needles: string[]): string | undefined {
  const text = label.toLowerCase();
  if (needles.some((n) => text.includes(n))) return undefined;
  return keywords
    .split(/\s+/)
    .find((word) => needles.some((n) => word.toLowerCase().includes(n)));
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Settings rows and pages matching `query`, grouped by page.
 *
 * A result matches when every word of the query appears in its label, its
 * keywords, or its page's name and keywords — so "git stats" and "appearance
 * orb" both narrow the way they read. Pages hidden by the current flags never
 * appear, and neither do rows the web remote client does not render. Pages are ordered by their strongest result, then nav order, so the
 * first result is the one the query most plausibly names.
 */
export function searchSettings(
  query: string,
  agentChatEnabled: boolean,
): SettingsSearchGroup[] {
  const needles = tokens(query);
  if (needles.length === 0) return [];
  const score = (result: SettingsSearchResult) => rank(result.label, needles);
  const remote = isRemoteClient();

  const groups: SettingsSearchGroup[] = [];
  for (const item of buildNavGroups(agentChatEnabled).flatMap((g) => g.items)) {
    const sectionKeywords = SECTION_KEYWORDS[item.id] ?? "";
    const sectionText = `${item.label} ${sectionKeywords}`.toLowerCase();
    const pageMatches = needles.every((n) => sectionText.includes(n));
    const results: SettingsSearchResult[] = pageMatches
      ? [
          {
            section: item.id,
            sectionLabel: item.label,
            anchor: null,
            label: item.label,
            matchedKeyword: keywordHint(item.label, sectionKeywords, needles),
          },
        ]
      : [];

    for (const entry of SETTINGS_SEARCH_ENTRIES) {
      if (entry.section !== item.id) continue;
      if (remote && entry.desktopOnly) continue;
      const rowText = `${entry.label} ${entry.keywords ?? ""}`.toLowerCase();
      if (!needles.every((n) => rowText.includes(n) || sectionText.includes(n))) continue;
      // When the query names the page, list only the rows it also says
      // something about, not every row on the page.
      if (pageMatches && !needles.some((n) => rowText.includes(n))) continue;
      results.push({
        section: item.id,
        sectionLabel: item.label,
        anchor: entry.label,
        label: entry.label,
        matchedKeyword: keywordHint(entry.label, `${entry.keywords ?? ""} ${sectionKeywords}`, needles),
      });
    }

    if (results.length === 0) continue;
    // Stable sort: equal ranks keep the order the page renders them in.
    results.sort((a, b) => score(a) - score(b));
    groups.push({ section: item.id, sectionLabel: item.label, results });
  }
  // Stable sort again: equal pages keep nav order.
  return groups.sort((a, b) => score(a.results[0]) - score(b.results[0]));
}

/**
 * Settings pages the command palette offers for `needle`. Typing "settings"
 * lists every page and "settings git" narrows them; any other query of two or
 * more characters finds pages by name, synonym or the rows they hold
 * ("scrollback" → Session Restore), strongest first and capped so a short
 * query does not bury the palette's other groups.
 */
export function settingsPagesForQuery(
  needle: string,
  agentChatEnabled: boolean,
  cap = 5,
): Section[] {
  const typed = needle.trim().toLowerCase();
  const rest = typed.replace(/^settings?(\s+|$)/, "");
  if (rest !== typed) {
    return rest === ""
      ? buildNavGroups(agentChatEnabled).flatMap((g) => g.items.map((i) => i.id))
      : searchSettings(rest, agentChatEnabled).map((g) => g.section);
  }
  if (typed.length < 2) return [];
  return searchSettings(typed, agentChatEnabled)
    .slice(0, cap)
    .map((g) => g.section);
}

/** Splits `text` around case-insensitive occurrences of the query's words, for
 *  highlighting. Odd indexes are matches. */
export function splitHighlight(text: string, query: string): string[] {
  const needles = tokens(query)
    .map(escapeRegExp)
    .sort((a, b) => b.length - a.length);
  if (needles.length === 0) return [text];
  return text.split(new RegExp(`(${needles.join("|")})`, "gi"));
}
