import {
  Bug,
  CircleDot,
  File as FileIcon,
  FolderOpen,
  GitPullRequest,
  Image as ImageIcon,
  ListTodo,
  MessageCircleQuestion,
} from "lucide-react";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { cn } from "@/lib/utils";
import { segmentDraftHighlight } from "@/lib/agent-chat/attachment-tokens";
import { buildSkillCommands } from "@/lib/agent-chat/skill-commands";
import {
  buildModeCommands,
  filterSlashItems,
  findMentionAtCursor,
  findSlashAtCursor,
  nextModeInCycle,
  type MentionAnchor,
  type SlashAnchor,
  type SlashCommandItem,
} from "@/lib/agent-chat/slash-commands";
import type { Attachment, ChatMode } from "@/stores/agent-chat-store";
import {
  selectActiveSkills,
  useSkillsStore,
} from "@/stores/skills-store";
import { listProjectFiles, listProjectFolders } from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  FileMatch,
  FolderMatch,
  PermissionModeOption,
} from "@/tauri/types";

import { AttachmentChip } from "./AttachmentChip";
import { ComposerFooter } from "./ComposerFooter";
import { ModePill, type ActivePillMode } from "./pickers/ModePill";
import { SlashCommandPopup } from "./SlashCommandPopup";

const EMPTY_ATTACHMENTS: Attachment[] = [];
const EMPTY_FILE_MATCHES: FileMatch[] = [];
const EMPTY_FOLDER_MATCHES: FolderMatch[] = [];
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

/** Three views the `+` popup can show. `main` lists categories +
 *  navigation nudges; `file` and `folder` list browsable rows that
 *  insert an inline `@<basename>` token on pick. */
type AttachSubmode = "main" | "file" | "folder";

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
  sessionReady: boolean;
  showProviderPicker: boolean;
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
   *  a project picker in when the draft target is Home. Pass `null`
   *  (or omit) to keep the default cwd label. */
  zone1Override?: React.ReactNode;
  /** Step 8 Stage 1 — staged attachments rendered as a chip strip
   *  inside the composer card, above the textarea. Empty array hides
   *  the strip. Defaults to `[]` so existing call sites keep working
   *  without changes. */
  stagedAttachments?: Attachment[];
  /** Step 8 Stage 1 — chip removal callback. Required for chips to be
   *  interactive; if omitted, the X button is a no-op. */
  onRemoveAttachment?: (attachmentId: string) => void;
  /** Step 8 Stage 2 — invoked when the user picks a file from the `@`
   *  mention popup OR the `+ → File…` browser. The Composer inserts
   *  the inline `@<basename>` token; the parent stages the chip +
   *  drives the readFileForAttachment resolution. Optional so
   *  existing call sites keep compiling. */
  onAttachFile?: (match: FileMatch) => void;
  /** Step 8 Stage 3 — invoked when the user picks a folder from the
   *  `+ → Folder…` browser. Mirrors `onAttachFile` for folders. */
  onAttachFolder?: (match: FolderMatch) => void;
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
  sessionReady,
  showProviderPicker,
  mode,
  errorMessage = null,
  showStopButton = true,
  zone1Override = null,
  stagedAttachments = EMPTY_ATTACHMENTS,
  onRemoveAttachment,
  onAttachFile,
  onAttachFolder,
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

  // Auto-grow textarea up to ~8 rows.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const desired = Math.min(el.scrollHeight, MAX_ROWS_APPROX_PX);
    el.style.height = `${desired}px`;
  }, [draft]);

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
  const mentionOpen = mentionAnchor !== null;
  const mentionQuery = mentionAnchor?.query ?? "";

  // ─── Attach (+) popup state — Step 8 Stage 3 ─────────────────────
  // Triggered by the `+` button in the footer (button-anchored, not
  // textarea-anchored). Submode pivots in-place inside the same
  // popup: main → file/folder → pick → close. Mutual exclusion with
  // slash + mention popups is enforced at the toggle point.
  const [attachOpen, setAttachOpen] = useState(false);
  const [attachSubmode, setAttachSubmode] = useState<AttachSubmode>("main");
  const [attachHighlighted, setAttachHighlighted] = useState<string | null>(
    null,
  );
  const [attachFileMatches, setAttachFileMatches] = useState<FileMatch[]>(
    EMPTY_FILE_MATCHES,
  );
  const [attachFolderMatches, setAttachFolderMatches] = useState<FolderMatch[]>(
    EMPTY_FOLDER_MATCHES,
  );

  const modeCommands = useMemo(
    () => buildModeCommands({ activeMode: mode, onActivate: onModeActivate }),
    [mode, onModeActivate],
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
    () => [...modeCommands, ...skillItems],
    [modeCommands, skillItems],
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
  useEffect(() => {
    if (!mentionOpen) return;
    if (!cwd) {
      setFileMatches(EMPTY_FILE_MATCHES);
      return;
    }
    let cancelled = false;
    const trimmed = mentionQuery.trim();
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
  }, [mentionOpen, mentionQuery, cwd]);

  const closeSlash = () => {
    setSlashAnchor(null);
    setSlashHighlighted(null);
  };

  const closeMention = useCallback(() => {
    setMentionAnchor(null);
    setMentionHighlighted(null);
    setFileMatches(EMPTY_FILE_MATCHES);
  }, []);

  const closeAttachPopup = useCallback(() => {
    setAttachOpen(false);
    setAttachSubmode("main");
    setAttachHighlighted(null);
    // Keep the cached match arrays so reopening the popup is snappy;
    // the next open's effect re-fetches anyway when cwd / submode
    // changes invalidate them.
  }, []);

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
    setAttachHighlighted(null);
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

  // Lazy-fetch the `+` popup's file or folder list when its submode
  // changes. Uses alphabetical ordering (no live search) — the user
  // searches via `@`. 30 entries is the popup's browse cap.
  useEffect(() => {
    if (!attachOpen) return;
    if (attachSubmode === "main") return;
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
  const fileItems = useMemo<SlashCommandItem[]>(
    () =>
      fileMatches.map((match) => ({
        id: `file:${match.absolute_path}`,
        label: match.path,
        command: `@${match.path.split("/").pop() ?? match.path}`,
        icon: FileIcon,
        group: "FILES",
        onSelect: () => {},
      })),
    [fileMatches],
  );

  // Footer hint when the chat isn't anchored to a project — the user
  // can still type `@`, but file matches won't load until they pick
  // a target directory.
  const mentionPopupFooter = useMemo(() => {
    if (!cwd) {
      return {
        tone: "muted" as const,
        message: "Open this chat in a project to attach files.",
      };
    }
    return null;
  }, [cwd]);

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
        {
          id: "mode:plan",
          label: "Plan",
          description: "Plan and design before coding",
          command: "/plan",
          icon: ListTodo,
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
          group: "MODES",
          disabled: mode === "ask",
          onSelect: () => {},
        },
        {
          id: "attach:file",
          label: "File…",
          description: "Pick a file from your project",
          command: "",
          icon: FileIcon,
          group: "ATTACH",
          onSelect: () => {},
        },
        {
          id: "attach:folder",
          label: "Folder…",
          description: "Attach a directory tree",
          command: "",
          icon: FolderOpen,
          group: "ATTACH",
          onSelect: () => {},
        },
        {
          id: "attach:issue",
          label: "GitHub Issue…",
          description: "coming soon",
          command: "",
          icon: CircleDot,
          group: "ATTACH",
          disabled: true,
          onSelect: () => {},
        },
        {
          id: "attach:pr",
          label: "GitHub PR…",
          description: "coming soon",
          command: "",
          icon: GitPullRequest,
          group: "ATTACH",
          disabled: true,
          onSelect: () => {},
        },
        {
          id: "attach:image",
          label: "Image…",
          description: "coming soon",
          command: "",
          icon: ImageIcon,
          group: "ATTACH",
          disabled: true,
          onSelect: () => {},
        },
      ];
    }
    if (attachSubmode === "file") {
      return attachFileMatches.map((match) => ({
        id: `attach-file:${match.absolute_path}`,
        label: match.path,
        command: "",
        icon: FileIcon,
        group: "FILES",
        onSelect: () => {},
      }));
    }
    return attachFolderMatches.map((match) => ({
      id: `attach-folder:${match.absolute_path}`,
      label: match.path,
      description: `${match.item_count} item${match.item_count === 1 ? "" : "s"}`,
      command: "",
      icon: FolderOpen,
      group: "FOLDERS",
      onSelect: () => {},
    }));
  }, [attachSubmode, attachFileMatches, attachFolderMatches, mode]);

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
  }, [attachSubmode, cwd, attachFileMatches.length, attachFolderMatches.length]);

  // Initialise / sync the attach-popup highlight to the first
  // enabled item whenever the visible item list shifts.
  useEffect(() => {
    if (!attachOpen) return;
    const enabledIds = attachPopupItems
      .filter((i) => !i.disabled)
      .map((i) => i.id);
    if (attachHighlighted && enabledIds.includes(attachHighlighted)) return;
    setAttachHighlighted(enabledIds[0] ?? null);
  }, [attachOpen, attachPopupItems, attachHighlighted]);

  const handleAttachPopupSelect = useCallback(
    (item: SlashCommandItem) => {
      if (item.disabled) return;
      // Submode pivots
      if (item.id === "attach:file") {
        setAttachSubmode("file");
        setAttachHighlighted(null);
        return;
      }
      if (item.id === "attach:folder") {
        setAttachSubmode("folder");
        setAttachHighlighted(null);
        return;
      }
      // Mode picks — close the popup and activate the mode. The
      // mode pill renders in the chip strip above the textarea
      // (Stage 3 refactor); the footer no longer carries a mode
      // selector after the `+ Mode` dropdown was retired.
      if (item.id === "mode:plan") {
        closeAttachPopup();
        onModeActivate("plan");
        return;
      }
      if (item.id === "mode:debug") {
        closeAttachPopup();
        onModeActivate("debug");
        return;
      }
      if (item.id === "mode:ask") {
        closeAttachPopup();
        onModeActivate("ask");
        return;
      }
      // File / folder picks: insert the inline token, dispatch the
      // resolve callback to the parent, close the popup.
      if (item.id.startsWith("attach-file:")) {
        const match = attachFileMatches.find(
          (m) => `attach-file:${m.absolute_path}` === item.id,
        );
        if (match) {
          const filename = match.path.split("/").pop() ?? match.path;
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
          const basename = match.path.split("/").pop() ?? match.path;
          insertInlineToken(basename);
          onAttachFolder?.(match);
        }
        closeAttachPopup();
        return;
      }
    },
    [
      attachFileMatches,
      attachFolderMatches,
      insertInlineToken,
      closeAttachPopup,
      onAttachFile,
      onAttachFolder,
      onModeActivate,
    ],
  );

  // Keep the highlighted file id valid as the match list shifts.
  useEffect(() => {
    if (!mentionOpen) return;
    const ids = fileItems.map((item) => item.id);
    if (mentionHighlighted && ids.includes(mentionHighlighted)) return;
    setMentionHighlighted(ids[0] ?? null);
  }, [mentionOpen, fileItems, mentionHighlighted]);

  const handleMentionPopupSelect = useCallback(
    (item: SlashCommandItem) => {
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
      if (mentionAnchor) {
        const consumed = 1 + mentionAnchor.query.length;
        const before = draft.slice(0, mentionAnchor.start);
        const after = draft.slice(mentionAnchor.start + consumed);
        const filename = match.path.split("/").pop() ?? match.path;
        const insertion = `@${filename}${after.startsWith(" ") ? "" : " "}`;
        const next = before + insertion + after;
        onDraftChange(next);
        const cursor = (before + insertion).length;
        requestAnimationFrame(() => {
          const el = textareaRef.current;
          if (!el) return;
          el.focus();
          el.setSelectionRange(cursor, cursor);
        });
      } else {
        requestAnimationFrame(() => textareaRef.current?.focus());
      }
      closeMention();
      onAttachFile?.(match);
    },
    [
      fileMatches,
      mentionAnchor,
      draft,
      onDraftChange,
      onAttachFile,
      closeMention,
    ],
  );

  const handleSlashSelect = (item: SlashCommandItem) => {
    if (slashAnchor) {
      const consumedLength = 1 + slashAnchor.query.length;
      const before = draft.slice(0, slashAnchor.start);
      const after = draft.slice(slashAnchor.start + consumedLength);

      if (item.id.startsWith("skill:")) {
        // Cursor-style inline expansion. Replace the typed `/<query>`
        // with the full `/<skill-name> ` (trailing space so the user can
        // keep typing context after the token without an extra
        // keystroke). The mirror overlay highlights it; send-time
        // parsing resolves it to the skill body.
        const skillName = item.command.replace(/^\//, "");
        const insertion = `/${skillName}${after.startsWith(" ") ? "" : " "}`;
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

  const canSubmit = sessionReady && !streaming && draft.trim().length > 0;

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

    if (attachOpen) {
      // Esc inside a submode walks back to main; Esc on main closes
      // the popup. Mirrors the macOS / Slack-style nested-menu UX.
      if (e.key === "Escape") {
        e.preventDefault();
        if (attachSubmode !== "main") {
          setAttachSubmode("main");
          setAttachHighlighted(null);
        } else {
          closeAttachPopup();
        }
        return;
      }
      // Arrow nav skips disabled rows entirely. Enter activates the
      // currently highlighted (enabled) row via the same dispatch
      // path the mouse-click would use.
      const enabledIds = attachPopupItems
        .filter((i) => !i.disabled)
        .map((i) => i.id);
      if (enabledIds.length > 0) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          const idx = attachHighlighted
            ? enabledIds.indexOf(attachHighlighted)
            : -1;
          const next = enabledIds[(idx + 1) % enabledIds.length];
          if (next) setAttachHighlighted(next);
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          const idx = attachHighlighted
            ? enabledIds.indexOf(attachHighlighted)
            : 0;
          const next =
            enabledIds[(idx - 1 + enabledIds.length) % enabledIds.length];
          if (next) setAttachHighlighted(next);
          return;
        }
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          const item = attachPopupItems.find(
            (i) => i.id === attachHighlighted,
          );
          if (item) handleAttachPopupSelect(item);
          return;
        }
      }
      // Other keys (typing, etc.) pass through to the textarea so
      // the user can keep editing prose while the popup is open.
    }

    if (mentionOpen) {
      // Esc closes the mention popup; falls through to the normal
      // Enter-to-submit path when the filter returned no matches so
      // an "@unknown" mention can still be sent as plain prose.
      if (e.key === "Escape") {
        e.preventDefault();
        closeMention();
        return;
      }
      if (fileItems.length > 0) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          const ids = fileItems.map((i) => i.id);
          const idx = mentionHighlighted ? ids.indexOf(mentionHighlighted) : -1;
          const next = ids[(idx + 1) % ids.length];
          if (next) setMentionHighlighted(next);
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          const ids = fileItems.map((i) => i.id);
          const idx = mentionHighlighted ? ids.indexOf(mentionHighlighted) : 0;
          const next = ids[(idx - 1 + ids.length) % ids.length];
          if (next) setMentionHighlighted(next);
          return;
        }
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          const item = fileItems.find((i) => i.id === mentionHighlighted);
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
      <div className="mx-auto w-full max-w-2xl">
        {zone1Override !== null
          ? <div className="pb-1">{zone1Override}</div>
          : cwd && (
              <div className="px-3 pb-1 text-[11px] text-muted-foreground/70 truncate font-mono">
                {cwd}
              </div>
            )}
        <div
          className={cn(
            "relative",
            "rounded-xl bg-muted/30 ring-1 ring-border/60 focus-within:ring-muted-foreground/60",
            "transition-shadow",
          )}
        >
          <SlashCommandPopup
            items={filteredItems}
            highlightedId={slashHighlighted}
            onHighlightChange={setSlashHighlighted}
            onSelect={handleSlashSelect}
            open={slashOpen}
            footerNote={slashPopupFooter}
          />
          <SlashCommandPopup
            items={fileItems}
            highlightedId={mentionHighlighted}
            onHighlightChange={setMentionHighlighted}
            onSelect={handleMentionPopupSelect}
            open={mentionOpen}
            footerNote={mentionPopupFooter}
          />
          <SlashCommandPopup
            items={attachPopupItems}
            highlightedId={attachHighlighted}
            onHighlightChange={setAttachHighlighted}
            onSelect={handleAttachPopupSelect}
            open={attachOpen}
            footerNote={attachPopupFooter}
          />
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
            stagedAttachments.some((a) => a.kind === "image")) && (
            <div
              data-testid="composer-attachment-strip"
              className="flex flex-wrap gap-1.5 px-3 pt-2"
            >
              {mode !== "default" && (
                <ModePill
                  mode={mode as ActivePillMode}
                  onRemove={onModeRemove}
                />
              )}
              {stagedAttachments
                .filter((a) => a.kind === "image")
                .map((attachment) => (
                  <AttachmentChip
                    key={attachment.id}
                    attachment={attachment}
                    onRemove={(id) => onRemoveAttachment?.(id)}
                  />
                ))}
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
              aria-hidden
              data-testid="composer-highlight-mirror"
              className={cn(
                "pointer-events-none absolute inset-0 px-3 py-2.5",
                "whitespace-pre-wrap break-words",
                "text-sm text-foreground",
                "overflow-hidden",
              )}
            >
              {highlightSegments.map((seg, i) => {
                if (seg.kind === "skill") {
                  return (
                    <span
                      key={i}
                      className="text-amber-500 dark:text-amber-400"
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
              onKeyDown={handleKeyDown}
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
                sessionReady ? placeholderForMode(mode) : "Starting session…"
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
            streaming={streaming}
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
      </div>
    </div>
  );
}

/** Placeholder text per Cursor-style mode. `default` keeps the
 *  existing copy; Plan/Ask/Debug swap in mode-specific prompts so
 *  users see at a glance what the pill changes about their next
 *  message. */
function placeholderForMode(mode: ChatMode): string {
  switch (mode) {
    case "plan":
      return "Plan and design before coding…";
    case "ask":
      return "Ask questions without making changes…";
    case "debug":
      return "Debug and troubleshoot issues…";
    case "default":
      return "Message the agent…";
  }
}
