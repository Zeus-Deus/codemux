import {
  Bug,
  CircleCheck,
  CircleDot,
  File as FileIcon,
  FolderOpen,
  GitMerge,
  GitPullRequest,
  GitPullRequestClosed,
  GitPullRequestDraft,
  Image as ImageIcon,
  ListTodo,
  MessageCircleQuestion,
  RotateCw,
  Server,
  Settings,
} from "lucide-react";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { basename } from "@/lib/path";
import { cn } from "@/lib/utils";
import { segmentDraftHighlight } from "@/lib/agent-chat/attachment-tokens";
import { buildSkillCommands } from "@/lib/agent-chat/skill-commands";
import {
  buildModeCommands,
  buildWorkflowCommand,
  filterCommandMenuItems,
  filterSlashItems,
  findMentionAtCursor,
  findSlashAtCursor,
  nextModeInCycle,
  parseMentionQuery,
  type MentionAnchor,
  type SlashAnchor,
  type SlashCommandItem,
} from "@/lib/agent-chat/slash-commands";
import type { Attachment, ChatMode } from "@/stores/agent-chat-store";
import {
  selectActiveSkills,
  useSkillsStore,
} from "@/stores/skills-store";
import {
  getGithubIssueByPath,
  getGithubPrByPath,
  listGithubIssuesByPath,
  listMcpServers,
  listProjectFiles,
  listProjectFolders,
  listPullRequests,
  MCP_CODEMUX_SELF_ID,
  pasteClipboardImage,
  type McpServerConfig,
} from "@/tauri/commands";
import { Switch } from "@/components/ui/switch";
import { useMcpRuntime } from "@/hooks/use-mcp-runtime";
import { useMcpStore } from "@/stores/mcp-store";
import { IssuePickerPanel } from "@/components/github/issue-picker";
import { PrPickerPanel } from "@/components/github/pr-picker";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  FileMatch,
  FolderMatch,
  GitHubIssue,
  PermissionModeOption,
  PullRequestInfo,
} from "@/tauri/types";

import { AttachmentChip } from "./AttachmentChip";
import { ComposerCommandMenu } from "./ComposerCommandMenu";
import { ComposerFooter } from "./ComposerFooter";
import { ModePill, type ActivePillMode } from "./pickers/ModePill";
import { SlashCommandPopup } from "./SlashCommandPopup";

const EMPTY_ATTACHMENTS: Attachment[] = [];
const EMPTY_FILE_MATCHES: FileMatch[] = [];
const EMPTY_FOLDER_MATCHES: FolderMatch[] = [];
const EMPTY_ISSUE_MATCHES: GitHubIssue[] = [];
const EMPTY_PR_MATCHES: PullRequestInfo[] = [];
/** Step 8 Stage 2 — debounce window for `listProjectFiles` so fast
 *  typing doesn't flood the backend during the popup's open lifetime. */
const MENTION_FETCH_DEBOUNCE_MS = 100;
/** How many file matches the `@` popup requests at most. The fuzzy
 *  matcher already pre-sorts by score, so the first 20 are always the
 *  highest-ranked relative to the query. */
const MENTION_FETCH_LIMIT = 20;
/** Step 8 Stage 3 — `+` popup browses alphabetical without live
 *  search (search lives on `@`). 30 entries fits the popup's max-h
 *  cap with room to scroll. */
const ATTACH_BROWSE_LIMIT = 30;
/** Step 8 Stage 7 — soft / hard caps for the staged-attachment list.
 *  The pane handlers reject adds beyond the hard cap with a toast;
 *  the composer surfaces the warning copy so the user knows where
 *  they sit relative to those thresholds. Mirrors the constants in
 *  AgentChatPane.tsx — kept in sync by hand because there's no other
 *  shared module owning these values. */
const ATTACHMENT_SOFT_LIMIT = 10;
const ATTACHMENT_HARD_LIMIT = 20;

/** Views the `+` popup can show. `main` lists categories +
 *  navigation nudges; `file` / `folder` list browsable rows that
 *  insert an inline `@<basename>` token on pick; `issue` and `pr`
 *  swap the row pipeline for a dedicated picker panel that owns
 *  search + state colours. */
type AttachSubmode = "main" | "file" | "folder" | "issue" | "pr" | "mcp";

interface Props {
  draft: string;
  cwd: string | null;
  provider: AgentChatProviderKind;
  model: string | null;
  permissionMode: string | null;
  effort: string | null;
  contextWindow: string | null;
  activeModel: ChatModelInfo | null;
  effortLabelMap: Record<string, string>;
  permissionModes: PermissionModeOption[] | null;
  ultrathinkInBodyText: boolean;
  streaming: boolean;
  /** True while THIS composer's send RPC is in flight (before the
   *  backend acks). Blocks submit to avoid a double-send, but — unlike
   *  `streaming` — does not block queueing a follow-up. Defaults false. */
  sending?: boolean;
  /** True when the thread's last run died without cleanly settling
   *  (issue #154). When set (and nothing is in flight) the composer
   *  strip shows a one-click "Continue run" chip. Defaults false. */
  interrupted?: boolean;
  /** Click handler for the "Continue run" chip. Sends the fixed text
   *  "Continue" through the normal send path. Required for the chip to
   *  render. */
  onContinueRun?: () => void;
  sessionReady: boolean;
  showProviderPicker: boolean;
  /** True on the pre-session draft surface (no live session yet). Only
   *  affects the default-mode placeholder copy: draft reads "Describe
   *  what you want the agent to do…", a live session reads "Reply or
   *  steer the agent…" (design D10). Mode-specific placeholders
   *  (plan/ask/debug) are unaffected. Defaults to false. */
  isDraft?: boolean;
  /** When set, overrides the computed textarea placeholder entirely.
   *  Used by the subagent drill-in to swap in "Steering goes to the
   *  orchestrator…" while the composer stays parent-bound. */
  placeholderOverride?: string;
  /** Composer-level Cursor-style mode pill. Swaps the placeholder,
   *  hides the permission picker, and toggles the mode selector
   *  (dropdown → pill). */
  mode: ChatMode;
  /** When set, renders a muted inline banner above the textarea. Used
   *  for draft-send retry affordance (§8). `null` hides the banner. */
  errorMessage?: string | null;
  /** When false, hides the Stop button even while streaming. The draft
   *  surface uses this during materialise because there is no live
   *  session to interrupt. Defaults to true. */
  showStopButton?: boolean;
  /** Optional replacement for the Zone 1 strip (the cwd / "Home"
   *  label above the textarea). The draft surface uses this to thread
   *  a project picker in when the draft target is Home. Omit
   *  (`undefined`) to keep the default cwd label; pass `null` to
   *  render nothing above the textarea (a running chat keeps its
   *  scope in the workspace context bar instead). */
  zone1Override?: React.ReactNode;
  /** Thread Scope redesign — optional slot rendered BELOW the composer
   *  card (inside the same max-w-[760px] column), under the footer.
   *  The draft surface uses this for `ThreadScopeRow` (location ·
   *  checkout · from-branch + the centered scope hint). `undefined`
   *  (the default) renders nothing — existing non-draft call sites are
   *  unaffected. */
  belowComposerSlot?: React.ReactNode;
  /** Step 8 Stage 1 — staged attachments rendered as a chip strip
   *  inside the composer card, above the textarea. Empty array hides
   *  the strip. Defaults to `[]` so existing call sites keep working
   *  without changes. */
  stagedAttachments?: Attachment[];
  /** Step 8 Stage 1 — chip removal callback. Required for chips to be
   *  interactive; if omitted, the X button is a no-op. */
  onRemoveAttachment?: (attachmentId: string) => void;
  /** Step 8 Stage 7 — PR-only: toggles the staged PR's `expandFullDiff`
   *  flag, swapping its resolved content from name-only to full diff
   *  (or vice-versa). The chip surfaces the affordance; this prop just
   *  forwards the click. Optional so non-PR call sites stay terse. */
  onToggleExpandPr?: (attachmentId: string) => void;
  /** Step 8 Stage 2 — invoked when the user picks a file from the `@`
   *  mention popup OR the `+ → File…` browser. The Composer inserts
   *  the inline `@<basename>` token; the parent stages the chip +
   *  drives the readFileForAttachment resolution. Optional so
   *  existing call sites keep compiling. */
  onAttachFile?: (match: FileMatch) => void;
  /** Step 8 Stage 3 — invoked when the user picks a folder from the
   *  `+ → Folder…` browser. Mirrors `onAttachFile` for folders. */
  onAttachFolder?: (match: FolderMatch) => void;
  /** Step 8 Stage 4 — invoked when the user picks an issue from the
   *  `+ → GitHub Issue…` browser OR an `@issue:` mention. The Composer
   *  has already inserted the inline `@#<number>` token; the parent
   *  stages the chip + drives the detail fetch. */
  onAttachIssue?: (issue: GitHubIssue) => void;
  /** Step 8 Stage 5 — invoked when the user picks a PR from the
   *  `+ → GitHub PR…` browser OR an `@pr:` mention. The Composer
   *  has already inserted the inline `@!<number>` token; the parent
   *  stages the chip + drives the detail + diff fetch. */
  onAttachPr?: (pr: PullRequestInfo) => void;
  /** Step 8 Stage 6 — invoked when the user attaches an image via
   *  paste, drag-drop, or the `+ → Image…` picker. Composer just
   *  forwards the raw File; the parent runs the allowlist check and
   *  drives the staging lifecycle (loading chip → resolved bytes). */
  onAttachImage?: (file: File) => void | Promise<void>;
  /** Step 8 Stage 6 — gates the image attach affordances. When false
   *  (or null while capability data is loading) paste/drop are still
   *  bound — the user just sees a disabled `+ → Image…` row with a
   *  "doesn't support images" hint. The parent decides this from
   *  `activeModel.supports_images`. */
  modelSupportsImages?: boolean;
  /** Step 8 Stage 4 — preflight result for `is_github_repo` on `cwd`.
   *  Drives:
   *   - `attach:issue` / `attach:pr` enable state in the `+` popup,
   *   - whether `@issue:` autocomplete fetches at all.
   *  `null` means "not yet known"; the parent runs the preflight on
   *  mount and patches this in. While `null`, the popup keeps the
   *  GitHub entries disabled so a slow preflight can't flash an
   *  enabled-then-disabled affordance. */
  isGithubRepo?: boolean | null;
  /** Step 8 Stage 4 — when the user is in a GitHub repo but `gh` is
   *  not authenticated, the popup footer surfaces the auth-recovery
   *  hint. `null` (or true) means we have a usable `gh`; `false` means
   *  the user needs to run `gh auth login` before the GitHub kinds
   *  will work. */
  ghAuthenticated?: boolean | null;
  onDraftChange: (draft: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  onProviderChange: (provider: AgentChatProviderKind) => void;
  onModelChange: (model: string) => void;
  onPermissionModeChange: (mode: string) => void;
  onEffortChange: (effort: string) => void;
  onContextWindowChange: (value: string) => void;
  onModeActivate: (mode: ActivePillMode) => void;
  onModeRemove: () => void;
}

const MAX_ROWS_APPROX_PX = 32 + 7 * 20; // ~8 rows

export function Composer({
  draft,
  cwd,
  provider,
  model,
  permissionMode,
  effort,
  contextWindow,
  activeModel,
  effortLabelMap,
  permissionModes,
  ultrathinkInBodyText,
  streaming,
  sending = false,
  interrupted = false,
  onContinueRun,
  sessionReady,
  showProviderPicker,
  isDraft = false,
  placeholderOverride,
  mode,
  errorMessage = null,
  showStopButton = true,
  zone1Override,
  belowComposerSlot,
  stagedAttachments = EMPTY_ATTACHMENTS,
  onRemoveAttachment,
  onToggleExpandPr,
  onAttachFile,
  onAttachFolder,
  onAttachIssue,
  onAttachPr,
  onAttachImage,
  modelSupportsImages = false,
  isGithubRepo = null,
  ghAuthenticated = null,
  onDraftChange,
  onSubmit,
  onStop,
  onProviderChange,
  onModelChange,
  onPermissionModeChange,
  onEffortChange,
  onContextWindowChange,
  onModeActivate,
  onModeRemove,
}: Props) {
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  // Mirror layer that paints the colored highlights behind the
  // transparent textarea. The textarea owns scroll position; the
  // mirror's `scrollTop` is slaved to it on every textarea scroll so
  // both layers stay aligned when the user types past `MAX_ROWS_APPROX_PX`.
  // Without this sync, long prompts surface a textarea scrollbar that
  // does nothing visible (textarea text is `text-transparent`; only the
  // mirror is painted, and a stale mirror clips the overflow).
  const mirrorRef = useRef<HTMLDivElement | null>(null);
  // Step 8 Stage 6 — hidden file input used by the `+ → Image…`
  // picker. We trigger `.click()` from the popup's onSelect; the
  // input's onChange forwards each picked File to onAttachImage.
  // Kept hidden so the visible affordance stays the popup row.
  const imageInputRef = useRef<HTMLInputElement | null>(null);

  // Auto-grow textarea up to ~8 rows.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const desired = Math.min(el.scrollHeight, MAX_ROWS_APPROX_PX);
    el.style.height = `${desired}px`;
    // Keep the mirror's scroll offset in step with the textarea after
    // any auto-grow recalculation. When the textarea shrinks (delete
    // a line near the bottom) its `scrollTop` snaps; the mirror needs
    // to follow so the painted text doesn't desync.
    const mirror = mirrorRef.current;
    if (mirror) mirror.scrollTop = el.scrollTop;
  }, [draft]);

  // Forward the textarea's scroll position to the mirror layer. We can't
  // attach `onScroll` declaratively in JSX inside the existing handlers
  // because the textarea is the scrolling element; the React event still
  // fires but we keep this in a stable callback so the JSX edit stays
  // small.
  const handleTextareaScroll = useCallback(() => {
    const ta = textareaRef.current;
    const mirror = mirrorRef.current;
    if (!ta || !mirror) return;
    mirror.scrollTop = ta.scrollTop;
  }, []);

  // ─── Slash command popup state ────────────────────────────────────
  // The popup is purely a discoverability surface: when the user types
  // `/` the popup opens; arrow keys move the highlight; Enter activates;
  // Esc closes leaving the typed `/` intact so the user can still type
  // a literal slash.
  const [slashAnchor, setSlashAnchor] = useState<SlashAnchor | null>(null);
  const [slashHighlighted, setSlashHighlighted] = useState<string | null>(null);
  // IME composition guard — slash detection must not fire mid-composition,
  // otherwise dead-key sequences for non-Latin keyboards trigger the
  // popup unexpectedly.
  const composingRef = useRef(false);

  // ─── Mention (@) popup state — Step 8 Stage 2 ────────────────────
  // Parallel to slashAnchor: opens when the textarea contains `@`
  // followed by a query at the cursor (start-of-line or after
  // whitespace). Files are fetched asynchronously via
  // `listProjectFiles`, debounced so fast typing doesn't flood the
  // backend. Only one of slash/mention can be open at a time —
  // `handleTextareaChange` enforces mutual exclusion.
  const [mentionAnchor, setMentionAnchor] = useState<MentionAnchor | null>(null);
  const [mentionHighlighted, setMentionHighlighted] = useState<string | null>(
    null,
  );
  const [fileMatches, setFileMatches] = useState<FileMatch[]>(EMPTY_FILE_MATCHES);
  /** Step 8 Stage 4 — `@issue:` mode. The mention popup decodes the
   *  category prefix on every keystroke; when it lands on `issue`
   *  these matches drive the popup rows instead of `fileMatches`. */
  const [mentionIssueMatches, setMentionIssueMatches] = useState<GitHubIssue[]>(
    EMPTY_ISSUE_MATCHES,
  );
  /** Stage 5 — `@pr:` mention popup. Mirrors the issue match state;
   *  numeric direct-fetch produces a single-row result. */
  const [mentionPrMatches, setMentionPrMatches] = useState<PullRequestInfo[]>(
    EMPTY_PR_MATCHES,
  );
  const mentionOpen = mentionAnchor !== null;
  const mentionQuery = mentionAnchor?.query ?? "";
  /** Decode the category prefix once per render so popup item
   *  derivation, fetch effects, and the keyboard handler all share
   *  the same view of "what is this query asking for". */
  const parsedMention = useMemo(
    () => parseMentionQuery(mentionQuery),
    [mentionQuery],
  );

  // ─── Attach (+) popup state — Step 8 Stage 3 ─────────────────────
  // Triggered by the `+` button in the footer (button-anchored, not
  // textarea-anchored). Submode pivots in-place inside the same
  // popup: main → file/folder → pick → close. Mutual exclusion with
  // slash + mention popups is enforced at the toggle point.
  const [attachOpen, setAttachOpen] = useState(false);
  const [attachSubmode, setAttachSubmode] = useState<AttachSubmode>("main");
  // Redesigned command menu — the search box is a real focused cmdk
  // input, so keyboard nav (arrows / Enter) is owned by cmdk and the
  // highlight is no longer parent-controlled. This holds the query the
  // parent filters `attachPopupItems` against so the visible list and
  // cmdk's navigable list stay identical.
  const [attachQuery, setAttachQuery] = useState("");
  const [attachFileMatches, setAttachFileMatches] = useState<FileMatch[]>(
    EMPTY_FILE_MATCHES,
  );
  const [attachFolderMatches, setAttachFolderMatches] = useState<FolderMatch[]>(
    EMPTY_FOLDER_MATCHES,
  );
  // (issue submode renders <IssuePickerPanel /> directly, which
  // owns its own list + loading + error state — no flat-row state
  // needed at the Composer level.)

  // MCP submode (Stage 4) — fetched lazily on submode entry. The
  // popup row uses `useMcpRuntime` for live status and the zustand
  // store for the toggle persistence; the config list itself is
  // refreshed each time the submode opens so toggles in another
  // session show up here.
  const [attachMcpServers, setAttachMcpServers] = useState<
    McpServerConfig[]
  >([]);
  const { runtimes: mcpRuntimes } = useMcpRuntime();
  const mcpDisabledIds = useMcpStore((s) => s.disabledIds);
  const mcpToggleDisabled = useMcpStore((s) => s.toggleDisabled);

  const modeCommands = useMemo(
    () => buildModeCommands({ activeMode: mode, onActivate: onModeActivate }),
    [mode, onModeActivate],
  );

  // `/workflow` — gated to the Claude provider (server-side runtime
  // support only exists there). Shared between the typed `/` popup and
  // the `+` menu so the gating logic lives in one place.
  const workflowCommand = useMemo(
    () => buildWorkflowCommand({ isClaude: provider === "claude" }),
    [provider],
  );

  // ─── Skills (Cursor-style inline tokens) ─────────────────────────
  // Lazy-load on first popup open. Picking a skill expands the typed
  // `/<query>` to the full `/<skill-name>` in the textarea — the slash
  // command stays as literal text, syntax-highlighted by the mirror
  // overlay below. At send time the parent parses the text against the
  // skills registry and injects matched skill bodies as a per-turn
  // prefix (no separate chip / staging state needed).
  // `selectActiveSkills` already filters out disabled ids — Composer
  // never sees disabled skills, so highlight + picker + send-time
  // injection all stay consistent.
  const skills = useSkillsStore(selectActiveSkills);
  const loadSkills = useSkillsStore((s) => s.loadSkills);
  const skillsLoading = useSkillsStore((s) => s.loading);
  const skillsLoaded = useSkillsStore((s) => s.loaded);
  const skillsError = useSkillsStore((s) => s.error);

  // The popup-side `onInvoke` handler is a no-op signal: the actual
  // textarea mutation happens inside `handleSlashSelect` based on the
  // item's id prefix. Modes still need their `onSelect` activator.
  const skillItems = useMemo(
    () => buildSkillCommands({ skills, onInvoke: () => {} }),
    [skills],
  );

  // Highlight segments for the mirror overlay. Recomputed on every
  // keystroke; cheap (one regex pass + map) for realistic draft sizes.
  // Step 8 Stage 2.1 — folds attachment tokens into the same segment
  // stream so `@filename` mentions render as inline chips alongside
  // `/skill-name` highlights.
  const highlightSegments = useMemo(
    () => segmentDraftHighlight(draft, skills, stagedAttachments),
    [draft, skills, stagedAttachments],
  );

  const allSlashItems = useMemo(
    () => [...modeCommands, workflowCommand, ...skillItems],
    [modeCommands, workflowCommand, skillItems],
  );

  // Surface skill-loading + skill-error in the popup footer so the user
  // never sees a silent empty `SKILLS` group on first open or a quiet
  // failure when the backend scan errors. Loading shows only while the
  // skills haven't loaded yet — refreshes after the first success
  // populate in the background without a UI hint.
  const slashPopupFooter = useMemo(() => {
    if (skillsError) {
      return { tone: "error" as const, message: `Skills: ${skillsError}` };
    }
    if (skillsLoading && !skillsLoaded) {
      return { tone: "muted" as const, message: "Loading skills…" };
    }
    return null;
  }, [skillsError, skillsLoading, skillsLoaded]);

  const filteredItems = useMemo(
    () => filterSlashItems(allSlashItems, slashAnchor?.query ?? ""),
    [allSlashItems, slashAnchor?.query],
  );

  // Keep highlighted item valid: if the current highlight no longer
  // matches the filter, jump to the first visible item. This also
  // initialises the highlight when the popup first opens.
  useEffect(() => {
    if (!slashAnchor) return;
    const visible = filteredItems.map((i) => i.id);
    if (slashHighlighted && visible.includes(slashHighlighted)) return;
    setSlashHighlighted(visible[0] ?? null);
  }, [slashAnchor, filteredItems, slashHighlighted]);

  // Popup visibility is driven by *anchor presence only*, not by
  // filter results. An empty filter shows the "No commands match"
  // empty state rather than unmounting; auto-close happens via
  // `findSlashAtCursor` returning null (cursor moved out of slash
  // context) or explicit Esc.
  const slashOpen = slashAnchor !== null;

  // First-open lazy load. The store guards against double-fetch via TTL +
  // in-flight loading flag, so re-firing this effect on every open is
  // harmless and keeps the popup snappy after the initial scan.
  useEffect(() => {
    if (!slashOpen) return;
    void loadSkills(cwd ?? null);
  }, [slashOpen, cwd, loadSkills]);

  // ─── Mention popup: debounced file fetch ─────────────────────────
  // Fires on every query change while the popup is open. The 100ms
  // debounce is short enough for the popup to feel live while still
  // collapsing burst typing to a single backend roundtrip. When `cwd`
  // is null (Home draft, no project anchored), don't fetch — the
  // footer note tells the user to anchor a project first.
  // Stage 4 — only fires for the default `file` category. Issue/PR
  // categories route to their own fetch effects so they don't fight
  // over `fileMatches`.
  useEffect(() => {
    if (!mentionOpen) return;
    if (parsedMention.category !== "file") return;
    if (!cwd) {
      setFileMatches(EMPTY_FILE_MATCHES);
      return;
    }
    let cancelled = false;
    const trimmed = parsedMention.filter.trim();
    const timer = setTimeout(async () => {
      try {
        const matches = await listProjectFiles(
          cwd,
          trimmed.length > 0 ? trimmed : null,
          MENTION_FETCH_LIMIT,
        );
        if (!cancelled) setFileMatches(matches);
      } catch {
        // Backend errors (missing project, transient I/O, etc.) just
        // empty the popup. The "No matches" empty state is good enough
        // signal — no need to surface the raw error to the user.
        if (!cancelled) setFileMatches(EMPTY_FILE_MATCHES);
      }
    }, MENTION_FETCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [mentionOpen, parsedMention.category, parsedMention.filter, cwd]);

  // ─── Mention popup: `@issue:<query>` fetch ───────────────────────
  // Decoupled from the file fetch so we can branch on numeric query
  // (direct fetch by number) vs text (gh search). Same 100ms debounce
  // collapse so fast typing doesn't flood gh; cancellation flag
  // prevents stale roundtrips from clobbering newer ones.
  useEffect(() => {
    if (!mentionOpen) return;
    if (parsedMention.category !== "issue") return;
    if (!cwd || isGithubRepo === false) {
      setMentionIssueMatches(EMPTY_ISSUE_MATCHES);
      return;
    }
    if (ghAuthenticated === false) {
      setMentionIssueMatches(EMPTY_ISSUE_MATCHES);
      return;
    }
    let cancelled = false;
    const filter = parsedMention.filter.trim();
    const timer = setTimeout(async () => {
      try {
        // Numeric filter → direct single-issue fetch. We surface the
        // result as a one-row popup so the user can confirm before
        // attaching, even when the issue isn't in the open list.
        if (/^\d+$/.test(filter)) {
          const num = Number.parseInt(filter, 10);
          const issue = await getGithubIssueByPath(cwd, num);
          if (!cancelled) setMentionIssueMatches([issue]);
          return;
        }
        const matches = await listGithubIssuesByPath(
          cwd,
          filter.length > 0 ? filter : undefined,
        );
        if (!cancelled) setMentionIssueMatches(matches);
      } catch {
        if (!cancelled) setMentionIssueMatches(EMPTY_ISSUE_MATCHES);
      }
    }, MENTION_FETCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [
    mentionOpen,
    parsedMention.category,
    parsedMention.filter,
    cwd,
    isGithubRepo,
    ghAuthenticated,
  ]);

  // ─── Mention popup: `@pr:<query>` fetch ──────────────────────────
  // Stage 5 mirror of the issue branch. Numeric → direct PR fetch;
  // text → fuzzy filter against the open-PR list (gh `pr list`
  // doesn't have a `--search` flag, so server-side text search isn't
  // available — the local filter on titles is the best we can do).
  useEffect(() => {
    if (!mentionOpen) return;
    if (parsedMention.category !== "pr") return;
    if (!cwd || isGithubRepo === false) {
      setMentionPrMatches(EMPTY_PR_MATCHES);
      return;
    }
    if (ghAuthenticated === false) {
      setMentionPrMatches(EMPTY_PR_MATCHES);
      return;
    }
    let cancelled = false;
    const filter = parsedMention.filter.trim();
    const timer = setTimeout(async () => {
      try {
        if (/^\d+$/.test(filter)) {
          const num = Number.parseInt(filter, 10);
          const pr = await getGithubPrByPath(cwd, num);
          if (!cancelled) setMentionPrMatches([pr]);
          return;
        }
        const list = await listPullRequests(cwd, "open");
        const filtered =
          filter.length === 0
            ? list
            : list.filter(
                (p) =>
                  p.title.toLowerCase().includes(filter.toLowerCase()) ||
                  String(p.number).includes(filter),
              );
        if (!cancelled) setMentionPrMatches(filtered);
      } catch {
        if (!cancelled) setMentionPrMatches(EMPTY_PR_MATCHES);
      }
    }, MENTION_FETCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [
    mentionOpen,
    parsedMention.category,
    parsedMention.filter,
    cwd,
    isGithubRepo,
    ghAuthenticated,
  ]);

  const closeSlash = () => {
    setSlashAnchor(null);
    setSlashHighlighted(null);
  };

  const closeMention = useCallback(() => {
    setMentionAnchor(null);
    setMentionHighlighted(null);
    setFileMatches(EMPTY_FILE_MATCHES);
    setMentionIssueMatches(EMPTY_ISSUE_MATCHES);
    setMentionPrMatches(EMPTY_PR_MATCHES);
  }, []);

  const closeAttachPopup = useCallback(() => {
    setAttachOpen(false);
    setAttachSubmode("main");
    setAttachQuery("");
    // Keep the cached match arrays so reopening the popup is snappy;
    // the next open's effect re-fetches anyway when cwd / submode
    // changes invalidate them.
  }, []);

  // Dismiss any open popup when the user clicks outside of it. Without
  // this, the only escape hatches are the textarea Escape handler or
  // re-clicking the `+` trigger — clicking elsewhere on screen leaves
  // the popup stranded. The trigger button is excluded so its own
  // toggle handler fires unhindered; pickers' interior elements are
  // excluded so dragging the scrollbar / clicking items still works.
  useEffect(() => {
    if (!attachOpen && !slashAnchor && !mentionAnchor) return;
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target as HTMLElement | null;
      if (!target) return;
      if (
        target.closest('[data-testid="slash-command-popup"]') ||
        target.closest('[data-testid="composer-command-menu"]') ||
        target.closest('[data-testid="composer-issue-picker"]') ||
        target.closest('[data-testid="composer-pr-picker"]') ||
        target.closest('[data-testid="composer-attach-button"]')
      ) {
        return;
      }
      if (attachOpen) closeAttachPopup();
      if (slashAnchor) {
        setSlashAnchor(null);
        setSlashHighlighted(null);
      }
      if (mentionAnchor) closeMention();
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
    };
  }, [
    attachOpen,
    slashAnchor,
    mentionAnchor,
    closeAttachPopup,
    closeMention,
  ]);

  // Insert text at the textarea's current cursor, preserving prose
  // around the insertion point. Schedules a focus + caret restore
  // via rAF so React's render flush doesn't fight us.
  const insertAtCursor = useCallback(
    (insertion: string) => {
      const ta = textareaRef.current;
      const cursor = ta?.selectionStart ?? draft.length;
      const before = draft.slice(0, cursor);
      const after = draft.slice(cursor);
      const next = before + insertion + after;
      onDraftChange(next);
      const newCursor = (before + insertion).length;
      requestAnimationFrame(() => {
        const el = textareaRef.current;
        if (!el) return;
        el.focus();
        el.setSelectionRange(newCursor, newCursor);
      });
    },
    [draft, onDraftChange],
  );

  // Insert an inline `@<basename> ` token at the cursor, used by the
  // `+ → File…` and `+ → Folder…` paths. Mirrors the strip-and-
  // insert flow that the `@` mention popup uses, minus the strip.
  const insertInlineToken = useCallback(
    (basename: string) => {
      const ta = textareaRef.current;
      const cursor = ta?.selectionStart ?? draft.length;
      const after = draft.slice(cursor);
      const insertion = `@${basename}${after.startsWith(" ") ? "" : " "}`;
      insertAtCursor(insertion);
    },
    [draft, insertAtCursor],
  );

  // Toggle the `+` popup. Closes any open slash / mention popup so
  // only one popup is visible at a time. Resets the submode so the
  // user always lands on the main category list.
  const handleAttachClick = useCallback(() => {
    if (attachOpen) {
      closeAttachPopup();
      return;
    }
    setAttachOpen(true);
    setAttachSubmode("main");
    setAttachQuery("");
    if (slashAnchor) {
      setSlashAnchor(null);
      setSlashHighlighted(null);
    }
    if (mentionAnchor) closeMention();
  }, [
    attachOpen,
    closeAttachPopup,
    slashAnchor,
    mentionAnchor,
    closeMention,
  ]);

  // Lazy-fetch the `+` popup's MCP server list each time the
  // submode opens. The store-level toggles fire updates back to the
  // backend; the list itself is just a hydration of `list_mcp_servers`.
  useEffect(() => {
    if (!attachOpen) return;
    if (attachSubmode !== "mcp") return;
    let cancelled = false;
    void listMcpServers(cwd ?? null)
      .then((servers) => {
        if (!cancelled) setAttachMcpServers(servers);
      })
      .catch((err) => {
        console.warn("[mcp] listMcpServers (+ popup) failed:", err);
      });
    return () => {
      cancelled = true;
    };
  }, [attachOpen, attachSubmode, cwd]);

  // Lazy-fetch the `+` popup's file/folder list when its submode
  // changes. Uses alphabetical ordering (no live search) — the user
  // searches via `@`. 30 entries is the popup's browse cap. The
  // issue submode does NOT participate here: it mounts the
  // `IssuePickerPanel` component which owns its own fetch + search +
  // loading state.
  useEffect(() => {
    if (!attachOpen) return;
    if (attachSubmode !== "file" && attachSubmode !== "folder") return;
    if (!cwd) {
      if (attachSubmode === "file") setAttachFileMatches(EMPTY_FILE_MATCHES);
      else setAttachFolderMatches(EMPTY_FOLDER_MATCHES);
      return;
    }
    let cancelled = false;
    if (attachSubmode === "file") {
      void listProjectFiles(cwd, null, ATTACH_BROWSE_LIMIT)
        .then((matches) => {
          if (!cancelled) setAttachFileMatches(matches);
        })
        .catch(() => {
          if (!cancelled) setAttachFileMatches(EMPTY_FILE_MATCHES);
        });
    } else {
      void listProjectFolders(cwd, null, ATTACH_BROWSE_LIMIT)
        .then((matches) => {
          if (!cancelled) setAttachFolderMatches(matches);
        })
        .catch(() => {
          if (!cancelled) setAttachFolderMatches(EMPTY_FOLDER_MATCHES);
        });
    }
    return () => {
      cancelled = true;
    };
  }, [attachOpen, attachSubmode, cwd]);

  // Build SlashCommandItem[] from the fuzzy matches. The popup
  // component renders these generically — no file-specific knowledge.
  // `onSelect` is intentionally a no-op here because the popup-side
  // dispatch goes through `handleMentionPopupSelect`, which has the
  // full anchor + draft state in scope (the per-item closure would
  // capture stale values otherwise).
  // Stage 4 — branched by `parsedMention.category`. The id prefix
  // (`file:` vs `issue:`) routes the pick handler.
  const mentionItems = useMemo<SlashCommandItem[]>(() => {
    if (parsedMention.category === "issue") {
      // Match the IssuePickerPanel's visual language so users see
      // the same chrome whether they got here via `+` or `@`:
      //   open  → CircleDot (filled circle), tinted `text-success`
      //   closed → CircleCheck (circle with tick), muted
      // The right-aligned `command` slot keeps the textual
      // open/closed badge as a fallback for users with degraded
      // colour perception.
      return mentionIssueMatches.map((issue) => {
        const isOpen = issue.state.toUpperCase() !== "CLOSED";
        return {
          id: `issue:${issue.number}`,
          label: `#${issue.number}`,
          description: issue.title,
          command: isOpen ? "open" : "closed",
          icon: isOpen ? CircleDot : CircleCheck,
          iconClassName: isOpen ? "text-success" : "text-muted-foreground",
          group: "ISSUES",
          onSelect: () => {},
        };
      });
    }
    if (parsedMention.category === "pr") {
      // Stage 5 — PR rows mirror the PrPickerPanel's icon mapping so
      // both surfaces stay consistent. State + draft both contribute
      // to the icon shape + tint.
      return mentionPrMatches.map((pr) => {
        const upper = pr.state.toUpperCase();
        let Icon = GitPullRequest;
        let iconClassName = "text-success";
        let badge = "open";
        if (upper === "MERGED") {
          Icon = GitMerge;
          iconClassName = "text-chart-4";
          badge = "merged";
        } else if (upper === "CLOSED") {
          Icon = GitPullRequestClosed;
          iconClassName = "text-destructive";
          badge = "closed";
        } else if (pr.is_draft) {
          Icon = GitPullRequestDraft;
          iconClassName = "text-muted-foreground";
          badge = "draft";
        }
        return {
          id: `pr:${pr.number}`,
          label: `#${pr.number}`,
          description: pr.title,
          command: badge,
          icon: Icon,
          iconClassName,
          group: "PULL REQUESTS",
          onSelect: () => {},
        };
      });
    }
    return fileMatches.map((match) => ({
      id: `file:${match.absolute_path}`,
      label: match.path,
      command: `@${basename(match.path)}`,
      icon: FileIcon,
      group: "FILES",
      onSelect: () => {},
    }));
  }, [
    parsedMention.category,
    mentionIssueMatches,
    mentionPrMatches,
    fileMatches,
  ]);

  // Footer hint per category. File hints stay file-flavoured; issue
  // and pr hints surface the GitHub-specific failure modes (not a
  // repo, gh not authenticated). Keeps the user oriented when the
  // popup is empty for *reasons* rather than just no-matches.
  const mentionPopupFooter = useMemo(() => {
    if (
      parsedMention.category === "issue" ||
      parsedMention.category === "pr"
    ) {
      const noun = parsedMention.category === "issue" ? "issues" : "PRs";
      if (!cwd) {
        return {
          tone: "muted" as const,
          message: `Open this chat in a project to attach ${noun}.`,
        };
      }
      if (isGithubRepo === false) {
        return {
          tone: "muted" as const,
          message: "Not a GitHub repo.",
        };
      }
      if (ghAuthenticated === false) {
        return {
          tone: "muted" as const,
          message: "Sign in with: gh auth login",
        };
      }
      return null;
    }
    if (!cwd) {
      return {
        tone: "muted" as const,
        message: "Open this chat in a project to attach files.",
      };
    }
    return null;
  }, [parsedMention.category, cwd, isGithubRepo, ghAuthenticated]);

  // ─── Attach popup items + pick handler ───────────────────────────
  // Items are derived per-submode. The `main` view is static
  // (categories + nav nudges); `file` and `folder` map the cached
  // match arrays into picker rows. Disabled rows render muted and
  // skip selection (Stage 4–6 promise; the popup discovers them now
  // so users see what's coming without the row being live).
  const attachPopupItems = useMemo<SlashCommandItem[]>(() => {
    if (attachSubmode === "main") {
      // MODES come first (primary affordance — the popup is the
      // canonical mode selector now that the `+ Mode` dropdown is
      // gone). The currently-active mode is disabled so users can't
      // double-activate it. ATTACH follows; the disabled coming-soon
      // entries (Issue/PR/Image) stay for discoverability ahead of
      // Stages 4–6.
      return [
        // MODES — design tones (sky / amber / violet) on the icon
        // chips; the lucide icons stay in sync with MODE_CONFIG /
        // ModePill so the picked mode's pill shows the same glyph.
        {
          id: "mode:plan",
          label: "Plan",
          description: "Plan and design before coding",
          command: "/plan",
          icon: ListTodo,
          tone: "sky",
          group: "MODES",
          disabled: mode === "plan",
          onSelect: () => {},
        },
        {
          id: "mode:debug",
          label: "Debug",
          description: "Add diagnostic logs to find bugs",
          command: "/debug",
          icon: Bug,
          tone: "amber",
          group: "MODES",
          disabled: mode === "debug",
          onSelect: () => {},
        },
        {
          id: "mode:ask",
          label: "Ask",
          description: "Read-only conversational mode",
          command: "/ask",
          icon: MessageCircleQuestion,
          tone: "violet",
          group: "MODES",
          disabled: mode === "ask",
          onSelect: () => {},
        },
        // WORKFLOWS — single row, gated to the Claude provider. Kept
        // visible-but-disabled for other providers (design D-parity
        // with the `attach:image` capability gate) so the affordance
        // stays discoverable rather than vanishing.
        workflowCommand,
        {
          id: "attach:file",
          label: "File…",
          description: "Pick a file from your project",
          command: "",
          icon: FileIcon,
          tone: "muted",
          group: "ATTACH",
          onSelect: () => {},
        },
        {
          id: "attach:folder",
          label: "Folder…",
          description: "Attach a directory tree",
          command: "",
          icon: FolderOpen,
          tone: "muted",
          group: "ATTACH",
          onSelect: () => {},
        },
        {
          id: "attach:image",
          label: "Image…",
          // Stage 6 — capability gate. When `modelSupportsImages` is
          // false the entry stays visible (so users discover that the
          // affordance exists) but disabled with a model-specific hint.
          description: modelSupportsImages
            ? "Pick an image from disk"
            : "Current model doesn't support images",
          command: "",
          icon: ImageIcon,
          tone: "muted",
          group: "ATTACH",
          disabled: !modelSupportsImages,
          onSelect: () => {},
        },
        {
          id: "attach:issue",
          label: "GitHub Issue…",
          // Stage 4 — three flavours of disabled-state copy so the
          // user knows whether the row is disabled because they're
          // off-GitHub, off-auth, or just waiting on the preflight.
          // The row stays VISIBLE and dimmed with its reason in the
          // description; once the preflight resolves to true +
          // authenticated it enables and the copy returns to the
          // active-affordance line.
          description:
            isGithubRepo === false
              ? "Not a GitHub repo"
              : ghAuthenticated === false
                ? "Run gh auth login"
                : "Pick an issue from this repo",
          command: "",
          icon: CircleDot,
          tone: "muted",
          group: "ATTACH",
          disabled:
            isGithubRepo !== true || ghAuthenticated === false,
          onSelect: () => {},
        },
        {
          id: "attach:pr",
          label: "GitHub PR…",
          description:
            isGithubRepo === false
              ? "Not a GitHub repo"
              : ghAuthenticated === false
                ? "Run gh auth login"
                : "Pick a pull request from this repo",
          command: "",
          icon: GitPullRequest,
          tone: "muted",
          group: "ATTACH",
          disabled:
            isGithubRepo !== true || ghAuthenticated === false,
          onSelect: () => {},
        },
        {
          id: "attach:mcp",
          label: "MCP Servers…",
          description: "Toggle integrations the agent can call",
          command: "",
          icon: Server,
          tone: "green",
          group: "INTEGRATIONS",
          onSelect: () => {},
        },
      ];
    }
    if (attachSubmode === "mcp") {
      const rows: SlashCommandItem[] = attachMcpServers.map((server) => {
        const runtime = mcpRuntimes.get(server.id);
        const isCodemuxSelf = server.id === MCP_CODEMUX_SELF_ID;
        const isOff = mcpDisabledIds.includes(server.id);
        return {
          id: `mcp:${server.id}`,
          label: server.name,
          description: isCodemuxSelf
            ? "always on · built-in"
            : formatMcpRowStatus(runtime, isOff),
          command: "",
          icon: Server,
          tone: "green",
          group: "MCP SERVERS",
          // The Switch in `rightAdornment` owns the click; the row's
          // own onSelect is intentionally a no-op so users don't
          // accidentally toggle by hitting Enter on the highlighted row.
          onSelect: () => {},
          rightAdornment: isCodemuxSelf ? null : (
            <Switch
              checked={!isOff}
              onCheckedChange={() => mcpToggleDisabled(server.id)}
              aria-label={`Enable ${server.name}`}
              data-testid={`attach-mcp-${server.id}-toggle`}
            />
          ),
        };
      });
      rows.push({
        id: "mcp:settings",
        label: "Open MCP Settings",
        description: "View tools and manage servers",
        command: "",
        icon: Settings,
        tone: "muted",
        group: "MCP SERVERS",
        onSelect: () => {},
      });
      return rows;
    }
    if (attachSubmode === "file") {
      return attachFileMatches.map((match) => ({
        id: `attach-file:${match.absolute_path}`,
        label: match.path,
        command: "",
        icon: FileIcon,
        tone: "muted",
        group: "FILES",
        onSelect: () => {},
      }));
    }
    // Folder submode — `attachSubmode === "folder"` is the only
    // remaining branch (issue submode renders IssuePickerPanel
    // outside this items pipeline).
    return attachFolderMatches.map((match) => ({
      id: `attach-folder:${match.absolute_path}`,
      label: match.path,
      description: `${match.item_count} item${match.item_count === 1 ? "" : "s"}`,
      command: "",
      icon: FolderOpen,
      tone: "muted",
      group: "FOLDERS",
      onSelect: () => {},
    }));
  }, [
    attachSubmode,
    attachFileMatches,
    attachFolderMatches,
    attachMcpServers,
    mcpRuntimes,
    mcpDisabledIds,
    mcpToggleDisabled,
    mode,
    isGithubRepo,
    ghAuthenticated,
    modelSupportsImages,
    workflowCommand,
  ]);

  const attachPopupFooter = useMemo(() => {
    if (attachSubmode === "main") return null;
    if (!cwd) {
      return {
        tone: "muted" as const,
        message: "Open this chat in a project to browse.",
      };
    }
    if (attachSubmode === "file" && attachFileMatches.length === 0) {
      return {
        tone: "muted" as const,
        message: "Loading files… (Esc to go back)",
      };
    }
    if (attachSubmode === "folder" && attachFolderMatches.length === 0) {
      return {
        tone: "muted" as const,
        message: "Loading folders… (Esc to go back)",
      };
    }
    return {
      tone: "muted" as const,
      message: "Esc to go back",
    };
  }, [
    attachSubmode,
    cwd,
    attachFileMatches.length,
    attachFolderMatches.length,
  ]);

  // Rows the command menu actually shows for the current query.
  // Filtering here (rather than inside the menu) keeps the visible
  // list identical to the one cmdk navigates. Disabled rows survive
  // the filter so they stay visible with their reason; cmdk skips
  // them during keyboard nav. `filterCommandMenuItems` also matches a
  // leading `/` against the mode tags (so `/pl` finds Plan).
  const visibleAttachItems = useMemo(
    () => filterCommandMenuItems(attachPopupItems, attachQuery),
    [attachPopupItems, attachQuery],
  );

  // Escape inside the command menu: a submode walks back to `main`
  // (clearing the search); `main` closes the popup and restores focus
  // to the textarea. Mirrors the prior macOS / Slack-style nested-menu
  // UX, now that the menu's own focused search input receives the key.
  const handleAttachEscape = useCallback(() => {
    if (attachSubmode !== "main") {
      setAttachSubmode("main");
      setAttachQuery("");
      return;
    }
    closeAttachPopup();
    requestAnimationFrame(() => textareaRef.current?.focus());
  }, [attachSubmode, closeAttachPopup]);

  const handleAttachPopupSelect = useCallback(
    (item: SlashCommandItem) => {
      if (item.disabled) return;
      // Submode pivots
      if (item.id === "attach:file") {
        setAttachSubmode("file");
        setAttachQuery("");
        return;
      }
      if (item.id === "attach:folder") {
        setAttachSubmode("folder");
        setAttachQuery("");
        return;
      }
      if (item.id === "attach:issue") {
        setAttachSubmode("issue");
        setAttachQuery("");
        return;
      }
      if (item.id === "attach:pr") {
        setAttachSubmode("pr");
        setAttachQuery("");
        return;
      }
      if (item.id === "attach:mcp") {
        setAttachSubmode("mcp");
        setAttachQuery("");
        return;
      }
      if (item.id === "mcp:settings") {
        // Navigate to Settings → MCP Servers and close the popup.
        // Settings panel reads its initial section from the URL hash;
        // the chat-pane shell exposes a "settings" event the AppShell
        // listens to. Both work; we use the hash + window event fallback
        // so this works whether or not the listener is wired in tests.
        try {
          window.location.hash = "#settings/mcp";
        } catch {
          // jsdom may not let us set location — ignore.
        }
        window.dispatchEvent(
          new CustomEvent("codemux:open-settings", {
            detail: { section: "mcp" },
          }),
        );
        setAttachOpen(false);
        return;
      }
      // Stage 6 — `+ → Image…` triggers the hidden file input. Close
      // the popup first so the file dialog doesn't render under it.
      // The input's onChange handler dispatches each picked File to
      // onAttachImage; the input's `accept` attribute enforces the
      // png/jpeg/webp/gif allowlist on Linux/Windows file pickers
      // (the parent re-validates anyway because some file dialogs
      // ignore the hint).
      if (item.id === "attach:image") {
        closeAttachPopup();
        // requestAnimationFrame so the popup unmount completes before
        // the dialog opens; otherwise the dialog can flash under it.
        requestAnimationFrame(() => imageInputRef.current?.click());
        return;
      }
      // Mode picks — close the popup and activate the mode. The
      // mode pill renders in the chip strip above the textarea
      // (Stage 3 refactor); the footer no longer carries a mode
      // selector after the `+ Mode` dropdown was retired. Focus
      // returns to the textarea (it was on the menu's search input).
      if (
        item.id === "mode:plan" ||
        item.id === "mode:debug" ||
        item.id === "mode:ask"
      ) {
        const nextMode = item.id.slice("mode:".length) as ActivePillMode;
        closeAttachPopup();
        onModeActivate(nextMode);
        requestAnimationFrame(() => textareaRef.current?.focus());
        return;
      }
      // `/workflow` — insert the literal command text at the cursor so
      // the user types their task after it and sends it as a normal
      // message; the Claude runtime parses the prefix and drives the
      // orchestration server-side (no frontend workflow logic here).
      if (item.id === "workflow") {
        insertAtCursor("/workflow ");
        closeAttachPopup();
        return;
      }
      // File / folder picks: insert the inline token, dispatch the
      // resolve callback to the parent, close the popup.
      if (item.id.startsWith("attach-file:")) {
        const match = attachFileMatches.find(
          (m) => `attach-file:${m.absolute_path}` === item.id,
        );
        if (match) {
          const filename = basename(match.path);
          insertInlineToken(filename);
          onAttachFile?.(match);
        }
        closeAttachPopup();
        return;
      }
      if (item.id.startsWith("attach-folder:")) {
        const match = attachFolderMatches.find(
          (m) => `attach-folder:${m.absolute_path}` === item.id,
        );
        if (match) {
          const folderName = basename(match.path);
          insertInlineToken(folderName);
          onAttachFolder?.(match);
        }
        closeAttachPopup();
        return;
      }
      // Note: issue picks are handled by IssuePickerPanel directly,
      // not via this items pipeline. The pivot above
      // (`attach:issue` → setAttachSubmode("issue")) is the only
      // entry point that touches this code path for issues.
    },
    [
      attachFileMatches,
      attachFolderMatches,
      insertInlineToken,
      insertAtCursor,
      closeAttachPopup,
      onAttachFile,
      onAttachFolder,
      onModeActivate,
    ],
  );

  // Keep the highlighted file id valid as the match list shifts.
  useEffect(() => {
    if (!mentionOpen) return;
    const ids = mentionItems.map((item) => item.id);
    if (mentionHighlighted && ids.includes(mentionHighlighted)) return;
    setMentionHighlighted(ids[0] ?? null);
  }, [mentionOpen, mentionItems, mentionHighlighted]);

  /** Replace the typed `@<query>` with the picked token + trailing
   *  space and keep the cursor right after the insertion. Shared
   *  between file and issue picks so the inline-mirror stays in lock-
   *  step regardless of which kind landed. */
  const replaceMentionWithToken = useCallback(
    (token: string) => {
      if (!mentionAnchor) {
        requestAnimationFrame(() => textareaRef.current?.focus());
        return;
      }
      const consumed = 1 + mentionAnchor.query.length;
      const before = draft.slice(0, mentionAnchor.start);
      const after = draft.slice(mentionAnchor.start + consumed);
      const insertion = `@${token}${after.startsWith(" ") ? "" : " "}`;
      const next = before + insertion + after;
      onDraftChange(next);
      const cursor = (before + insertion).length;
      requestAnimationFrame(() => {
        const el = textareaRef.current;
        if (!el) return;
        el.focus();
        el.setSelectionRange(cursor, cursor);
      });
    },
    [mentionAnchor, draft, onDraftChange],
  );

  const handleMentionPopupSelect = useCallback(
    (item: SlashCommandItem) => {
      // Stage 4 — `@issue:` picks. Insert `@#<number>` as the
      // inline token and let the parent stage the chip + drive the
      // detail fetch. The id is `issue:<num>`, minted by the popup
      // builder.
      if (item.id.startsWith("issue:")) {
        const num = Number.parseInt(item.id.slice("issue:".length), 10);
        const issue = mentionIssueMatches.find((i) => i.number === num);
        if (!issue) {
          closeMention();
          return;
        }
        replaceMentionWithToken(`#${issue.number}`);
        closeMention();
        onAttachIssue?.(issue);
        return;
      }
      // Stage 5 — `@pr:` picks. Insert `@!<number>` (one-char
      // distinct from issue tokens) and dispatch to onAttachPr.
      if (item.id.startsWith("pr:")) {
        const num = Number.parseInt(item.id.slice("pr:".length), 10);
        const pr = mentionPrMatches.find((p) => p.number === num);
        if (!pr) {
          closeMention();
          return;
        }
        replaceMentionWithToken(`!${pr.number}`);
        closeMention();
        onAttachPr?.(pr);
        return;
      }
      const match = fileMatches.find(
        (m) => `file:${m.absolute_path}` === item.id,
      );
      if (!match) {
        closeMention();
        return;
      }
      // Step 8 Stage 2.1 — inline insertion. Replace the typed
      // `@<query>` with `@<basename> ` so the token stays in the
      // textarea and the mirror renders it as a chip. The trailing
      // space lets the user keep typing context without an extra
      // keystroke (collapsed when the next char is already a space).
      // Mirrors the skill-insertion pattern at the slash popup —
      // both inline-token systems share the same shape.
      const filename = basename(match.path);
      replaceMentionWithToken(filename);
      closeMention();
      onAttachFile?.(match);
    },
    [
      fileMatches,
      mentionIssueMatches,
      mentionPrMatches,
      replaceMentionWithToken,
      onAttachFile,
      onAttachIssue,
      onAttachPr,
      closeMention,
    ],
  );

  const handleSlashSelect = (item: SlashCommandItem) => {
    // Mirrors the `+` menu's `handleAttachPopupSelect` guard — Enter on
    // a highlighted-but-disabled row (e.g. `/workflow` on a non-Claude
    // provider) must no-op rather than expand/activate it. Mouse clicks
    // are already double-guarded inside SlashCommandPopup itself.
    if (item.disabled) return;
    if (slashAnchor) {
      const consumedLength = 1 + slashAnchor.query.length;
      const before = draft.slice(0, slashAnchor.start);
      const after = draft.slice(slashAnchor.start + consumedLength);

      if (item.id.startsWith("skill:") || item.id === "workflow") {
        // Cursor-style inline expansion. Replace the typed `/<query>`
        // with the full `/<skill-name> ` or `/workflow ` (trailing
        // space so the user can keep typing context after the token
        // without an extra keystroke). The mirror overlay highlights
        // skill tokens; `/workflow` is handled server-side by the
        // Claude runtime — this composer only ever inserts the text.
        const tokenName = item.command.replace(/^\//, "");
        const insertion = `/${tokenName}${after.startsWith(" ") ? "" : " "}`;
        const next = before + insertion + after;
        onDraftChange(next);
        // Cursor lands right after the inserted token (and the space we
        // added, if any). Schedule for after onDraftChange propagates.
        const newCursor = (before + insertion).length;
        requestAnimationFrame(() => {
          const el = textareaRef.current;
          if (!el) return;
          el.focus();
          el.setSelectionRange(newCursor, newCursor);
        });
      } else {
        // Mode picks (and any future non-text-token items) strip the
        // typed `/<query>` because the activation is state-only.
        onDraftChange(before + after);
        requestAnimationFrame(() => textareaRef.current?.focus());
      }
    } else {
      requestAnimationFrame(() => textareaRef.current?.focus());
    }
    closeSlash();
    item.onSelect();
  };

  // ─── Image attach: paste + drop wiring ───────────────────────────
  // Stage 6 — listens on the textarea for paste events, on the
  // composer wrapper for drops. Both paths defer file-type validation
  // (and the unsupported-type toast) to the parent's onAttachImage so
  // Composer stays display-only. When onAttachImage is undefined
  // (e.g. older call sites that haven't wired Stage 6 yet) the
  // handlers no-op rather than crash.
  const handlePasteImage = useCallback(
    async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      if (!onAttachImage) return;

      // ── Fast path: clipboardData carries the image bytes ──────────
      // Works in the browser dev mock (npm run dev / Chrome) and on
      // platforms where the webview exposes image files on the paste
      // event. Keep it FIRST so those paths never pay for the IPC
      // round-trip below.
      const items = Array.from(e.clipboardData?.items ?? []);
      const imageItems = items.filter(
        (item) => item.kind === "file" && item.type.startsWith("image/"),
      );
      if (imageItems.length > 0) {
        // Prevent the data-URL fallback the browser would otherwise
        // dump into the textarea. We still let plain-text paste work
        // because this branch only fires when the clipboard has image
        // files in it.
        e.preventDefault();
        for (const item of imageItems) {
          const file = item.getAsFile();
          if (file) await onAttachImage(file);
        }
        return;
      }

      // ── Fallback: Linux/WebKit2GTK strips image payloads from the JS
      // paste event, so `clipboardData.items` never contains an image
      // file. Read the OS clipboard server-side instead (mirrors the
      // new-workspace dialog's `paste_clipboard_image_to_file`). The
      // Rust command encodes a real PNG and returns the bytes; we wrap
      // them in a File so the parent's onAttachImage runs the exact
      // same MIME/size/animated-GIF validation + staging as the "+"
      // picker. Composer stays display-only — the bytes flow straight
      // through onAttachImage. ──
      let payload: Awaited<ReturnType<typeof pasteClipboardImage>>;
      try {
        payload = await pasteClipboardImage();
      } catch {
        // No image on the OS clipboard (text-only) or plugin error.
        // We did NOT preventDefault above, so the default paste has
        // already run — plain text lands in the textarea normally.
        return;
      }
      if (!payload.bytes.length) return;
      // Defensive preventDefault to match the dialog. On WebKit an
      // image-only clipboard pastes no text, so by the time this async
      // call resolves there's nothing to clobber; on a text+image
      // clipboard the text has already been pasted (acceptable — the
      // user gets both the text and the attached image).
      e.preventDefault();
      const file = new File([payload.bytes], "pasted-image.png", {
        type: payload.mime || "image/png",
      });
      await onAttachImage(file);
    },
    [onAttachImage],
  );

  const handleDragOver = useCallback((e: React.DragEvent) => {
    if (e.dataTransfer.types.includes("Files")) {
      // Without this preventDefault, the drop target rejects the
      // operation and the user gets a "no-entry" cursor. Keep it
      // permissive so the drop event below can decide what to do.
      e.preventDefault();
    }
  }, []);

  // Stage 7 — visual drop-target feedback. Counts enters/leaves so
  // moving across child elements doesn't flicker the highlight on
  // and off. Only counts file drags; ignores text/internal drags so
  // the highlight never fires on selecting text in the textarea.
  const handleDragEnter = useCallback((e: React.DragEvent) => {
    if (!e.dataTransfer.types.includes("Files")) return;
    setDragDepth((d) => d + 1);
  }, []);
  const handleDragLeave = useCallback((e: React.DragEvent) => {
    if (!e.dataTransfer.types.includes("Files")) return;
    setDragDepth((d) => Math.max(0, d - 1));
  }, []);

  const handleDrop = useCallback(
    async (e: React.DragEvent) => {
      // Always reset the drag highlight on drop, regardless of whether
      // the payload is something we can attach.
      setDragDepth(0);
      if (!onAttachImage) return;
      const files = Array.from(e.dataTransfer?.files ?? []);
      const imageFiles = files.filter((f) => f.type.startsWith("image/"));
      if (imageFiles.length === 0) return;
      e.preventDefault();
      for (const file of imageFiles) {
        await onAttachImage(file);
      }
    },
    [onAttachImage],
  );

  // Total bytes across staged image attachments. Drives the soft
  // 5MB warning chip rendered below the strip — non-blocking, the
  // user can still submit but the warning nudges them to compress
  // before sending.
  const totalImageBytes = useMemo(
    () =>
      stagedAttachments
        .filter((a) => a.kind === "image")
        .reduce((sum, a) => sum + (a.resolvedImage?.bytes.length ?? 0), 0),
    [stagedAttachments],
  );
  const showImageSizeWarning = totalImageBytes > 5 * 1024 * 1024;

  // Step 8 Stage 7 — soft warn at SOFT_LIMIT, hard cap at HARD_LIMIT.
  // The pane handlers enforce HARD_LIMIT at attach time; here we just
  // surface the "you're getting close" / "at the limit" copy so the
  // user can self-correct before the next add bounces.
  const stagedCount = stagedAttachments.length;
  const showCountSoftWarning =
    stagedCount >= ATTACHMENT_SOFT_LIMIT && stagedCount < ATTACHMENT_HARD_LIMIT;
  const showCountHardWarning = stagedCount >= ATTACHMENT_HARD_LIMIT;

  // Step 8 Stage 7 — drag-over state drives the wrapper's ring +
  // background tint so the user gets a visible "drop here" target.
  // Using a counter (rather than a bool) sidesteps the standard
  // dragenter/dragleave glitch where moving over a child element
  // fires `dragleave` on the parent before `dragenter` on the child:
  // every enter increments, every leave decrements, and the visual
  // is on whenever the count is > 0.
  const [dragDepth, setDragDepth] = useState(0);
  const isDragging = dragDepth > 0;

  // Follow-up queueing: submit is allowed WHILE a turn streams (the send
  // is queued, not rejected). It is still blocked while this composer's
  // own send RPC is in flight (`sending`) to avoid a double-send. The
  // Stop button stays visible whenever a turn is active or a send is
  // mid-flight (`busy`).
  const busy = streaming || sending;
  const canSubmit = sessionReady && !sending && draft.trim().length > 0;
  // Subtle affordance so the user knows Enter will queue rather than
  // interrupt, shown only while a turn streams and there's text to send.
  const showQueueHint = streaming && draft.trim().length > 0;

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Shift+Tab cycles modes regardless of popup state. preventDefault
    // is critical — the browser would otherwise move focus out of the
    // textarea via native tab navigation.
    if (e.shiftKey && e.key === "Tab") {
      e.preventDefault();
      const next = nextModeInCycle(mode);
      if (next === "default") {
        onModeRemove();
      } else {
        onModeActivate(next as ActivePillMode);
      }
      return;
    }

    if (slashOpen) {
      // Esc always closes — even when there are no items to pick.
      if (e.key === "Escape") {
        e.preventDefault();
        closeSlash();
        return;
      }
      // When the filter returned no matches, the popup is still
      // visible (showing "No commands match"), but keyboard nav has
      // nothing to do. Fall through to the normal Enter-to-submit
      // path so the user can still send the message-with-slash-text
      // without first dismissing the popup.
      if (filteredItems.length > 0) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          const ids = filteredItems.map((i) => i.id);
          const idx = slashHighlighted ? ids.indexOf(slashHighlighted) : -1;
          const next = ids[(idx + 1) % ids.length];
          if (next) setSlashHighlighted(next);
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          const ids = filteredItems.map((i) => i.id);
          const idx = slashHighlighted ? ids.indexOf(slashHighlighted) : 0;
          const next = ids[(idx - 1 + ids.length) % ids.length];
          if (next) setSlashHighlighted(next);
          return;
        }
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          const item = filteredItems.find((i) => i.id === slashHighlighted);
          if (item) handleSlashSelect(item);
          return;
        }
      }
    }

    // The redesigned command menu owns a real focused search input, so
    // its keyboard nav (arrows / Enter / Escape) is handled by cmdk +
    // the menu itself — not here on the textarea. When the menu is
    // open the textarea isn't focused, so no attach branch is needed.

    if (mentionOpen) {
      // Esc closes the mention popup; falls through to the normal
      // Enter-to-submit path when the filter returned no matches so
      // an "@unknown" mention can still be sent as plain prose.
      if (e.key === "Escape") {
        e.preventDefault();
        closeMention();
        return;
      }
      if (mentionItems.length > 0) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          const ids = mentionItems.map((i) => i.id);
          const idx = mentionHighlighted ? ids.indexOf(mentionHighlighted) : -1;
          const next = ids[(idx + 1) % ids.length];
          if (next) setMentionHighlighted(next);
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          const ids = mentionItems.map((i) => i.id);
          const idx = mentionHighlighted ? ids.indexOf(mentionHighlighted) : 0;
          const next = ids[(idx - 1 + ids.length) % ids.length];
          if (next) setMentionHighlighted(next);
          return;
        }
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          const item = mentionItems.find((i) => i.id === mentionHighlighted);
          if (item) handleMentionPopupSelect(item);
          return;
        }
      }
    }

    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (canSubmit) onSubmit();
    }
  };

  // ─── Slash & mention detection on text change ────────────────────
  // Pure cursor-aware detection — handles trigger-at-position-0,
  // trigger-after-whitespace, and refuses trigger-inside-word.
  // Mutual exclusion enforced: at most one popup is open at any time
  // (the cursor walk-back can only land on one trigger character).
  const handleTextareaChange = (
    e: React.ChangeEvent<HTMLTextAreaElement>,
  ) => {
    const value = e.target.value;
    const cursor = e.target.selectionStart ?? value.length;
    onDraftChange(value);
    if (composingRef.current) return;
    const slashHit = findSlashAtCursor(value, cursor);
    const mentionHit = findMentionAtCursor(value, cursor);
    if (slashHit) {
      setSlashAnchor(slashHit);
      if (mentionAnchor) closeMention();
      if (attachOpen) closeAttachPopup();
    } else if (mentionHit) {
      setMentionAnchor(mentionHit);
      if (slashAnchor) {
        setSlashAnchor(null);
        setSlashHighlighted(null);
      }
      if (attachOpen) closeAttachPopup();
    } else {
      if (slashAnchor) {
        setSlashAnchor(null);
        setSlashHighlighted(null);
      }
      if (mentionAnchor) closeMention();
    }
  };

  // Selection changes (arrow keys, mouse click) can move the cursor
  // out of a trigger context without changing the text — close
  // whichever popup was open when that happens.
  const handleSelect = (e: React.SyntheticEvent<HTMLTextAreaElement>) => {
    if (composingRef.current) return;
    const el = e.currentTarget;
    const cursor = el.selectionStart ?? 0;
    const slashHit = findSlashAtCursor(el.value, cursor);
    const mentionHit = findMentionAtCursor(el.value, cursor);
    if (slashHit) {
      if (
        !slashAnchor ||
        slashHit.start !== slashAnchor.start ||
        slashHit.query !== slashAnchor.query
      ) {
        setSlashAnchor(slashHit);
      }
      if (mentionAnchor) closeMention();
      return;
    }
    if (mentionHit) {
      if (
        !mentionAnchor ||
        mentionHit.start !== mentionAnchor.start ||
        mentionHit.query !== mentionAnchor.query
      ) {
        setMentionAnchor(mentionHit);
      }
      if (slashAnchor) closeSlash();
      return;
    }
    if (slashAnchor !== null) closeSlash();
    if (mentionAnchor !== null) closeMention();
  };

  return (
    <div className="w-full px-4 pb-3">
      <div className="mx-auto w-full max-w-[760px]">
        {zone1Override !== null && zone1Override !== undefined ? (
          <div className="pb-1">{zone1Override}</div>
        ) : zone1Override === undefined && cwd ? (
          <div className="px-3 pb-1 text-[11px] text-muted-foreground/70 truncate font-mono">
            {cwd}
          </div>
        ) : null}
        <div
          data-testid="composer-wrapper"
          data-dragging={isDragging || undefined}
          onDragOver={handleDragOver}
          onDragEnter={handleDragEnter}
          onDragLeave={handleDragLeave}
          onDrop={handleDrop}
          className={cn(
            "relative",
            // Composer card (design D10): 15px radius, a real hairline
            // border on the slightly-elevated surface, and a soft
            // shadow so the card lifts off the pane background. Focus
            // sharpens the border rather than stacking a ring.
            "rounded-[15px] border border-border bg-muted/40 shadow-sm",
            // Composer is mounted for the entire chat session and re-
            // renders frequently as the draft / attachments change.
            // `transition-all` would animate every property change;
            // scope to the properties that actually transition here
            // (drag-state border + tinted background, plus focus-within
            // border shift) so the compositor only has work to do on
            // those changes.
            "transition-[box-shadow,border-color,background-color]",
            "focus-within:border-muted-foreground/50",
            // Drag-over uses a neutral foreground-tinted ring instead
            // of the primary accent: the chat-ui skill reserves accent
            // for the app shell, and the brightness shift alone is
            // enough to confirm the drop target.
            isDragging &&
              "border-foreground/40 bg-foreground/[0.04] ring-1 ring-foreground/40",
          )}
        >
          {/* Step 8 Stage 6 — hidden image picker. The `+ → Image…`
              row triggers .click() on this input to surface the
              system file dialog. Multiple selection is allowed so
              users can attach a batch in one go. The `accept` attr
              filters png/jpeg/webp/gif at the OS picker level on
              platforms that respect it; the parent's onAttachImage
              re-validates because some platforms ignore the hint. */}
          <input
            ref={imageInputRef}
            type="file"
            accept="image/png,image/jpeg,image/webp,image/gif"
            multiple
            className="hidden"
            data-testid="composer-image-file-input"
            onChange={async (e) => {
              const files = Array.from(e.target.files ?? []);
              for (const file of files) {
                if (onAttachImage) await onAttachImage(file);
              }
              // Reset so re-picking the same file fires onChange again.
              e.target.value = "";
            }}
          />
          <SlashCommandPopup
            items={filteredItems}
            highlightedId={slashHighlighted}
            onHighlightChange={setSlashHighlighted}
            onSelect={handleSlashSelect}
            open={slashOpen}
            footerNote={slashPopupFooter}
          />
          <SlashCommandPopup
            items={mentionItems}
            highlightedId={mentionHighlighted}
            onHighlightChange={setMentionHighlighted}
            onSelect={handleMentionPopupSelect}
            open={mentionOpen}
            footerNote={mentionPopupFooter}
          />
          {/* Stage 4 polish — when the user lands on the issue
              submode, mount the existing IssuePickerPanel instead of
              the generic flat-row SlashCommandPopup. The picker
              already has the affordances users expect from the rest
              of the app: typeable search bar, debounced server-side
              search, green CircleDot for open + muted CircleCheck for
              closed, and a skeleton load. The same Tauri backend
              (listGithubIssuesByPath / getGithubIssueByPath) powers
              both surfaces — the cache layer added in Stage 4 makes
              the second call cheap regardless of which surface
              triggered the first. */}
          {attachOpen && attachSubmode === "issue" ? (
            <div
              data-testid="composer-issue-picker"
              className="absolute bottom-full left-0 right-0 mb-2 z-50 rounded-lg border border-border/60 bg-popover shadow-md overflow-hidden"
              onMouseDown={(e) => e.preventDefault()}
            >
              <IssuePickerPanel
                projectPath={cwd ?? ""}
                open
                onSelect={(issue) => {
                  insertInlineToken(`#${issue.number}`);
                  onAttachIssue?.(issue);
                  closeAttachPopup();
                }}
                onClose={closeAttachPopup}
              />
            </div>
          ) : attachOpen && attachSubmode === "pr" ? (
            <div
              data-testid="composer-pr-picker"
              className="absolute bottom-full left-0 right-0 mb-2 z-50 rounded-lg border border-border/60 bg-popover shadow-md overflow-hidden"
              onMouseDown={(e) => e.preventDefault()}
            >
              <PrPickerPanel
                projectPath={cwd ?? ""}
                open
                onSelect={(pr) => {
                  // Stage 5 — `@!<n>` is the inline token convention
                  // for PRs (one-char distinct from `@#<n>` issues).
                  insertInlineToken(`!${pr.number}`);
                  onAttachPr?.(pr);
                  closeAttachPopup();
                }}
                onClose={closeAttachPopup}
              />
            </div>
          ) : (
            <ComposerCommandMenu
              open={attachOpen}
              items={visibleAttachItems}
              query={attachQuery}
              onQueryChange={setAttachQuery}
              onSelect={handleAttachPopupSelect}
              onEscape={handleAttachEscape}
              footerNote={attachPopupFooter}
              submode={attachSubmode}
              placeholder={
                attachSubmode === "file"
                  ? "Filter files"
                  : attachSubmode === "folder"
                    ? "Filter folders"
                    : attachSubmode === "mcp"
                      ? "Filter servers"
                      : "Search or type /"
              }
            />
          )}
          {errorMessage && (
            <div
              role="alert"
              className="px-3 pt-2 text-[11px] text-destructive/90 leading-tight"
            >
              <span>Send failed: {errorMessage}. </span>
              <span className="text-muted-foreground/80">
                Press Enter to retry.
              </span>
            </div>
          )}
          {/* Step 8 Stage 3 refactor — strip above textarea hosts
              the active mode pill + image attachment chips. File /
              folder chips render INSIDE the textarea via the mirror
              overlay below; images can't live inline as text so they
              stay here. The mode pill moved out of the footer when
              the `+ Mode` dropdown was retired in favour of the
              unified `+` popup. */}
          {(mode !== "default" ||
            stagedAttachments.some(
              (a) => a.kind === "image" || a.kind === "pr",
            ) ||
            (interrupted && !streaming && !sending && !!onContinueRun)) && (
            <div
              data-testid="composer-attachment-strip"
              className="flex flex-wrap gap-1.5 px-3 pt-2"
            >
              {/* Dead-run recovery (issue #154): a one-click chip that
                  resumes the interrupted run. Amber-tinted, mirroring
                  ModePill's shape. */}
              {interrupted && !streaming && !sending && onContinueRun && (
                <button
                  type="button"
                  data-testid="composer-continue-run-chip"
                  onClick={onContinueRun}
                  className="inline-flex items-center gap-1.5 rounded-full bg-warning/15 px-2.5 py-1 text-xs text-warning hover:bg-warning/25"
                >
                  <RotateCw className="h-3 w-3" aria-hidden />
                  <span>Continue run</span>
                </button>
              )}
              {mode !== "default" && (
                <ModePill
                  mode={mode as ActivePillMode}
                  onRemove={onModeRemove}
                />
              )}
              {stagedAttachments
                .filter((a) => a.kind === "image" || a.kind === "pr")
                .map((attachment) => (
                  <AttachmentChip
                    key={attachment.id}
                    attachment={attachment}
                    onRemove={(id) => onRemoveAttachment?.(id)}
                    onToggleExpand={
                      attachment.kind === "pr" && onToggleExpandPr
                        ? (id) => onToggleExpandPr(id)
                        : undefined
                    }
                  />
                ))}
            </div>
          )}
          {/* Step 8 Stage 6 — soft 5MB warning. Non-blocking by
              design: the user can still send, but a request that big
              hits Anthropic's 32MB request cap fast and slows
              response time noticeably. We show actual bytes so the
              user can decide whether to compress before sending. */}
          {showImageSizeWarning && (
            <div
              data-testid="composer-image-size-warning"
              className="px-3 pt-1 text-[10px] text-warning"
            >
              Total image size: {(totalImageBytes / 1024 / 1024).toFixed(1)} MB
              — consider reducing for faster requests
            </div>
          )}
          {/* Step 8 Stage 7 — soft warning at the SOFT_LIMIT line so
              the user can self-trim before the hard cap blocks the
              next add. The hard-cap copy renders separately below. */}
          {showCountSoftWarning && (
            <div
              data-testid="composer-attachment-count-warning"
              className="px-3 pt-1 text-[10px] text-warning"
            >
              {stagedCount} attachments — consider trimming for cleaner prompts
            </div>
          )}
          {showCountHardWarning && (
            <div
              data-testid="composer-attachment-count-hardcap"
              className="px-3 pt-1 text-[10px] text-destructive"
            >
              {stagedCount} attachments — limit reached. Remove some to add
              more.
            </div>
          )}
          <div className="relative">
            {/*
              Mirror overlay: renders the same text as the textarea but
              with `/skill-name` tokens wrapped in a colored span. The
              textarea on top has transparent text + a visible caret, so
              the user sees the highlighted mirror through the
              transparent layer. Critical that the two layers share
              identical padding/font/line-height so cursor and mirror
              stay glued together at every position.
            */}
            <div
              ref={mirrorRef}
              aria-hidden
              data-testid="composer-highlight-mirror"
              className={cn(
                "pointer-events-none absolute inset-0 px-3 py-2.5",
                "whitespace-pre-wrap break-words",
                "text-sm text-foreground",
                // `overflow-y-auto` (rather than `overflow-hidden`) is
                // required so we can imperatively assign `scrollTop` —
                // setting `scrollTop` on a clipped element is a no-op in
                // some browsers. The mirror's own scrollbar is hidden
                // (next two utilities) so only the textarea's scrollbar
                // is ever visible to the user.
                "overflow-y-auto",
                "[scrollbar-width:none]",
                "[&::-webkit-scrollbar]:hidden",
              )}
            >
              {highlightSegments.map((seg, i) => {
                if (seg.kind === "skill") {
                  return (
                    <span
                      key={i}
                      className="text-status-working dark:text-status-working"
                    >
                      {seg.text}
                    </span>
                  );
                }
                if (seg.kind === "attachment") {
                  // Inline chip rendering. Background fills the text
                  // bounding box only — no padding tricks — so the
                  // mirror's character widths stay glued to the
                  // textarea's caret positions. Loading state dims
                  // the chip; an unresolvable read renders red.
                  return (
                    <span
                      key={i}
                      data-testid={`composer-attachment-token-${seg.basename}`}
                      data-loading={seg.isLoading || undefined}
                      data-error={seg.hasError || undefined}
                      className={cn(
                        "rounded-sm bg-foreground/10 text-foreground",
                        seg.isLoading && "opacity-60",
                        seg.hasError &&
                          "bg-destructive/15 text-destructive",
                      )}
                    >
                      {seg.text}
                    </span>
                  );
                }
                if (seg.kind === "issue-attachment") {
                  // Stage 4 — state-coloured pill. Open issues use the
                  // warning token (amber) to match the chip strip
                  // directly above; closed issues use a muted neutral
                  // so the eye isn't drawn to them. Error / loading
                  // states override the same way file tokens do.
                  return (
                    <span
                      key={i}
                      data-testid={`composer-issue-token-${seg.ref}`}
                      data-state={seg.state}
                      data-loading={seg.isLoading || undefined}
                      data-error={seg.hasError || undefined}
                      className={cn(
                        "rounded-sm",
                        seg.state === "open"
                          ? "bg-warning/15 text-warning"
                          : "bg-foreground/10 text-muted-foreground",
                        seg.isLoading && "opacity-60",
                        seg.hasError &&
                          "bg-destructive/15 text-destructive",
                      )}
                    >
                      {seg.text}
                    </span>
                  );
                }
                if (seg.kind === "pr-attachment") {
                  // Stage 5 — PR pill. Four state branches, matching
                  // the chip strip's colours so the inline token and
                  // the staged chip read as the same thing.
                  //   open    → primary blue   (active, mergeable)
                  //   merged  → purple/chart-4 (canonical "merged" hue)
                  //   closed  → muted          (unmerged, dropped)
                  //   draft   → muted          (in-progress)
                  const stateClass =
                    seg.state === "open"
                      ? "bg-primary/15 text-primary"
                      : seg.state === "merged"
                        ? "bg-chart-4/15 text-chart-4"
                        : "bg-foreground/10 text-muted-foreground";
                  return (
                    <span
                      key={i}
                      data-testid={`composer-pr-token-${seg.ref}`}
                      data-state={seg.state}
                      data-loading={seg.isLoading || undefined}
                      data-error={seg.hasError || undefined}
                      className={cn(
                        "rounded-sm",
                        stateClass,
                        seg.isLoading && "opacity-60",
                        seg.hasError &&
                          "bg-destructive/15 text-destructive",
                      )}
                    >
                      {seg.text}
                    </span>
                  );
                }
                return <Fragment key={i}>{seg.text}</Fragment>;
              })}
              {/* Trailing newline doesn't render unless followed by a
                  glyph — pad with a zero-width space so the mirror's
                  height matches the textarea's after a fresh Enter. */}
              {draft.endsWith("\n") || draft === "" ? "​" : null}
            </div>
            <textarea
              ref={textareaRef}
              value={draft}
              onChange={handleTextareaChange}
              onSelect={handleSelect}
              onScroll={handleTextareaScroll}
              onKeyDown={handleKeyDown}
              onPaste={handlePasteImage}
              onCompositionStart={() => {
                composingRef.current = true;
              }}
              onCompositionEnd={(e) => {
                composingRef.current = false;
                // After composition ends, run detection on the now-final
                // value so popup state catches up with what was typed.
                const el = e.currentTarget;
                const cursor = el.selectionStart ?? el.value.length;
                const slashHit = findSlashAtCursor(el.value, cursor);
                const mentionHit = findMentionAtCursor(el.value, cursor);
                if (slashHit) {
                  setSlashAnchor(slashHit);
                  if (mentionAnchor) closeMention();
                } else if (mentionHit) {
                  setMentionAnchor(mentionHit);
                  if (slashAnchor) closeSlash();
                } else {
                  closeSlash();
                  closeMention();
                }
              }}
              placeholder={
                sessionReady
                  ? (placeholderOverride ?? placeholderForMode(mode, isDraft))
                  : "Starting session…"
              }
              rows={1}
              className={cn(
                "relative w-full resize-none bg-transparent px-3 py-2.5",
                // Transparent text — the colored mirror behind shows
                // through. Caret stays visible via `caret-foreground`.
                "text-sm text-transparent caret-foreground",
                "placeholder:text-muted-foreground/60",
                "outline-none",
              )}
              style={{ maxHeight: `${MAX_ROWS_APPROX_PX}px` }}
            />
          </div>
          {showQueueHint ? (
            <div className="px-3 pb-1 text-[11px] leading-none text-muted-foreground/70">
              Enter to queue
            </div>
          ) : null}
          <ComposerFooter
            provider={provider}
            model={model}
            permissionMode={permissionMode}
            effort={effort}
            contextWindow={contextWindow}
            activeModel={activeModel}
            effortLabelMap={effortLabelMap}
            permissionModes={permissionModes}
            ultrathinkInBodyText={ultrathinkInBodyText}
            streaming={busy}
            canSubmit={canSubmit}
            showProviderPicker={showProviderPicker}
            showStopButton={showStopButton}
            mode={mode}
            onProviderChange={onProviderChange}
            onModelChange={onModelChange}
            onPermissionModeChange={onPermissionModeChange}
            onEffortChange={onEffortChange}
            onContextWindowChange={onContextWindowChange}
            onSubmit={onSubmit}
            onStop={onStop}
            controlsDisabled={!sessionReady}
            onAttachClick={handleAttachClick}
            attachOpen={attachOpen}
          />
        </div>
        {belowComposerSlot ? (
          <div className="pt-1.5">{belowComposerSlot}</div>
        ) : null}
      </div>
    </div>
  );
}

/** Placeholder text per Cursor-style mode. `default` keeps the
 *  existing copy; Plan/Ask/Debug swap in mode-specific prompts so
 *  users see at a glance what the pill changes about their next
 *  message. */
/** Step 9 Stage 4 — `+` popup MCP submode row description. Mirrors the
 *  Settings status badges in plain text so the popup row can convey
 *  the same state at a glance. */
function formatMcpRowStatus(
  runtime: import("@/tauri/commands").McpServerRuntime | undefined,
  isOff: boolean,
): string {
  if (isOff) return "disabled";
  if (!runtime) return "discovered";
  switch (runtime.status.kind) {
    case "running":
      return `${runtime.status.toolCount} tool${
        runtime.status.toolCount === 1 ? "" : "s"
      } running`;
    case "starting":
      return "starting…";
    case "errored":
      return `errored — ${runtime.status.message}`;
    case "stopped":
      return "stopped";
    case "discovered":
      return "discovered";
  }
}

function placeholderForMode(mode: ChatMode, isDraft: boolean): string {
  switch (mode) {
    case "plan":
      return "Plan and design before coding…";
    case "ask":
      return "Ask questions without making changes…";
    case "debug":
      return "Debug and troubleshoot issues…";
    case "default":
      // Design D10: the empty draft surface invites a fresh task; a
      // live session invites a follow-up / steer.
      return isDraft
        ? "Describe what you want the agent to do…"
        : "Reply or steer the agent…";
  }
}
