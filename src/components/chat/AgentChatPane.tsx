import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Folder, GitBranch, Home } from "lucide-react";

import { useAgentChatEvents } from "@/hooks/use-agent-chat-events";
import {
  planCapabilityCompatReset,
  planEffortChange,
  planModelChange,
  planPermissionModeChange,
  planSubmit,
} from "@/lib/agent-chat/chat-pane-plans";
import {
  buildAttachmentBlock,
  buildFileResolvedContent,
  buildFolderResolvedContent,
  buildImageDisplaySources,
  buildImageRefs,
  buildIssueResolvedContent,
  buildPrResolvedContent,
  imageAttachmentIds,
  unstagedImageAttachments,
} from "@/lib/agent-chat/attachment-block";
import {
  awaitImageStaging,
  beginImageStaging,
  discardStagedImage,
} from "@/lib/agent-chat/image-staging";
import { activeAttachments } from "@/lib/agent-chat/attachment-tokens";
import { applyAllPrefixes } from "@/lib/agent-chat/mode-prefix";
import { resolveSkillBodies } from "@/lib/agent-chat/skill-tokens";
import {
  selectActiveSkills,
  useSkillsStore,
} from "@/stores/skills-store";
import type {
  ChatViewItem,
  PermissionRequestItem,
} from "@/lib/agent-chat/types";
import {
  findSubagentView,
  subagentOrdinal,
} from "@/lib/agent-chat/subagents";
import { hasUltrathinkInBodyText } from "@/lib/agent-chat/ultrathink";
import { basename } from "@/lib/path";
import { toast } from "@/lib/toast";
import {
  findWorkspaceIdForPane,
  groupWorkspacesByProject,
  useAppStore,
  useHomeDir,
} from "@/stores/app-store";
import {
  DEFAULT_THREAD_PERMISSION_MODE,
  useAgentChatStore,
  type Attachment,
} from "@/stores/agent-chat-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useShallow } from "zustand/react/shallow";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import {
  activateWorkspace,
  agentChatCancelQueuedTurn,
  agentChatSendQueuedTurnNow,
  agentChatGetSession,
  agentChatInterruptTurn,
  agentChatListMessages,
  agentChatTurnActive,
  agentChatListSessions,
  agentChatRespondToRequest,
  agentChatSendTurn,
  agentChatSetModel,
  agentChatSetPermissionMode,
  agentChatStartSession,
  agentChatStopSession,
  agentChatUpdateSessionConfig,
  checkGhStatus,
  checkGithubRepo,
  getGithubIssueByPath,
  getGithubPrByPath,
  getGithubPrDiffByPath,
  grepCountPattern,
  primeChatMcp,
  readFileForAttachment,
  readFolderForAttachment,
  type AgentChatSessionConfigUpdate,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import { replayPayloads } from "@/lib/agent-chat/hydrate";
import { createDeferredWorktree } from "@/lib/agent-chat/materialize";
import { prestartWorktreeSession } from "@/lib/agent-chat/prestart-worktree-session";
import { capabilityDefaults } from "@/lib/agent-chat/capability-defaults";
import type { AgentChatEventPayload, ApprovalDecision } from "@/tauri/events";
import type {
  AgentChatProviderKind,
  FileMatch,
  FolderMatch,
  GitHubIssue,
  PaneNodeSnapshot,
  PullRequestInfo,
} from "@/tauri/types";

import { ChatTranscript } from "./ChatTranscript";
import { ChatHomeLanding } from "./ChatHomeLanding";
import { Composer } from "./Composer";
import { SubagentActivityBar } from "./SubagentActivityBar";
import { SubagentBreadcrumb } from "./SubagentBreadcrumb";
import { SubagentView } from "./SubagentView";
import { DebugCleanupBanner } from "./DebugCleanupBanner";
import { DebugExitDialog, type DebugExitChoice } from "./DebugExitDialog";
import {
  type AskUserQuestionOutput,
  ComposerPendingInputPanel,
} from "./ComposerPendingInputPanel";
import { defaultModelForProvider } from "./pickers/ModelPicker";
import type { ActivePillMode } from "./pickers/ModePill";
import { ThreadScopeRow } from "./pickers/ThreadScopeRow";
import { WorkspaceStatusCluster } from "./WorkspaceStatusCluster";
import { useUIStore } from "@/stores/ui-store";

// Kept for parity with Step 1's export shape. The pane tree renderer
// passes the pane snapshot verbatim; nothing else imports this type.
type AgentChatPaneNode = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

// Step 12 Stage 4 — flipped on. The picker is now the unified
// `MultiProviderModelPicker` (provider rail + searchable model list)
// that replaces the side-by-side `ProviderPicker + ModelPicker` pair.
// Codex now ships proper `permission_modes` so the original
// `set_permission_mode` concern is moot; OpenCode is federated through
// `sub_provider`. The picker is the only entry point for provider
// switching from inside an existing pane.
const ENABLE_PROVIDER_PICKER = true;

/** Step 8 Stage 7 — hard cap on staged attachments. Above this we
 *  toast and reject the next attach so prompts can't silently grow
 *  into a token-budget cliff. The matching soft-warning copy lives
 *  in Composer.tsx (rendering concern); this constant gates the
 *  attach handlers. */
const ATTACHMENT_HARD_LIMIT = 20;
/** Step 8 Stage 7 — issue/PR fetches go stale at this age. On send,
 *  we re-fetch any GitHub-kind attachment whose `fetchedAt` is older
 *  so the agent always sees fresh detail (state flips, new comments)
 *  even if the user kept the picker open for a few minutes before
 *  hitting Enter. Lines up with the gh detail cache TTL on the Rust
 *  side so a re-fetch is cheap when the cached value is still warm. */
const STALE_ATTACHMENT_THRESHOLD_MS = 60_000;

/** Synthetic user prompt fired when the user invokes Debug-mode
 *  cleanup. The grep pattern carries no comment-prefix so the model
 *  finds CODEMUX_DEBUG markers across every language Claude touched. */
const CLEANUP_PROMPT = `Search for \`CODEMUX_DEBUG\` across the project and remove every line containing that marker. Also remove any surrounding debug-only scaffolding (variable declarations, imports) that were added solely to support those markers. After removing, run a final grep to verify zero matches remain.`;

/** Step 8 Stage 7 — animated GIF detection. Anthropic's image API
 *  rejects animated GIFs (only the first frame is read, and that
 *  often produces confusing errors). We detect at attach time and
 *  reject with a clear toast rather than letting the SDK surface a
 *  cryptic 400. The GIF89a spec frames each animation cycle with a
 *  Graphic Control Extension marker (`0x21 0xF9 0x04`); a static
 *  GIF has 0–1, an animated GIF has 2+. The scan walks the entire
 *  file but bails early on the second hit so even multi-MB animated
 *  GIFs cost milliseconds. */
export function detectAnimatedGif(buffer: ArrayBuffer): boolean {
  const bytes = new Uint8Array(buffer);
  let frameCount = 0;
  for (let i = 0; i < bytes.length - 3; i++) {
    if (
      bytes[i] === 0x21 &&
      bytes[i + 1] === 0xf9 &&
      bytes[i + 2] === 0x04
    ) {
      frameCount++;
      if (frameCount > 1) return true;
    }
  }
  return false;
}

// Jump-to-card correction-loop tuning for the docked SubagentActivityBar
// (see `handleJumpToSubagentCard` below). Mirrors MessageTrail.tsx's own
// jump constants: `content-visibility:auto` rows only expose estimated
// heights until they render, so a single scrollTop computation can land
// off; a few corrective rAF passes converge on the real offset.
const JUMP_SCROLL_MARGIN_PX = 16;
const JUMP_SETTLE_TOLERANCE_PX = 2;
const JUMP_SETTLE_STABLE_FRAMES = 3;
const JUMP_SETTLE_MAX_FRAMES = 40;
/** How long the ember ring highlight stays on a jumped-to card (design:
 *  "flashes `box-shadow` ... for ~1100ms"). */
const JUMP_HIGHLIGHT_MS = 1100;

export function AgentChatPane({ pane }: { pane: AgentChatPaneNode }) {
  const initialProvider: AgentChatProviderKind = pane.provider ?? "claude";
  const [provider, setProvider] = useState<AgentChatProviderKind>(initialProvider);
  const [threadId, setThreadId] = useState<string | null>(pane.thread_id);
  // Session-created timestamp for the transcript's top-of-thread marker
  // (design D2). Resolved from the persisted sessions list, keyed by the
  // active thread id; `undefined` until it resolves (or when no record
  // matches) so ChatTranscript falls back to the plain "Session started"
  // divider.
  const [sessionStartedAt, setSessionStartedAt] = useState<number | undefined>(
    undefined,
  );
  const [starting, setStarting] = useState(false);
  const [restarting, setRestarting] = useState(false);
  // Optimistic in-flight flag mirroring the reference impl's
  // `isSendBusy` (ChatView.tsx:406). Set synchronously on submit so
  // the button disables BEFORE the backend's Running event
  // round-trips.
  //
  // NOTE: `isSending` is useState (drives the render), but rapid-fire
  // Enter presses in the SAME JS tick don't see the updated state —
  // the `useCallback` closure captured the pre-set snapshot. The
  // reference impl pairs the state with a `sendInFlightRef.current`
  // synchronous guard (ChatView.tsx:2458) for exactly this reason:
  // refs mutate synchronously, so the second call within the same
  // tick sees the flag the first one just set. We do the same.
  const [isSending, setIsSending] = useState(false);
  const sendInFlightRef = useRef(false);

  // Stage 6 Debug-mode cleanup affordances. The exit dialog opens when
  // the user removes the Debug pill while CODEMUX_DEBUG markers exist;
  // the busy flag disables the in-banner Clean up button while the
  // synthetic cleanup turn is in flight.
  const [debugExitDialog, setDebugExitDialog] = useState<{
    resolve: (choice: DebugExitChoice) => void;
  } | null>(null);
  // The `cleanupInFlight` state drives the banner's busy label; the
  // ref is the synchronous race guard. Same trick as `sendInFlightRef`
  // — React state updates aren't visible to a second click that fires
  // in the same tick (the closure already captured `cleanupInFlight =
  // false`), so the ref provides the actual single-fire guarantee.
  const [cleanupInFlight, setCleanupInFlight] = useState(false);
  const cleanupInFlightRef = useRef(false);

  const fallbackCwd = useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) => w.workspace_id === s.appState!.active_workspace_id,
    );
    return ws?.cwd ?? null;
  });
  const cwd = pane.cwd ?? fallbackCwd;

  // Step 8 Stage 4 — preflight GitHub status. We check on mount + on
  // cwd change so a workspace swap doesn't leave the popup acting on
  // a stale answer. `null` means "not yet known" — the popup keeps
  // GitHub entries disabled while we wait so the user never sees the
  // entry flip from enabled to disabled. Errors fall back to `false`
  // (treat as not-a-github-repo) which gives the user a reachable
  // disabled-state instead of a crashing popup.
  const [isGithubRepo, setIsGithubRepo] = useState<boolean | null>(null);
  const [ghAuthenticated, setGhAuthenticated] = useState<boolean | null>(null);
  useEffect(() => {
    if (!cwd) {
      setIsGithubRepo(false);
      setGhAuthenticated(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const [repo, gh] = await Promise.all([
          checkGithubRepo(cwd),
          checkGhStatus(),
        ]);
        if (cancelled) return;
        setIsGithubRepo(repo);
        setGhAuthenticated(gh.status === "Authenticated");
      } catch {
        if (cancelled) return;
        setIsGithubRepo(false);
        setGhAuthenticated(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [cwd]);

  // Stage C race fix: when the pane was just created by
  // `materializeAndSend` → `agent_chat_create_pane`, its `thread_id`
  // starts as `null` and only flips to the draft's pre-minted value
  // once the `agent_chat_start_session` emit lands on the frontend.
  // If mount happens inside that race window, the original `useState`
  // initialiser captured `null` and the mount effect below would mint
  // a fresh thread id + start a duplicate session — orphaning the
  // slice materialise seeded with the draft's thread id. This
  // prop-sync effect catches up whenever `pane.thread_id` becomes
  // non-null after mount.
  useEffect(() => {
    if (pane.thread_id && pane.thread_id !== threadId) {
      setThreadId(pane.thread_id);
    }
  }, [pane.thread_id, threadId]);

  // Session startup persists provider + thread as one backend binding. Keep
  // the pane-local router in sync with that authoritative snapshot so a
  // successful provider handoff (or an external state refresh immediately
  // after it) cannot leave callbacks closed over the previous adapter.
  useEffect(() => {
    if (pane.provider) {
      setProvider(pane.provider);
    }
  }, [pane.provider]);

  // Stage C race fix (belt to the above suspender): if `pane.thread_id`
  // is still null at mount but a promoted draft claims this workspace,
  // use the draft's pre-minted thread id directly. Materialize already
  // started the session server-side — AgentChatPane just subscribes.
  const workspaceIdForPane = useAppStore((s) =>
    findWorkspaceIdForPane(s, pane.pane_id),
  );
  const promotedDraftThreadId = useChatDraftStore((s) => {
    if (!workspaceIdForPane) return null;
    // Match must be both workspace AND pane-scoped. A pure workspace
    // match would hand the FIRST pane's threadId to a freshly-created
    // SECOND pane in the same workspace (e.g. a Chat Agent preset
    // click on a workspace already hosting a chat), making the two
    // panes mirror the same Zustand slice. CLI panes don't have this
    // problem because each gets a uniquely-minted `session_id` —
    // chats need the equivalent invariant scoped per pane id.
    const match = Object.values(s.draftsById).find(
      (d) =>
        d.promotedTo?.workspaceId === workspaceIdForPane &&
        d.promotedTo.paneId === pane.pane_id,
    );
    return match?.threadId ?? null;
  });
  // Recovery fallback: if materialize made it past pane-creation but
  // failed during start_session / send_turn, the draft has
  // `materializedTo` set and `promotedTo` still null. The existing
  // `promotedDraftThreadId` selector above doesn't match — it only
  // looks for fully-promoted drafts. The mount effect below uses this
  // to finish the half-completed materialise: adopt the orphan thread
  // id, start the session that step 3 never managed, and call
  // `markPromoted` so we don't loop.
  // `useShallow` is REQUIRED here. The selector synthesises a fresh
  // object literal whenever a match exists, so default `Object.is`
  // equality would treat every render as a state change and trigger
  // an infinite re-render loop ("Maximum update depth exceeded").
  // `useShallow` does a shallow per-key comparison and returns the
  // previous reference when the field values are unchanged.
  const recoveryDraft = useChatDraftStore(
    useShallow((s) => {
      if (!workspaceIdForPane) return null;
      // Pane-scoped match — see promotedDraftThreadId for the same
      // rationale. A fresh second chat pane in the same workspace
      // must NOT inherit the first pane's partially-materialised
      // recovery state.
      const match = Object.values(s.draftsById).find(
        (d) =>
          d.promotedTo === null &&
          d.materializedTo?.workspaceId === workspaceIdForPane &&
          d.materializedTo.paneId === pane.pane_id,
      );
      if (!match || !match.materializedTo) return null;
      return {
        draftId: match.draftId,
        threadId: match.threadId,
        paneId: match.materializedTo.paneId,
        workspaceId: match.materializedTo.workspaceId,
        provider: match.provider,
        model: match.model,
        permissionMode: match.permissionMode,
        effort: match.effort,
        contextWindow: match.contextWindow,
      };
    }),
  );
  const markDraftPromoted = useChatDraftStore((s) => s.markPromoted);
  const clearDraft = useChatDraftStore((s) => s.clearDraft);
  // A pane is "home-rooted" if its workspace's project_root matches
  // the cached $HOME. This replaces the legacy `workspace_type ===
  // "home"` check since the Home singleton was retired in Stage B of
  // the Home rework.
  const homeDir = useHomeDir();
  const workspaceProjectRoot = useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) =>
        w.workspace_id === (workspaceIdForPane ?? s.appState!.active_workspace_id),
    );
    return ws?.project_root ?? ws?.cwd ?? null;
  });
  const isHomeWorkspace =
    homeDir !== null && workspaceProjectRoot === homeDir;
  // The pane workspace's actual checked-out branch (from the workspace
  // snapshot's git watcher). Thread Scope prefers this over the branch
  // picker's main/master heuristic for the "current checkout" display,
  // since it's the real answer to "which branch am I on?".
  const paneWorkspaceBranch = useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) =>
        w.workspace_id === (workspaceIdForPane ?? s.appState!.active_workspace_id),
    );
    return ws?.git_branch ?? null;
  });
  const setShowNewWorkspaceDialog = useUIStore(
    (s) => s.setShowNewWorkspaceDialog,
  );

  const ensureThread = useAgentChatStore((s) => s.ensureThread);
  const setInputDraft = useAgentChatStore((s) => s.setInputDraft);
  const setStoreModel = useAgentChatStore((s) => s.setModel);
  const setStorePermissionMode = useAgentChatStore((s) => s.setPermissionMode);
  const setSessionLaunchMode = useAgentChatStore(
    (s) => s.setSessionLaunchMode,
  );
  const removeStagedAttachment = useAgentChatStore(
    (s) => s.removeStagedAttachment,
  );
  const addStagedAttachment = useAgentChatStore((s) => s.addStagedAttachment);
  const updateStagedAttachment = useAgentChatStore(
    (s) => s.updateStagedAttachment,
  );
  const clearStagedAttachments = useAgentChatStore(
    (s) => s.clearStagedAttachments,
  );
  const setStoreEffort = useAgentChatStore((s) => s.setEffort);
  const setStoreContextWindow = useAgentChatStore((s) => s.setContextWindow);
  const setStoreResumeCursor = useAgentChatStore((s) => s.setResumeCursor);
  const setStoreMode = useAgentChatStore((s) => s.setMode);
  const setStoreModePriorPermissionMode = useAgentChatStore(
    (s) => s.setModePriorPermissionMode,
  );
  const setStoreHasDebugActivity = useAgentChatStore(
    (s) => s.setHasDebugActivity,
  );
  const setStoreDebugActivityResolved = useAgentChatStore(
    (s) => s.setDebugActivityResolved,
  );
  // Skills registry for send-time `/skill-name` token resolution. The
  // store dedupes loads behind a 60s TTL, so reading without a load
  // call is fine — Composer's slash-popup effect ensures the registry
  // is hydrated by the time the user picks a skill.
  // `selectActiveSkills` filters out user-disabled skills so a
  // disabled `/release` token in the textarea is treated as plain
  // prose, not silently injected.
  const skillsRegistry = useSkillsStore(selectActiveSkills);
  const migrateThreadId = useAgentChatStore((s) => s.migrateThreadId);
  const appendUserMessage = useAgentChatStore((s) => s.appendUserMessage);
  const removeUserMessageByNonce = useAgentChatStore(
    (s) => s.removeUserMessageByNonce,
  );
  const markRequestResponding = useAgentChatStore(
    (s) => s.markRequestResponding,
  );
  const markRequestResolved = useAgentChatStore(
    (s) => s.markRequestResolved,
  );

  // Chat-side capabilities for the active provider. `null` until the
  // refresh hook resolves (or when the backend errors — pickers render
  // a disabled "unavailable" state in that case).
  const capabilities = useProviderCapabilities((s) =>
    selectCapabilities(s, provider),
  );

  const slice = useAgentChatStore((s) =>
    threadId ? s.threads[threadId] : null,
  );
  const draft = slice?.inputDraft ?? "";
  const messages = slice?.messages ?? EMPTY_MESSAGES;
  const streaming = slice?.streaming ?? false;

  // Warm the MCP servers once when a fresh (empty) chat pane mounts, so
  // the prime cost overlaps the user composing rather than blocking the
  // first `agent_chat_start_session`. Fire-and-forget; guarded so it runs
  // at most once per pane mount and only for a not-yet-started thread.
  const primedMcpRef = useRef(false);
  useEffect(() => {
    if (primedMcpRef.current) return;
    if (messages.length > 0) return;
    primedMcpRef.current = true;
    void primeChatMcp().catch(() => {
      /* best-effort */
    });
  }, [messages.length]);
  // Dead-run detection (issue #154): the stall notice + interrupted /
  // Continue affordances read straight off the thread slice.
  const stalled = slice?.stalled ?? null;
  const interrupted = slice?.interrupted ?? false;

  // Subagent drill-in view state (locked decision 3): the pane swaps its
  // transcript body for a read-only sub-transcript and its sub-header for
  // a breadcrumb, but the composer stays wired to the parent thread. Kept
  // as local state; entering is triggered from the orchestration card, Esc
  // returns.
  const [enteredSubagentId, setEnteredSubagentId] = useState<string | null>(
    null,
  );
  const enteredSubagent = useMemo(
    () =>
      enteredSubagentId ? findSubagentView(messages, enteredSubagentId) : null,
    [messages, enteredSubagentId],
  );
  // The subagent vanished (thread switch / hydrate) — fall back to the
  // orchestrator so we never render a dangling breadcrumb.
  useEffect(() => {
    if (enteredSubagentId && !enteredSubagent) setEnteredSubagentId(null);
  }, [enteredSubagentId, enteredSubagent]);
  // Approval requests that bubbled from the entered subagent, mirrored
  // into the drill-in (locked decision 4).
  const enteredSubagentRequests = useMemo<PermissionRequestItem[]>(() => {
    if (!enteredSubagentId) return EMPTY_REQUESTS;
    return messages.filter(
      (m): m is PermissionRequestItem =>
        m.kind === "permission_request" && m.subagent_id === enteredSubagentId,
    );
  }, [messages, enteredSubagentId]);
  const handleEnterSubagent = useCallback((subagentId: string) => {
    setEnteredSubagentId(subagentId);
  }, []);
  const handleExitSubagent = useCallback(() => {
    setEnteredSubagentId(null);
  }, []);
  // Esc leaves the drill-in and returns to the orchestrator.
  useEffect(() => {
    if (!enteredSubagentId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setEnteredSubagentId(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enteredSubagentId]);
  // Scoped to this pane's own subtree (not `document`) so split panes each
  // jump within their own transcript. Plain DOM query rather than the
  // MessageScroller engine's `scrollToMessage`: the docked bar lives
  // outside the scroller's provider tree, and MessageList renders every
  // slot (no windowing — see docs/features/agent-chat.md "Transcript
  // scroller"), so a direct query by `data-subagent-card` always finds the
  // node.
  const paneRootRef = useRef<HTMLDivElement>(null);
  // Send → jump-to-latest (catch-up on send). An incrementing counter
  // forwarded to the transcript; bumping it snaps the viewport to the
  // bottom and re-pins following-bottom mode. Sending a new prompt must
  // always bring the reader to the latest content and keep them on the
  // stream, even if they had scrolled up into history — see
  // `ScrollToBottomOnSend` in MessageList.tsx.
  const [scrollToBottomSignal, setScrollToBottomSignal] = useState(0);
  const requestScrollToBottom = useCallback(
    () => setScrollToBottomSignal((n) => n + 1),
    [],
  );
  // Live handles for an in-flight jump (rAF settle loop + highlight
  // timeout), cancelled on a re-jump, a thread switch, and unmount — the
  // same hygiene as MessageList's / MessageTrail's own settle loops.
  const jumpFrameRef = useRef<number | null>(null);
  const jumpHighlightRef = useRef<{
    timer: number;
    card: HTMLElement;
  } | null>(null);
  const cancelJump = useCallback(() => {
    if (jumpFrameRef.current != null) {
      cancelAnimationFrame(jumpFrameRef.current);
      jumpFrameRef.current = null;
    }
    if (jumpHighlightRef.current != null) {
      window.clearTimeout(jumpHighlightRef.current.timer);
      jumpHighlightRef.current.card.classList.remove(
        "subagent-card-highlight",
      );
      jumpHighlightRef.current = null;
    }
  }, []);
  // Cleanup-only effect: cancels any in-flight jump when the thread
  // changes and on unmount.
  useEffect(() => cancelJump, [cancelJump, threadId]);
  const handleJumpToSubagentCard = useCallback(
    (cardId: string) => {
      const root = paneRootRef.current;
      if (!root) return;
      const viewport = root.querySelector<HTMLElement>(
        '[data-slot="message-scroller-viewport"]',
      );
      if (!viewport) return;
      const findCard = () => {
        const nodes = viewport.querySelectorAll<HTMLElement>(
          "[data-subagent-card]",
        );
        for (const node of Array.from(nodes)) {
          if (node.dataset.subagentCard === cardId) return node;
        }
        return null;
      };
      let card = findCard();
      if (!card) return;

      // A jump already in flight (or a lingering highlight on another
      // card) is superseded by this one.
      cancelJump();

      card.classList.add("subagent-card-highlight");
      jumpHighlightRef.current = {
        card,
        timer: window.setTimeout(() => {
          jumpHighlightRef.current?.card.classList.remove(
            "subagent-card-highlight",
          );
          jumpHighlightRef.current = null;
        }, JUMP_HIGHLIGHT_MS),
      };

      let frames = 0;
      let stableFrames = 0;
      const settle = () => {
        jumpFrameRef.current = null;
        if (!card?.isConnected) card = findCard();
        if (!card) return;
        const top =
          card.getBoundingClientRect().top -
          viewport.getBoundingClientRect().top;
        const delta = top - JUMP_SCROLL_MARGIN_PX;
        if (Math.abs(delta) > JUMP_SETTLE_TOLERANCE_PX) {
          viewport.scrollTop += delta;
          stableFrames = 0;
        } else {
          stableFrames += 1;
        }
        frames += 1;
        if (
          stableFrames < JUMP_SETTLE_STABLE_FRAMES &&
          frames < JUMP_SETTLE_MAX_FRAMES
        ) {
          jumpFrameRef.current = requestAnimationFrame(settle);
        }
      };
      jumpFrameRef.current = requestAnimationFrame(settle);
    },
    [cancelJump],
  );
  const activeTurnId = slice?.activeTurnId ?? null;
  const model = slice?.model ?? null;
  const permissionMode =
    slice?.permissionMode ?? DEFAULT_THREAD_PERMISSION_MODE;
  const effort = slice?.effort ?? null;
  const contextWindow = slice?.contextWindow ?? null;
  const mode = slice?.mode ?? "default";
  const hasDebugActivity = slice?.hasDebugActivity ?? false;
  const debugActivityResolved = slice?.debugActivityResolved ?? false;
  const stagedAttachments = slice?.stagedAttachments ?? EMPTY_ATTACHMENTS;
  const activeModel = selectModel(capabilities, model);
  const effortLabelMap = capabilities?.effort_label_map ?? {};
  const permissionModes = capabilities?.permission_modes ?? null;
  const ultrathinkInBodyText = hasUltrathinkInBodyText(draft);

  // When capabilities arrive (or change), reset the thread's
  // permissionMode if it's orphaned for the active provider. Seeds
  // the default when the slice has no mode yet. Idempotent — fires
  // only when the current value actually needs to change.
  useEffect(() => {
    if (!threadId) return;
    if (!capabilities) return;
    const plan = planCapabilityCompatReset({
      capabilities,
      currentPermissionMode: permissionMode,
    });
    if (plan.resetPermissionMode !== undefined) {
      // null is a legitimate reset value (provider with no modes).
      setStorePermissionMode(
        threadId,
        plan.resetPermissionMode ?? DEFAULT_THREAD_PERMISSION_MODE,
      );
    }
  }, [threadId, capabilities, permissionMode, setStorePermissionMode]);

  // Seed the slice's picker config from the persisted session row
  // whenever the slice exists (threadId set) but has no model yet.
  // Three paths land in this state:
  //   (a) app restart — the pane snapshot still carries a thread_id,
  //       but the in-memory store was wiped, so `ensureThread` below
  //       creates a fresh empty slice. THIS is the restart bug: the
  //       DB row holds the user's chosen model / effort / context /
  //       permission mode + the durable `sdk_session_id`, none of
  //       which live in the wiped store.
  //   (b) resume from session history — `hydrateThread` rebuilds the
  //       transcript from persisted events, none of which carry the
  //       chosen model.
  //   (c) any future flow that pre-creates a slice without seeding
  //       the model (e.g. silent restart on a thread that never got
  //       a model assigned).
  //
  // We fetch `agent_chat_get_session(threadId)` (which — unlike the
  // history list — returns rows whose `sdk_session_id` is still null)
  // and seed model/effort/contextWindow/permissionMode from it,
  // falling back to the provider default only when the row (or its
  // model column) is absent. Fetching FIRST and seeding once avoids
  // flashing the Opus default and then swapping to the persisted
  // model. `resumeCursor` is restored from `sdk_session_id` so a
  // picker-triggered silent restart resumes the SDK session instead
  // of starting fresh.
  //
  // Race safety: gated on `model === null`, and we re-check the live
  // slice after the async fetch so a selection the user made while the
  // fetch was in flight is never clobbered. `seedAttemptedRef` (keyed
  // by thread id, released on teardown like the hydrate effect) keeps
  // StrictMode's double-mount from firing two fetches. Without this
  // seed, ReasoningPicker (which short-circuits on `!model`) renders
  // nothing and the user loses the effort / context-window picker.
  const seedAttemptedRef = useRef<string | null>(null);
  useEffect(() => {
    if (!threadId) return;
    if (model !== null) return;
    if (seedAttemptedRef.current === threadId) return;
    seedAttemptedRef.current = threadId;
    const seedThreadId = threadId;
    const seedProvider = provider;
    let cancelled = false;
    void (async () => {
      let record: AgentChatSessionRecord | null = null;
      try {
        record = await agentChatGetSession(seedThreadId);
      } catch (err) {
        // Soft-fail: fall through to the provider default so the
        // pickers still render. Log so it's debuggable.
        console.warn("[agent-chat] get-session on mount failed:", err);
      }
      if (cancelled) return;
      // Never overwrite a value the user picked while the fetch was in
      // flight — the picker handlers write straight to the slice. The
      // guard is PER FIELD (not a single `model`-only bail): a user can
      // change permission-mode / effort / context — or nothing but the
      // model — during the in-flight window, and each field must be
      // protected independently. A field is only seeded when the live
      // slice still holds its unseeded sentinel (the `emptySlice`
      // default), i.e. the user hasn't touched it. `!current` means the
      // slice doesn't exist yet, so seeding is unconditionally safe.
      const current = useAgentChatStore.getState().threads[seedThreadId];
      // A model pick runs the compat planner, which OWNS effort/context
      // for the new model — so if the user picked a model mid-flight we
      // must not re-seed model/effort/context from the (now stale) row.
      const modelUntouched = !current || current.model === null;
      if (modelUntouched) {
        setStoreModel(
          seedThreadId,
          record?.model ?? defaultModelForProvider(seedProvider),
        );
      }
      if (record) {
        if (modelUntouched) {
          if (record.effort != null && (!current || current.effort === null))
            setStoreEffort(seedThreadId, record.effort);
          if (
            record.context_window != null &&
            (!current || current.contextWindow === null)
          )
            setStoreContextWindow(seedThreadId, record.context_window);
        }
        if (
          record.permission_mode != null &&
          (!current ||
            current.permissionMode === DEFAULT_THREAD_PERMISSION_MODE)
        )
          setStorePermissionMode(seedThreadId, record.permission_mode);
        // Resume cursor is independent of the model pick, so seed it even
        // when the user changed the model mid-flight — otherwise a later
        // picker-triggered restart would start fresh instead of resuming.
        if (!current || current.resumeCursor == null)
          setStoreResumeCursor(
            seedThreadId,
            record.sdk_session_id ? { resume: record.sdk_session_id } : null,
          );
      }
    })();
    return () => {
      cancelled = true;
      // Release the attempt marker on teardown so StrictMode's
      // mount→unmount→remount cycle re-seeds on the real mount instead
      // of bailing on a stale marker (mirrors hydrateAttemptedRef).
      if (seedAttemptedRef.current === threadId) {
        seedAttemptedRef.current = null;
      }
    };
  }, [
    threadId,
    model,
    provider,
    setStoreModel,
    setStoreEffort,
    setStoreContextWindow,
    setStorePermissionMode,
    setStoreResumeCursor,
  ]);

  // Resolve the session-start timestamp for the D2 transcript marker.
  // The persisted sessions list is the only place a chat's wall-clock
  // creation time lives (ChatViewItems carry only a monotonic seq), so
  // we look the current thread up there and hand ChatTranscript the
  // parsed `created_at`. A missing workspace/thread, no matching record,
  // an unparseable timestamp, or a failed fetch all clear it back to
  // `undefined` — the marker then renders the plain divider.
  useEffect(() => {
    if (!workspaceIdForPane || !threadId) {
      setSessionStartedAt(undefined);
      return;
    }
    let cancelled = false;
    agentChatListSessions(workspaceIdForPane, cwd)
      .then((records) => {
        if (cancelled) return;
        const record = records.find((r) => r.thread_id === threadId);
        const parsed = record ? Date.parse(record.created_at) : Number.NaN;
        setSessionStartedAt(Number.isNaN(parsed) ? undefined : parsed);
      })
      .catch(() => {
        if (!cancelled) setSessionStartedAt(undefined);
      });
    return () => {
      cancelled = true;
    };
  }, [workspaceIdForPane, threadId, cwd]);

  // Stage 6 grep-on-chat-open: when the pane mounts (or the project
  // root changes), background-check whether the workspace already
  // contains `CODEMUX_DEBUG` markers from a previous debug session.
  // The result drives the cleanup banner / exit dialog. Failures are
  // soft: we treat them as "no markers" so a missing ripgrep binary
  // can't pin the cleanup affordance on forever.
  useEffect(() => {
    if (!threadId) return;
    if (!workspaceProjectRoot) return;
    let cancelled = false;
    setStoreDebugActivityResolved(threadId, false);
    grepCountPattern(workspaceProjectRoot, "CODEMUX_DEBUG")
      .then((count) => {
        if (cancelled) return;
        setStoreHasDebugActivity(threadId, count > 0);
        setStoreDebugActivityResolved(threadId, true);
      })
      .catch((err) => {
        console.warn("[agent-chat] debug-marker grep failed:", err);
        if (cancelled) return;
        setStoreHasDebugActivity(threadId, false);
        setStoreDebugActivityResolved(threadId, true);
      });
    return () => {
      cancelled = true;
    };
  }, [
    threadId,
    workspaceProjectRoot,
    setStoreHasDebugActivity,
    setStoreDebugActivityResolved,
  ]);

  // Subscribe to provider events for this thread. The handler reads
  // store actions via `getState()` so it stays stable across
  // re-renders — otherwise we'd rebind the Tauri listener on every
  // keystroke.
  const handleEvent = useCallback((payload: AgentChatEventPayload) => {
    useAgentChatStore.getState().applyEvent(payload.thread_id, payload.event);
  }, []);
  useAgentChatEvents(threadId, handleEvent);

  // Hydrate on (re)mount when the pane attaches to a thread that
  // already has a server-side transcript.
  //
  // The provider event broadcaster is live-only (bounded channel, no
  // replay — see docs/features/agent-chat.md). When the user switches
  // workspaces the entire inactive workspace's pane tree unmounts
  // (workspace-main.tsx returns null for non-active workspaces), so
  // the `useAgentChatEvents` listener detaches and any provider events
  // emitted while the pane is gone are dropped on the frontend side.
  // The backend still persists transcript-affecting events to SQLite
  // via `forward_event` in `commands/agent_chat.rs`, but nothing pulled
  // that history back into the in-memory Zustand slice on remount —
  // so the user came back to a chat showing only their optimistic
  // user message with no agent reply, even though the agent had run
  // (file changes proved it).
  //
  // We pull the persisted transcript on every mount that lands with a
  // truthy `threadId`, replay it through the same pure reducer the
  // live stream uses, and only call `hydrateThread` if the replayed
  // transcript is longer than the in-memory slice — otherwise the
  // local state is at least as fresh as disk and a hydrate would
  // clobber events that haven't been persisted yet (the persistence
  // path is async, so live state can briefly lead disk).
  //
  // `hydrateAttemptedRef` is keyed by thread id so a thread switch
  // (resume → new thread id, recovery → adopted orphan thread id)
  // re-triggers hydrate, but a stable thread id within one mount only
  // hydrates once.
  const hydrateAttemptedRef = useRef<string | null>(null);
  useEffect(() => {
    if (!threadId) return;
    if (hydrateAttemptedRef.current === threadId) return;
    hydrateAttemptedRef.current = threadId;
    let cancelled = false;
    void (async () => {
      try {
        const payloads = await agentChatListMessages(threadId);
        if (cancelled) return;
        if (payloads.length === 0) return;
        const replayed = replayPayloads(payloads);
        const slice = useAgentChatStore.getState().threads[threadId];
        const localCount = slice?.messages.length ?? 0;
        // Guard against clobbering live state: only hydrate when disk
        // has strictly more rendered messages than memory. Equal /
        // shorter means the live stream kept up.
        if (replayed.messages.length <= localCount) return;
        // Ask the backend whether this thread's turn is still in flight.
        // Ordering matters: messages are fetched FIRST, liveness SECOND —
        // if the turn settles between the two calls the `turn_completed`
        // row is already in `payloads`, so `runLive` can only ever be a
        // conservative false, never a stale "alive". Any invoke error
        // (missing command, transient failure) degrades to `false`, i.e.
        // today's heuristic behavior. A live turn means the persisted
        // tail's missing terminal event is expected, not an interrupt —
        // `runLive` suppresses the false Run-interrupted divider.
        const runLive = await agentChatTurnActive(provider, threadId).catch(
          () => false,
        );
        if (cancelled) return;
        useAgentChatStore
          .getState()
          .hydrateThread(threadId, payloads, { runLive });
      } catch (err) {
        // Soft-fail: if hydrate fails, the user still sees whatever
        // the live stream brings in. Log so it's debuggable.
        console.warn("[agent-chat] hydrate-on-mount failed:", err);
      }
    })();
    return () => {
      cancelled = true;
      // Release the attempt marker on teardown. Without this, React
      // StrictMode's dev-only mount→unmount→remount cycle cancels the
      // first invocation's in-flight fetch and the second invocation
      // bails on the marker — so a dev build never hydrates. The
      // remount re-runs the effect with a clean slate; in production
      // (single mount) this cleanup only runs on real unmounts, where
      // the ref dies anyway.
      if (hydrateAttemptedRef.current === threadId) {
        hydrateAttemptedRef.current = null;
      }
    };
  }, [threadId]);

  // Start a session on mount if the pane doesn't already have one.
  const startAttempted = useRef(false);
  // Tracks whether we've already attempted to recover a partial
  // materialise on this mount. Unlike `startAttempted` (which resets
  // on failure to allow a hot retry), recovery is a one-shot per
  // mount: if it fails, the draft stays in SendFailed state and the
  // user can retry by re-opening the workspace.
  const recoveryAttempted = useRef(false);
  useEffect(() => {
    if (threadId) {
      ensureThread(threadId);
      return;
    }
    // Stage C race fix: if a promoted draft owns this workspace, it
    // already started the session under `draft.threadId`. Just adopt
    // that id — don't spin up a second session.
    if (promotedDraftThreadId) {
      setThreadId(promotedDraftThreadId);
      ensureThread(promotedDraftThreadId);
      return;
    }
    // Recovery: the draft completed step 2 (workspace + pane) but
    // step 3 (start_session) or step 4 (send_turn) failed. Adopt
    // the orphan thread id (which already has the user's optimistic
    // message in its slice) and finish what materialize started.
    if (recoveryDraft && !recoveryAttempted.current) {
      if (starting) return;
      if (!cwd) return;
      recoveryAttempted.current = true;
      startAttempted.current = true;
      setStarting(true);
      const recoveryLaunchMode =
        provider === "opencode"
          ? null
          : (recoveryDraft.permissionMode ??
            capabilityDefaults(
              provider,
              recoveryDraft.model ?? defaultModelForProvider(provider),
            ).permissionMode);
      const recoveryDisplayMode =
        recoveryLaunchMode ?? DEFAULT_THREAD_PERMISSION_MODE;
      const startInput = {
        thread_id: recoveryDraft.threadId,
        cwd,
        model: recoveryDraft.model,
        resume_cursor: null,
        permission_mode: recoveryLaunchMode,
        additional_directories: [],
        env: null,
      };
      agentChatStartSession(pane.pane_id, provider, startInput)
        .then((id) => {
          setThreadId(id);
          ensureThread(id);
          if (recoveryDraft.model !== null) {
            setStoreModel(id, recoveryDraft.model);
          } else {
            setStoreModel(id, defaultModelForProvider(provider));
          }
          setStorePermissionMode(id, recoveryDisplayMode);
          setSessionLaunchMode(id, recoveryLaunchMode);
          // Mark the draft as promoted so subsequent mounts take the
          // existing promotedDraftThreadId branch above instead of
          // re-attempting recovery.
          markDraftPromoted(recoveryDraft.draftId, {
            workspaceId: recoveryDraft.workspaceId,
            paneId: recoveryDraft.paneId,
            threadId: id,
          });
          // Sweep the now-promoted draft on the same 5s grace window
          // the success path uses (DraftChatSurface.tsx, preset-bar.tsx).
          const draftIdToClear = recoveryDraft.draftId;
          setTimeout(() => clearDraft(draftIdToClear), 5000);
        })
        .catch((err) => {
          toast.error(`Failed to recover chat session: ${err}`);
          // Leave the draft in SendFailed state. The user can re-open
          // the workspace to retry, or close it and start over.
        })
        .finally(() => setStarting(false));
      return;
    }
    if (starting || startAttempted.current) return;
    if (!cwd) return;
    startAttempted.current = true;
    setStarting(true);
    const localThreadId = `chat-${pane.pane_id}-${Date.now()}`;
    // For a brand-new thread with no slice yet, use this provider's
    // defaults. OpenCode deliberately launches with a null permission
    // mode; feeding it Claude's bypass token would persist invalid
    // cross-provider configuration.
    const defaultModel = defaultModelForProvider(provider);
    const startLaunchMode = capabilityDefaults(
      provider,
      defaultModel,
    ).permissionMode;
    const startDisplayMode =
      startLaunchMode ?? DEFAULT_THREAD_PERMISSION_MODE;
    const startInput = {
      thread_id: localThreadId,
      cwd,
      model: null,
      resume_cursor: null,
      permission_mode: startLaunchMode,
      additional_directories: [],
      env: null,
    };
    agentChatStartSession(pane.pane_id, provider, startInput)
      .then((id) => {
        setThreadId(id);
        ensureThread(id);
        setStoreModel(id, defaultModel);
        setStorePermissionMode(id, startDisplayMode);
        setSessionLaunchMode(id, startLaunchMode);
      })
      .catch((err) => {
        toast.error(`Failed to start chat session: ${err}`);
        startAttempted.current = false;
      })
      .finally(() => setStarting(false));
  }, [
    threadId,
    promotedDraftThreadId,
    recoveryDraft,
    starting,
    pane.pane_id,
    provider,
    cwd,
    ensureThread,
    setStoreModel,
    setStorePermissionMode,
    setSessionLaunchMode,
    markDraftPromoted,
    clearDraft,
  ]);

  const handleSubmit = useCallback((
    textOverride?: string,
    options?: { continueRun?: boolean },
  ) => {
    // Synchronous ref check BEFORE any React state reads — closes the
    // same-tick race that the `useState` guard can't (captured closure
    // sees the pre-set snapshot when two Enter presses fire in one
    // tick). Refs mutate synchronously; the second call sees
    // `sendInFlightRef.current === true` and bails.
    if (sendInFlightRef.current) return;
    if (!threadId) return;
    // `textOverride` is the one-click "Continue run" path (issue #154):
    // it sends fixed text through this same machinery so the optimistic
    // bubble, rollback, and backend auto-resume all behave identically to
    // a manual send. A string is only ever passed by `handleContinueRun`;
    // the Composer always calls `onSubmit()` with no args.
    //
    // `continueRun` is the recovery send: it must NOT consume the composer.
    // The user may be mid-way through typing their next message with staged
    // attachments; resuming a dead run should leave that draft + its chips
    // untouched and send a plain "Continue" with no images.
    const isContinue = options?.continueRun === true;
    const rawText = (typeof textOverride === "string" ? textOverride : draft).trim();
    if (!rawText) return;
    // Snapshot the pre-append interrupted state + composer draft so a failed
    // send can restore both (the optimistic append clears `interrupted` and
    // the store's `appendUserMessage` resets `inputDraft`).
    const preSendInterrupted =
      useAgentChatStore.getState().threads[threadId]?.interrupted ?? false;
    const composerDraft = draft;
    const plan = planSubmit({ rawText, provider, effort });
    sendInFlightRef.current = true;
    setIsSending(true);
    void (async () => {
      // Stage 7 — re-fetch any GitHub-kind chip whose detail is older
      // than STALE_ATTACHMENT_THRESHOLD_MS so the agent sees fresh
      // state (closed → reopened, new comments, label flips). Files
      // are read fresh by the agent itself via the Read tool, so they
      // don't need this. Failures here are non-fatal: we log and
      // proceed with the stale resolved content rather than abort the
      // turn — better to send something than to hard-block on a flaky
      // gh call.
      try {
        // A Continue send carries no attachments, so skip the stale refresh.
        const preStale = isContinue
          ? undefined
          : useAgentChatStore.getState().threads[threadId];
        const staleList = (preStale?.stagedAttachments ?? []).filter((a) => {
          if (a.kind !== "issue" && a.kind !== "pr") return false;
          if (a.metadata.isLoading) return false;
          const fetchedAt = a.metadata.fetchedAt ?? 0;
          return Date.now() - fetchedAt > STALE_ATTACHMENT_THRESHOLD_MS;
        });
        if (staleList.length > 0) {
          const repoPath = cwd ?? "";
          await Promise.all(
            staleList.map(async (att) => {
              try {
                if (att.kind === "issue") {
                  const num = Number.parseInt(att.ref.replace(/^#/, ""), 10);
                  if (!Number.isFinite(num)) return;
                  const detail = await getGithubIssueByPath(repoPath, num);
                  updateStagedAttachment(threadId, att.id, {
                    resolvedContent: buildIssueResolvedContent(detail),
                    metadata: {
                      ...att.metadata,
                      label: `#${detail.number} ${detail.title}`,
                      state: (detail.state.toLowerCase() === "closed"
                        ? "closed"
                        : "open") as "open" | "closed",
                      fetchedAt: Date.now(),
                    },
                  });
                } else {
                  const num = Number.parseInt(att.ref.replace(/^!/, ""), 10);
                  if (!Number.isFinite(num)) return;
                  const fullDiff = att.metadata.expandFullDiff === true;
                  const [detail, diff] = await Promise.all([
                    getGithubPrByPath(repoPath, num),
                    getGithubPrDiffByPath(repoPath, num, fullDiff),
                  ]);
                  const detailUpper = detail.state.toUpperCase();
                  const resolvedState: "open" | "closed" | "merged" | "draft" =
                    detailUpper === "MERGED"
                      ? "merged"
                      : detailUpper === "CLOSED"
                        ? "closed"
                        : detail.is_draft
                          ? "draft"
                          : "open";
                  updateStagedAttachment(threadId, att.id, {
                    resolvedContent: buildPrResolvedContent(detail, diff, {
                      fullDiff,
                    }),
                    metadata: {
                      ...att.metadata,
                      label: `#${detail.number} ${detail.title}`,
                      state: resolvedState,
                      fetchedAt: Date.now(),
                    },
                  });
                }
              } catch (err) {
                console.warn(
                  `[agent-chat] stale re-fetch failed for ${att.ref}`,
                  err,
                );
              }
            }),
          );
        }
      } catch (err) {
        console.warn("[agent-chat] stale re-fetch outer failure", err);
      }
      // Mode wrappers (Stage 4 onward) live SDK-side only — the
      // transcript stores the unwrapped (ultrathink-only) text so users
      // see what they typed, not the framing we layered on top.
      // Parse `/skill-name` tokens out of the raw text and resolve their
      // bodies against the skills registry. Unmatched tokens (typos, or
      // skills not in the registry) silently pass through as plain prose.
      const skillBodies = resolveSkillBodies(rawText, skillsRegistry);
      // Snapshot staged attachments AT submit time so the block we
      // inject reflects exactly what the user has staged. Reading from
      // the live store via getState() avoids stale closure bugs if a
      // chip resolved between the last render and Enter. Step 8 Stage
      // 2.1 — filter to attachments whose `@<basename>` token still
      // appears in the textarea (`rawText`); deleting a token excludes
      // its file from the prompt without an explicit chip-removal.
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      // A Continue send never carries attachments or images: it leaves the
      // user's staged chips intact for their real next message and sends plain
      // "Continue". An empty list makes every downstream builder a no-op.
      const liveAttachments = isContinue ? [] : (liveSlice?.stagedAttachments ?? []);
      const attachmentBlock = buildAttachmentBlock(
        activeAttachments(rawText, liveAttachments),
      );
      // Images travel as native multimodal content blocks at the SDK
      // layer, NOT inside the text body. They were staged to disk at
      // attach time (`agent_chat_stage_image`), so the turn carries only
      // `{ path, media_type }` references — the raw bytes never marshal
      // across IPC as a JSON `number[]` (the multi-minute first-send
      // stall). The `data:` URLs for the optimistic bubble are built from
      // the still-in-memory bytes so the thumbnail shows immediately.
      const imageDisplaySources = buildImageDisplaySources(liveAttachments);
      const sdkText = applyAllPrefixes(
        rawText,
        mode,
        effort,
        skillBodies,
        attachmentBlock,
      );
      // Optimistic append carries a client nonce so a `turn_queued`
      // event can reconcile THIS exact bubble (grey it out) instead of
      // duplicating it, and so an outright RPC failure can roll it back
      // (fixes the pre-queue orphan-bubble bug).
      const clientNonce =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `nonce-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      appendUserMessage(threadId, plan.text, clientNonce, imageDisplaySources);
      // Catch up to the latest on send: snap the transcript to the bottom
      // and re-pin following-bottom, even if the reader had scrolled up
      // into history. Bumped in the same commit as the optimistic append
      // so the jump lands on the freshly-added user bubble.
      requestScrollToBottom();
      if (isContinue) {
        // Preserve the composer: `appendUserMessage` reset `inputDraft` to ""
        // as a side effect, so restore the user's in-progress text. Staged
        // chips are left untouched (no `clearStagedAttachments` below).
        setInputDraft(threadId, composerDraft);
      }
      // Images stage at attach time, so normally already done; await only
      // stragglers (paste-then-Enter). Re-read fresh store state afterward
      // because staging patches land on new attachment objects the
      // submit-time snapshot doesn't reflect.
      const imageIds = imageAttachmentIds(liveAttachments);
      if (imageIds.length > 0) await awaitImageStaging(imageIds);
      const freshAttachments = isContinue
        ? []
        : useAgentChatStore.getState().threads[threadId]?.stagedAttachments ?? [];
      const unstaged = unstagedImageAttachments(freshAttachments);
      if (unstaged.length > 0) {
        // A staged image never landed (upload failed). The raw-bytes
        // fallback is gone, so block the send rather than silently drop
        // the image: roll the optimistic bubble back, restore the
        // composer, and leave the chip (with its error) for the user.
        removeUserMessageByNonce(threadId, clientNonce, preSendInterrupted);
        setInputDraft(threadId, rawText);
        toast.error(
          "An attached image failed to upload — remove it and try again.",
        );
        sendInFlightRef.current = false;
        setIsSending(false);
        return;
      }
      const imageRefs = buildImageRefs(freshAttachments);
      if (!isContinue) {
        // Clear chips per-turn (matches the inputDraft = "" reset that
        // appendUserMessage already does for the textarea).
        clearStagedAttachments(threadId);
      }
      const input = {
        thread_id: threadId,
        text: sdkText,
        images: imageRefs,
        model_override: null,
        effort_override: plan.effortOverride,
        client_nonce: clientNonce,
      };
      try {
        // Backend is authoritative: if a turn was already in flight the
        // send is QUEUED (result.queued_id set) rather than rejected. In
        // that case the `turn_queued` event greys the optimistic bubble;
        // we do nothing extra here. An immediate start keeps the bubble
        // as a normal user message.
        await agentChatSendTurn(provider, input);
      } catch (err) {
        // Genuine failure — roll back the optimistic bubble so no orphan
        // is left. `restoreInterrupted` re-arms the "Run interrupted"
        // divider + Continue chip when the failed send was itself a resume
        // of an interrupted thread (the optimistic append cleared the flag),
        // so one failed click never strands the run with no recovery
        // affordance.
        removeUserMessageByNonce(threadId, clientNonce, preSendInterrupted);
        // Restore the composer text so the user doesn't lose it. A Continue
        // send never owned the draft (it preserved the user's own in-progress
        // text above), so leave that intact rather than stamping "Continue".
        if (!isContinue) {
          setInputDraft(threadId, rawText);
        }
        toast.error(`Failed to send turn: ${err}`);
        sendInFlightRef.current = false;
        setIsSending(false);
      }
    })();
  }, [
    threadId,
    draft,
    provider,
    effort,
    mode,
    cwd,
    skillsRegistry,
    appendUserMessage,
    removeUserMessageByNonce,
    setInputDraft,
    clearStagedAttachments,
    updateStagedAttachment,
    requestScrollToBottom,
  ]);

  /** One-click "Continue run" (issue #154): resume an interrupted run by
   *  sending the fixed text "Continue" through the normal send path — the
   *  same thing users type manually today. Routing through
   *  `agentChatSendTurn` means the backend's `ensure_live_session` choke
   *  point transparently rebuilds the dead session with the resume cursor.
   *  Appending the optimistic bubble clears the interrupted flag. */
  const handleContinueRun = useCallback(() => {
    handleSubmit("Continue", { continueRun: true });
  }, [handleSubmit]);

  /** Step 8 Stage 2 — orchestrates the chip lifecycle when the user
   *  picks a file from the `@` mention popup. The Composer has
   *  already stripped the `@<query>` token from the textarea by the
   *  time this fires, so we just stage the chip with isLoading=true,
   *  fire `read_file_for_attachment`, then patch the chip with the
   *  resolved content (or an error indicator if the read failed). */
  const handleAttachFile = useCallback(
    (match: FileMatch) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const filename = basename(match.path);
      addStagedAttachment(threadId, {
        id,
        kind: "file",
        ref: match.absolute_path,
        metadata: {
          label: filename,
          isLoading: true,
        },
      });
      void (async () => {
        try {
          const info = await readFileForAttachment(match.absolute_path, cwd);
          updateStagedAttachment(threadId, id, {
            resolvedContent: buildFileResolvedContent(info),
            metadata: {
              label: filename,
              lineCount: info.lineCount,
              bytes: info.bytes,
              isTruncated: info.truncated,
              isLoading: false,
              fetchedAt: Date.now(),
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label: filename,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 3 — folder counterpart to handleAttachFile.
   *  Stages with isLoading=true, fires read_folder_for_attachment
   *  with depth 2 (research-locked), then patches the chip with the
   *  rendered tree. */
  const handleAttachFolder = useCallback(
    (match: FolderMatch) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const folderName = basename(match.path);
      addStagedAttachment(threadId, {
        id,
        kind: "folder",
        ref: match.absolute_path,
        metadata: { label: folderName, isLoading: true },
      });
      void (async () => {
        try {
          const info = await readFolderForAttachment(match.absolute_path, cwd, 2);
          updateStagedAttachment(threadId, id, {
            resolvedContent: buildFolderResolvedContent(info),
            metadata: {
              label: folderName,
              isLoading: false,
              fetchedAt: Date.now(),
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label: folderName,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 4 — issue counterpart. Stages with isLoading=true,
   *  fires the cached detail fetch, then patches the chip with the
   *  fully resolved body + metadata (state, fetchedAt). Errors render
   *  the chip muted with the error string in the tooltip. */
  const handleAttachIssue = useCallback(
    (summary: GitHubIssue) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const initialState = (summary.state.toLowerCase() === "closed"
        ? "closed"
        : "open") as "open" | "closed";
      addStagedAttachment(threadId, {
        id,
        kind: "issue",
        ref: `#${summary.number}`,
        metadata: {
          label: `#${summary.number} ${summary.title}`,
          state: initialState,
          isLoading: true,
        },
      });
      void (async () => {
        try {
          // Fall back to the workspace cwd when project_root isn't on
          // hand; backend `_by_path` accepts either as long as it's a
          // git checkout the user has gh access to.
          const repoPath = cwd ?? "";
          const detail = await getGithubIssueByPath(repoPath, summary.number);
          updateStagedAttachment(threadId, id, {
            resolvedContent: buildIssueResolvedContent(detail),
            metadata: {
              label: `#${detail.number} ${detail.title}`,
              state: (detail.state.toLowerCase() === "closed"
                ? "closed"
                : "open") as "open" | "closed",
              fetchedAt: Date.now(),
              isLoading: false,
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label: `#${summary.number} ${summary.title}`,
              state: initialState,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 5 — PR counterpart. Fetches detail + name-only
   *  diff in parallel; resolves the chip with both. The diff is
   *  capped at 100 KB on the Rust side so the prompt budget stays
   *  bounded even for monorepo migrations. */
  const handleAttachPr = useCallback(
    (summary: PullRequestInfo) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const upper = summary.state.toUpperCase();
      const initialState: "open" | "closed" | "merged" | "draft" =
        upper === "MERGED"
          ? "merged"
          : upper === "CLOSED"
            ? "closed"
            : summary.is_draft
              ? "draft"
              : "open";
      addStagedAttachment(threadId, {
        id,
        kind: "pr",
        ref: `!${summary.number}`,
        metadata: {
          label: `#${summary.number} ${summary.title}`,
          state: initialState,
          isLoading: true,
        },
      });
      void (async () => {
        try {
          const repoPath = cwd ?? "";
          // Detail + name-only diff fetched concurrently — both are
          // path-cached separately so a re-pick of the same PR
          // returns from cache after the first call.
          const [detail, diff] = await Promise.all([
            getGithubPrByPath(repoPath, summary.number),
            getGithubPrDiffByPath(repoPath, summary.number, false),
          ]);
          const detailUpper = detail.state.toUpperCase();
          const resolvedState: "open" | "closed" | "merged" | "draft" =
            detailUpper === "MERGED"
              ? "merged"
              : detailUpper === "CLOSED"
                ? "closed"
                : detail.is_draft
                  ? "draft"
                  : "open";
          updateStagedAttachment(threadId, id, {
            resolvedContent: buildPrResolvedContent(detail, diff),
            metadata: {
              label: `#${detail.number} ${detail.title}`,
              state: resolvedState,
              fetchedAt: Date.now(),
              isLoading: false,
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label: `#${summary.number} ${summary.title}`,
              state: initialState,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 7 — flip the `expandFullDiff` flag on a staged PR
   *  attachment and re-resolve its content with the matching diff
   *  shape. The chip's expand affordance calls this; the resolved
   *  body the agent sees swaps from `### Files changed (N)` to
   *  `### Diff (full)` (or vice-versa). The detail fetch is cached
   *  on the Rust side so the second call after a toggle is cheap;
   *  the diff fetch with `full=true` is uncached but capped at 100KB. */
  const handleToggleExpandPr = useCallback(
    (attachmentId: string) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      const att = liveSlice?.stagedAttachments.find((a) => a.id === attachmentId);
      if (!att || att.kind !== "pr") return;
      const num = Number.parseInt(att.ref.replace(/^!/, ""), 10);
      if (!Number.isFinite(num)) return;
      const currentLabel = att.metadata.label;
      const currentState = att.metadata.state;
      const newExpand = !att.metadata.expandFullDiff;
      updateStagedAttachment(threadId, attachmentId, {
        metadata: {
          ...att.metadata,
          expandFullDiff: newExpand,
          isLoading: true,
        },
      });
      void (async () => {
        try {
          const repoPath = cwd ?? "";
          const [detail, diff] = await Promise.all([
            getGithubPrByPath(repoPath, num),
            getGithubPrDiffByPath(repoPath, num, newExpand),
          ]);
          const detailUpper = detail.state.toUpperCase();
          const resolvedState: "open" | "closed" | "merged" | "draft" =
            detailUpper === "MERGED"
              ? "merged"
              : detailUpper === "CLOSED"
                ? "closed"
                : detail.is_draft
                  ? "draft"
                  : "open";
          updateStagedAttachment(threadId, attachmentId, {
            resolvedContent: buildPrResolvedContent(detail, diff, {
              fullDiff: newExpand,
            }),
            metadata: {
              label: `#${detail.number} ${detail.title}`,
              state: resolvedState,
              expandFullDiff: newExpand,
              fetchedAt: Date.now(),
              isLoading: false,
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, attachmentId, {
            metadata: {
              label: currentLabel,
              state: currentState,
              expandFullDiff: newExpand,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, updateStagedAttachment],
  );

  /** Step 8 Stage 6 — image attachment. Validates the MIME against
   *  the allowlist (png/jpeg/webp/gif), stages a chip with
   *  isLoading=true, then resolves with the decoded bytes. The
   *  resolved bytes are kept in-memory only (`resolvedImage` is
   *  excluded from persistence) so a session restart re-prompts the
   *  user to re-paste. */
  const handleAttachImage = useCallback(
    async (file: File) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const allowed = [
        "image/png",
        "image/jpeg",
        "image/webp",
        "image/gif",
      ];
      if (!allowed.includes(file.type)) {
        toast.error("Image type not supported", {
          description: `${file.type || "unknown type"} — supported: png, jpeg, webp, gif`,
        });
        return;
      }
      // Stage 7 — animated GIFs are silently rejected by Anthropic's
      // image API. Detect at attach time so the user sees a clear
      // toast instead of an opaque 400 at send time. Static GIFs
      // (single frame) pass through unchanged.
      let resolvedBuffer: ArrayBuffer | null = null;
      if (file.type === "image/gif") {
        try {
          resolvedBuffer = await file.arrayBuffer();
          if (detectAnimatedGif(resolvedBuffer)) {
            toast.error("Animated GIFs not supported", {
              description: "Save as PNG or JPEG instead.",
            });
            return;
          }
        } catch (err) {
          toast.error("Failed to read image", { description: String(err) });
          return;
        }
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const label = file.name || `pasted-image-${Date.now()}.png`;
      addStagedAttachment(threadId, {
        id,
        kind: "image",
        ref: `image:${id}`,
        metadata: {
          label,
          bytes: file.size,
          isLoading: true,
        },
      });
      try {
        const buffer = resolvedBuffer ?? (await file.arrayBuffer());
        const bytes = new Uint8Array(buffer);
        updateStagedAttachment(threadId, id, {
          resolvedImage: { mime: file.type, bytes },
          metadata: {
            label,
            bytes: file.size,
            isLoading: false,
            fetchedAt: Date.now(),
          },
        });
        // Stage the bytes to disk NOW (off the send path) so the first
        // send doesn't marshal them across IPC. Patches the chip with
        // `stagedImage` when it lands, or a chip error if it fails.
        beginImageStaging(threadId, id, bytes, file.type);
      } catch (err) {
        updateStagedAttachment(threadId, id, {
          metadata: {
            label,
            bytes: file.size,
            isLoading: false,
            error: String(err),
          },
        });
      }
    },
    [threadId, addStagedAttachment, updateStagedAttachment],
  );

  // Clear the optimistic send flag the moment the backend
  // acknowledges the turn via Running (streaming=true in the store).
  // For the degenerate case where Running and turn_completed batch
  // into the same render — so `streaming` appears to stay false from
  // the Composer's perspective — we also clear when `activeTurnId`
  // transitions non-null (another backend-ack signal) or when the
  // next render cycle completes without streaming flipping; the
  // sync-ref flip already prevented duplicate submits so the `ref`
  // stays correct either way.
  useEffect(() => {
    if (!isSending) return;
    if (streaming || activeTurnId != null) {
      sendInFlightRef.current = false;
      setIsSending(false);
    }
  }, [isSending, streaming, activeTurnId]);

  // Interrupt mechanic: the SDK's `query.interrupt()` causes its
  // async iterator to exit, so the underlying SDK query is dead after
  // an interrupt. The sidecar now recovers from that transparently —
  // like the reference multi-provider client, its next `send-turn`
  // rebuilds a resumed SDK query for the (surviving) session, so a bare
  // interrupt already lets subsequent turns land on a live query.
  // The Stop button still performs a deliberate hard reset: interrupt
  // for the immediate turn abort, then stop + start the session (fresh
  // thread id, resume cursor) so subsequent turns land on a known-good
  // SDK query and any wedged sidecar state is cleared. Transcript and
  // picker state persist via `migrateThreadId`.
  const handleStop = useCallback(() => {
    if (!threadId) return;
    if (restarting) return;
    const currentSlice = useAgentChatStore.getState().threads[threadId];
    if (!currentSlice) return;
    setRestarting(true);
    void (async () => {
      // Await the interrupt BEFORE tearing the session down. The provider
      // settles a user stop with a persisted `turn_completed{interrupted}`
      // (Claude via the sidecar's `session-ended`, Codex via the app-server's
      // `turn/completed`) that flows asynchronously. If we fired the interrupt
      // and immediately killed the child, that settlement could lose the race
      // and the stopped turn would hydrate as an unsettled — i.e. "Run
      // interrupted" — turn after a restart. Awaiting orders the interrupt
      // ahead of `stop_session`, whose graceful shutdown (EOF → grace → kill)
      // then gives the settlement time to reach the event bridge (which
      // persists from the broadcast independent of the child's liveness). An
      // interrupt failure is safe to swallow — the stop below tears down anyway.
      try {
        await agentChatInterruptTurn(provider, threadId, null);
      } catch {
        // Non-fatal: the teardown below still restarts the session.
      }
      try {
        await agentChatStopSession(provider, threadId);
      } catch (err) {
        console.warn("[agent-chat] stop_session during Stop failed", err);
      }
      if (!cwd) {
        setRestarting(false);
        return;
      }
      try {
        const newLocalThreadId = `chat-${pane.pane_id}-${Date.now()}`;
        const newId = await agentChatStartSession(
          pane.pane_id,
          provider,
          {
            thread_id: newLocalThreadId,
            cwd,
            model: currentSlice.model,
            resume_cursor: currentSlice.resumeCursor,
            permission_mode:
              provider === "opencode" ? null : currentSlice.permissionMode,
            effort: currentSlice.effort,
            context_window: currentSlice.contextWindow,
            additional_directories: [],
            env: null,
          },
        );
        migrateThreadId(threadId, newId);
        setThreadId(newId);
        setSessionLaunchMode(
          newId,
          provider === "opencode" ? null : currentSlice.permissionMode,
        );
      } catch (err) {
        toast.error(`Failed to restart session after stop: ${err}`);
      } finally {
        setRestarting(false);
        // Belt-and-braces: restart cleared any in-flight state, so
        // drop the local send guard.
        sendInFlightRef.current = false;
        setIsSending(false);
      }
    })();
  }, [
    threadId,
    provider,
    cwd,
    pane.pane_id,
    restarting,
    migrateThreadId,
    setSessionLaunchMode,
  ]);

  // Follow-up queueing: cancel a queued (not-yet-dispatched) turn. The
  // backend emits `queued_turn_cancelled`, which the reducer uses to
  // remove the greyed bubble — so we don't optimistically remove here.
  // On success we restore the cancelled text into the composer draft so
  // the user can edit and resend (prepended if the composer already has
  // content, to avoid clobbering an in-progress draft).
  const handleCancelQueued = useCallback(
    (queuedId: string, text: string) => {
      if (!threadId) return;
      const tid = threadId;
      void (async () => {
        try {
          await agentChatCancelQueuedTurn(provider, tid, queuedId);
          const current = useAgentChatStore.getState().threads[tid]?.inputDraft ?? "";
          setInputDraft(tid, current.trim().length > 0 ? `${text}\n${current}` : text);
        } catch (err) {
          toast.error(`Failed to cancel queued message: ${err}`);
        }
      })();
    },
    [threadId, provider, setInputDraft],
  );

  // Follow-up queueing: send a queued turn NOW (steer). The backend
  // promotes it to the front of the queue and soft-interrupts the active
  // turn — the session, transcript, and on-disk work are all preserved —
  // then dispatches it as a normal follow-up. No optimistic state change:
  // the `queued_turn_dispatched` event promotes the greyed bubble and the
  // interrupt's `ready`/`running` state events settle the composer.
  const handleSendQueuedNow = useCallback(
    (queuedId: string) => {
      if (!threadId) return;
      agentChatSendQueuedTurnNow(provider, threadId, queuedId).catch((err) => {
        toast.error(`Failed to send queued message: ${err}`);
      });
    },
    [threadId, provider],
  );

  const handleRespond = useCallback(
    (requestId: string, decision: ApprovalDecision) => {
      if (!threadId) return;
      markRequestResponding(threadId, requestId, decision);
      agentChatRespondToRequest(provider, threadId, requestId, decision).catch(
        (err) => {
          toast.error(`Failed to send decision: ${err}`);
        },
      );
    },
    [threadId, provider, markRequestResponding],
  );

  /**
   * Shared silent-restart helper. Permission-mode, effort, and
   * context-window all trigger the same flow on Claude: stop the
   * current session, start a new one with the updated launch params,
   * migrate the thread id. Transcript + model + draft survive via
   * `migrateThreadId`; resume cursor carries session state when
   * available. Codex-side callers don't route through here.
   *
   * Declared above the mode handlers so `handleModeRemove` can use
   * it for the silent-restart restore path (the SDK rejects live
   * `setPermissionMode("bypassPermissions")` even when the launch
   * carried `--dangerously-skip-permissions`).
   */
  const restartSessionWith = useCallback(
    (updates: {
      permissionMode?: string;
      effort?: string | null;
      contextWindow?: string | null;
    }) => {
      if (!threadId) return;
      const currentSlice = useAgentChatStore.getState().threads[threadId];
      if (!currentSlice) return;
      if (restarting) return;
      setRestarting(true);
      const resumeCursor = currentSlice.resumeCursor;
      const newLocalThreadId = `chat-${pane.pane_id}-${Date.now()}`;
      const nextMode =
        provider === "opencode"
          ? null
          : (updates.permissionMode ?? currentSlice.permissionMode);
      const nextEffort =
        updates.effort !== undefined ? updates.effort : currentSlice.effort;
      const nextContext =
        updates.contextWindow !== undefined
          ? updates.contextWindow
          : currentSlice.contextWindow;
      void (async () => {
        try {
          await agentChatStopSession(provider, threadId);
        } catch (err) {
          console.warn("[agent-chat] stop_session during restart failed", err);
        }
        try {
          const newId = await agentChatStartSession(pane.pane_id, provider, {
            thread_id: newLocalThreadId,
            cwd: cwd ?? "",
            model: currentSlice.model,
            resume_cursor: resumeCursor,
            permission_mode: nextMode,
            effort: nextEffort,
            context_window: nextContext,
            additional_directories: [],
            env: null,
          });
          migrateThreadId(threadId, newId);
          setThreadId(newId);
          setSessionLaunchMode(newId, nextMode);
        } catch (err) {
          toast.error(`Failed to restart session: ${err}`);
        } finally {
          setRestarting(false);
        }
      })();
    },
    [
      threadId,
      provider,
      cwd,
      pane.pane_id,
      restarting,
      migrateThreadId,
      setSessionLaunchMode,
    ],
  );

  // Plan-accept: flip the live session to `default` permission mode
  // (Claude adapter wires `query.setPermissionMode` directly — no
  // session restart) and send a synthetic "Proceed with the plan."
  // turn so the model un-sticks from the deny + interrupt that
  // ExitPlanMode triggered. This deliberately bypasses
  // `handlePermissionModeChange`, which goes through
  // `restartSessionWith` because Claude's capability declares
  // per-session granularity. Research 1 confirmed the live setter
  // is safe.
  const handleAcceptPlan = useCallback(
    async (requestId: string) => {
      if (!threadId) return;
      // Collapse the plan card locally. The sidecar denied+interrupted
      // the ExitPlanMode tool before emitting the request, so no
      // `request-resolved` event will ever arrive — without this the
      // card stays on its pending affordances and pins the transcript
      // tail in a "user must act" state that suppresses the thinking
      // indicator during the synthetic turn that follows.
      markRequestResolved(threadId, requestId, { decision: "allow" });
      try {
        await agentChatSetPermissionMode(provider, threadId, "default");
        setStorePermissionMode(threadId, "default");
        setSessionLaunchMode(threadId, "default");
        // Stage 3: accepting a plan also clears the Plan pill so the
        // picker reappears and the composer returns to normal. The
        // stashed priorPermissionMode is discarded — the user explicitly
        // opted into `default` by accepting.
        setStoreMode(threadId, "default");
        setStoreModePriorPermissionMode(threadId, null);
        await agentChatSendTurn(provider, {
          thread_id: threadId,
          text: "Proceed with the plan.",
          model_override: null,
          effort_override: null,
          permission_mode_override: null,
        });
      } catch (err) {
        toast.error(`Failed to accept plan: ${err}`);
      }
    },
    [
      threadId,
      provider,
      markRequestResolved,
      setStorePermissionMode,
      setSessionLaunchMode,
      setStoreMode,
      setStoreModePriorPermissionMode,
    ],
  );

  // Plan-reject: send a new user turn carrying the feedback (or a
  // generic revise prompt when empty). The ExitPlanMode canUseTool
  // was already denied + interrupted by the sidecar before this
  // card rendered, so there is no approval to resolve here — just a
  // fresh turn that lands on the still-open plan-mode session.
  const handleRejectPlan = useCallback(
    async (requestId: string) => {
      if (!threadId) return;
      // Same reasoning as handleAcceptPlan: no sidecar round-trip, so
      // collapse the card locally before firing the follow-up turn.
      markRequestResolved(threadId, requestId, {
        decision: "deny",
        message: "Please revise the plan.",
      });
      try {
        await agentChatSendTurn(provider, {
          thread_id: threadId,
          text: "Please revise the plan.",
          model_override: null,
          effort_override: null,
          permission_mode_override: null,
        });
      } catch (err) {
        toast.error(`Failed to reject plan: ${err}`);
      }
    },
    [threadId, provider, markRequestResolved],
  );

  // Composer-level mode pill activation. Plan and Ask both flip the
  // live session to `permission_mode: "plan"` via `setPermissionMode`
  // (no restart per Research 1) — Plan because the SDK enforces the
  // read-only contract via the picker semantics, Ask because we want
  // the same SDK-level write enforcement under the hood (the per-turn
  // prompt wrapper handled in `applyAllPrefixes` tells the model to
  // answer conversationally instead of calling ExitPlanMode). The
  // prior picker value is stashed so toggle-off can restore it.
  // Debug doesn't override permission_mode — it's prompt-wrapper-only
  // enforcement, so the slice flip is the entire activation.
  const handleModeActivate = useCallback(
    async (newMode: ActivePillMode) => {
      if (!threadId) return;
      const currentSlice = useAgentChatStore.getState().threads[threadId];
      if (!currentSlice) return;
      if (currentSlice.mode === newMode) return;
      if (newMode === "plan" || newMode === "ask") {
        const priorMode = currentSlice.permissionMode;
        try {
          await agentChatSetPermissionMode(provider, threadId, "plan");
        } catch (err) {
          const label = newMode === "plan" ? "Plan" : "Ask";
          toast.error(`Failed to activate ${label} mode: ${err}`);
          return;
        }
        setStoreModePriorPermissionMode(threadId, priorMode);
        setStorePermissionMode(threadId, "plan");
        setSessionLaunchMode(threadId, "plan");
        setStoreMode(threadId, newMode);
        // The Plan/Ask pill is a TRANSIENT, session-live override that
        // lives only in the in-memory slice (`mode` +
        // `modePriorPermissionMode`) — neither is persisted. But
        // `agent_chat_set_permission_mode` just wrote "plan" to the DB
        // row, so a restart would auto-resume a read-only "plan" session
        // with the pill gone and the prior mode unrecoverable. Re-persist
        // the user's DURABLE permission choice so the DB reflects the
        // real per-thread mode and a restart resumes in it (pill off),
        // instead of a stale, pill-less read-only lock.
        agentChatUpdateSessionConfig(threadId, {
          permission_mode: priorMode,
        }).catch((err) => {
          console.warn(
            "[agent-chat] failed to re-persist durable permission mode",
            err,
          );
        });
      } else {
        setStoreMode(threadId, newMode);
      }
    },
    [
      threadId,
      provider,
      setStoreModePriorPermissionMode,
      setStorePermissionMode,
      setSessionLaunchMode,
      setStoreMode,
    ],
  );

  /** Synthetic cleanup turn. Switches the slice off Debug first so
   *  the wrapper doesn't re-instruct the model to add markers, then
   *  asks the model to grep+remove every CODEMUX_DEBUG line and any
   *  scaffolding that supported them.
   *
   *  Optimistic-clear caveat: `hasDebugActivity` flips to false the
   *  moment `agentChatSendTurn` resolves (which is RPC-accept, not
   *  turn-completion). If Claude leaves stragglers, the banner
   *  disappears even though markers remain. The next mount-effect
   *  re-grep on chat-open reconciles this — so the stale state is
   *  bounded to the current pane lifetime.
   *
   *  Deferred: a post-`turn_completed` verification grep would close
   *  this gap inside the current session. Wiring it requires a
   *  one-shot listener tied to the turn id returned from
   *  `agentChatSendTurn` (events flow through `useAgentChatEvents`
   *  → `applyEvent`; nothing currently subscribes per-turn). The
   *  marginal value is low — Claude's cleanup is reliable in
   *  practice and the user can always click Clean up again — so
   *  this stays out of Stage 6. */
  const triggerDebugCleanup = useCallback(async () => {
    if (!threadId) return;
    if (cleanupInFlightRef.current) return;
    cleanupInFlightRef.current = true;
    setCleanupInFlight(true);
    setStoreMode(threadId, "default");
    try {
      await agentChatSendTurn(provider, {
        thread_id: threadId,
        text: CLEANUP_PROMPT,
        model_override: null,
        effort_override: null,
        permission_mode_override: null,
      });
      setStoreHasDebugActivity(threadId, false);
    } catch (err) {
      toast.error(`Failed to start debug cleanup: ${err}`);
    } finally {
      cleanupInFlightRef.current = false;
      setCleanupInFlight(false);
    }
  }, [threadId, provider, setStoreMode, setStoreHasDebugActivity]);

  /** Open the exit-confirmation dialog and resolve with the user's
   *  pick. Caller awaits the choice and dispatches accordingly. The
   *  dialog itself unmounts the moment a choice resolves. */
  const confirmDebugExit = useCallback((): Promise<DebugExitChoice> => {
    return new Promise((resolve) => {
      setDebugExitDialog({
        resolve: (choice) => {
          setDebugExitDialog(null);
          resolve(choice);
        },
      });
    });
  }, []);

  // Composer-level mode pill removal. Plan and Ask both restore the
  // stashed prior `permissionMode` via a silent session restart
  // rather than the live `setPermissionMode` setter — the SDK only
  // honours `--dangerously-skip-permissions` at launch time, so a
  // live switch BACK to `bypassPermissions` is rejected even though
  // the session was originally launched with the flag. Restarting
  // with the prior mode in launch params re-applies the flag
  // correctly. Transcript + pickers carry across via
  // `migrateThreadId`. Debug pops the cleanup-or-leave dialog when
  // markers exist; otherwise it just flips the slice back to default.
  // Missing prior falls back to the provider's default mode so the
  // session is never left in an orphan state.
  const handleModeRemove = useCallback(async () => {
    if (!threadId) return;
    const currentSlice = useAgentChatStore.getState().threads[threadId];
    if (!currentSlice) return;
    if (currentSlice.mode === "plan" || currentSlice.mode === "ask") {
      const restore =
        currentSlice.modePriorPermissionMode ??
        DEFAULT_THREAD_PERMISSION_MODE;
      // Snap the slice for immediate UI feedback (picker reappears,
      // pill drops). The restart sets sessionLaunchMode itself.
      setStorePermissionMode(threadId, restore);
      restartSessionWith({ permissionMode: restore });
      setStoreMode(threadId, "default");
      setStoreModePriorPermissionMode(threadId, null);
      return;
    }
    if (currentSlice.mode === "debug") {
      if (
        currentSlice.hasDebugActivity &&
        currentSlice.debugActivityResolved
      ) {
        const choice = await confirmDebugExit();
        if (choice === "cancel") return;
        if (choice === "cleanup") {
          await triggerDebugCleanup();
          return;
        }
        // "leave" — fall through to the default pill-drop below.
      }
      setStoreMode(threadId, "default");
      return;
    }
    setStoreMode(threadId, "default");
    setStoreModePriorPermissionMode(threadId, null);
  }, [
    threadId,
    setStorePermissionMode,
    setStoreMode,
    setStoreModePriorPermissionMode,
    restartSessionWith,
    confirmDebugExit,
    triggerDebugCleanup,
  ]);

  // Design G: mirror every picker change into the persisted session
  // row so the user's choice survives an app restart even when no live
  // session exists (the backend `set_model` / `set_permission_mode`
  // paths persist too, but the effort / context-window pickers restart
  // the session rather than calling a setter, and a mode already live
  // on the session early-returns before any restart). Fire-and-forget:
  // the DB write is best-effort and a failure only costs the
  // restart-time restore of one field, so we log at console level
  // rather than toasting.
  const persistSessionConfig = useCallback(
    (config: AgentChatSessionConfigUpdate) => {
      if (!threadId) return;
      agentChatUpdateSessionConfig(threadId, config).catch((err) => {
        console.warn("[agent-chat] persist session config failed:", err);
      });
    },
    [threadId],
  );

  const handleModelChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      setStoreModel(threadId, next);
      // Compatibility rule — use a pure planner so the decision is
      // testable in isolation (see `planModelChange`). Reads from the
      // fallback capability snapshot so the reset is correct even
      // while live data is still loading.
      const nextModel =
        capabilities?.models.find((m) => m.id === next) ?? null;
      const plan = planModelChange({
        newModel: nextModel,
        currentEffort: effort,
        currentContextWindow: contextWindow,
      });
      // Fold the model change and any compat-driven effort/context
      // resets into a single persisted patch so a restart doesn't
      // restore a stale effort that the new model no longer supports.
      const configPatch: AgentChatSessionConfigUpdate = { model: next };
      if (plan.resetEffort !== undefined) {
        setStoreEffort(threadId, plan.resetEffort);
        configPatch.effort = plan.resetEffort;
      }
      if (plan.resetContextWindow !== undefined) {
        setStoreContextWindow(threadId, plan.resetContextWindow);
        configPatch.context_window = plan.resetContextWindow;
      }
      persistSessionConfig(configPatch);
      agentChatSetModel(provider, threadId, next).catch((err) => {
        toast.error(`Failed to set model: ${err}`);
      });
    },
    [
      threadId,
      provider,
      capabilities,
      effort,
      contextWindow,
      setStoreModel,
      setStoreEffort,
      setStoreContextWindow,
      persistSessionConfig,
    ],
  );

  const handlePermissionModeChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      const currentSlice = useAgentChatStore.getState().threads[threadId];
      if (!currentSlice) return;
      // Delegate the decision to `planPermissionModeChange` — it
      // checks the mode is valid for the active provider and reads
      // `permission_granularity` from capabilities to decide whether
      // to restart. Returns null for unknown modes.
      const plan = planPermissionModeChange({
        nextMode: next,
        capabilities,
      });
      if (!plan) return;
      setStorePermissionMode(threadId, plan.setPermissionMode);
      // Persist the mode BEFORE the early return below: when the mode
      // is already live we skip the restart, so the DB write is the
      // only thing that carries this choice across a restart.
      persistSessionConfig({ permission_mode: plan.setPermissionMode });
      // Skip the restart when the same mode is already live on the
      // current session — avoids a no-op session teardown.
      if (currentSlice.sessionLaunchMode === plan.setPermissionMode) return;
      if (plan.restart) {
        restartSessionWith({ permissionMode: plan.setPermissionMode });
      }
      // PerTurn providers: the mode is already persisted; the next
      // `sendTurn` picks it up via `permission_mode_override`.
    },
    [
      threadId,
      capabilities,
      setStorePermissionMode,
      restartSessionWith,
      persistSessionConfig,
    ],
  );

  /**
   * Effort change — delegates the decision to `planEffortChange` so
   * the three-branch logic (ultrathink prepend / strip-and-set / plain
   * set) can be unit-tested in isolation. This handler is pure
   * action-dispatch.
   */
  const handleEffortChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      const plan = planEffortChange({
        nextEffort: next,
        model: activeModel,
        currentDraft: draft,
        provider,
      });
      if (!plan) return;
      if (plan.updateDraft) {
        setInputDraft(threadId, plan.updateDraft.nextDraft);
      }
      if (plan.setEffort !== null) {
        setStoreEffort(threadId, plan.setEffort);
        persistSessionConfig({ effort: plan.setEffort });
      }
      if (plan.restart) {
        restartSessionWith({ effort: plan.setEffort });
      }
    },
    [
      threadId,
      activeModel,
      draft,
      provider,
      setInputDraft,
      setStoreEffort,
      restartSessionWith,
      persistSessionConfig,
    ],
  );

  const handleContextWindowChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      setStoreContextWindow(threadId, next);
      persistSessionConfig({ context_window: next });
      // Context window on Claude is encoded into the model id (e.g.
      // `claude-opus-4-7[1m]`), which is a session-init parameter.
      // Mid-session change → restart.
      if (provider === "claude") {
        restartSessionWith({ contextWindow: next });
      }
    },
    [
      threadId,
      provider,
      setStoreContextWindow,
      restartSessionWith,
      persistSessionConfig,
    ],
  );

  const handleProviderModelChange = useCallback(
    (nextProvider: AgentChatProviderKind, nextModel: string) => {
      // A model pick inside the current provider is the cheap live setter.
      if (nextProvider === provider) {
        handleModelChange(nextModel);
        return;
      }
      if (!threadId || !cwd || restarting) return;

      const currentSlice = useAgentChatStore.getState().threads[threadId];
      if (!currentSlice) return;
      const defaults = capabilityDefaults(nextProvider, nextModel);
      const nextLaunchMode = defaults.permissionMode;
      const nextDisplayMode =
        nextLaunchMode ?? DEFAULT_THREAD_PERMISSION_MODE;
      const oldProvider = provider;
      const oldThreadId = threadId;
      const oldLaunchMode =
        oldProvider === "opencode"
          ? null
          : (currentSlice.sessionLaunchMode ?? currentSlice.permissionMode);
      setRestarting(true);

      void (async () => {
        try {
          // Stop first so provider events for the shared thread id cannot
          // race: an old Claude `closed` arriving after Codex `ready` would
          // otherwise make the new session look dead. SessionNotFound is an
          // idempotent success in the backend, covering restored panes whose
          // provider process has not been rebuilt yet.
          await agentChatStopSession(oldProvider, oldThreadId);

          const startedThreadId = await agentChatStartSession(
            pane.pane_id,
            nextProvider,
            {
              thread_id: oldThreadId,
              cwd,
              model: nextModel,
              // Provider-native cursors are not portable. The visible
              // transcript stays on the same CodeMux thread, while the new
              // adapter starts with clean provider-side context.
              resume_cursor: null,
              permission_mode: nextLaunchMode,
              effort: defaults.effort,
              context_window: defaults.contextWindow,
              additional_directories: [],
              env: null,
            },
          );

          if (startedThreadId !== oldThreadId) {
            migrateThreadId(oldThreadId, startedThreadId);
            setThreadId(startedThreadId);
          }
          const targetThreadId = startedThreadId;
          setProvider(nextProvider);
          setStoreModel(targetThreadId, nextModel);
          setStoreEffort(targetThreadId, defaults.effort);
          setStoreContextWindow(targetThreadId, defaults.contextWindow);
          setStorePermissionMode(targetThreadId, nextDisplayMode);
          setSessionLaunchMode(targetThreadId, nextLaunchMode);
          setStoreResumeCursor(targetThreadId, null);
          // Provider-specific transient modes cannot safely cross adapters.
          setStoreMode(targetThreadId, "default");
          setStoreModePriorPermissionMode(targetThreadId, null);
        } catch (switchError) {
          // The old DB row and pane binding are unchanged until the new
          // provider starts successfully. Best-effort rebuild the stopped
          // adapter so a failed switch leaves the existing chat usable.
          try {
            await agentChatStartSession(pane.pane_id, oldProvider, {
              thread_id: oldThreadId,
              cwd,
              model: currentSlice.model,
              resume_cursor: currentSlice.resumeCursor,
              permission_mode: oldLaunchMode,
              effort: currentSlice.effort,
              context_window: currentSlice.contextWindow,
              additional_directories: [],
              env: null,
            });
          } catch (recoveryError) {
            console.error(
              "[agent-chat] failed to recover previous provider after switch error",
              recoveryError,
            );
          }
          toast.error(`Failed to switch provider: ${switchError}`);
        } finally {
          setRestarting(false);
        }
      })();
    },
    [
      provider,
      threadId,
      cwd,
      restarting,
      pane.pane_id,
      handleModelChange,
      migrateThreadId,
      setStoreModel,
      setStoreEffort,
      setStoreContextWindow,
      setStorePermissionMode,
      setSessionLaunchMode,
      setStoreResumeCursor,
      setStoreMode,
      setStoreModePriorPermissionMode,
    ],
  );

  const sessionReady = threadId != null && !starting && !restarting;

  // ── Thread Scope (new-thread empty state) ──
  //
  // Pane-local scope state, per-pane lifetime (there is no draft to
  // persist into — this pane is bound to a real workspace). Mirrors
  // the draft surface's `checkoutMode`/`worktreeName`/`baseBranch`
  // draft fields:
  //  - "current" (default): keep chatting in this pane's workspace.
  //  - "worktree": the FIRST submit creates a sibling worktree
  //    (deferred; auto-named from the first message when the name is
  //    empty), routes the message into the new worktree's session,
  //    and activates it — see `handleDeferredWorktreeSubmit`.
  const [checkoutMode, setCheckoutMode] = useState<"current" | "worktree">(
    "current",
  );
  const [worktreeName, setWorktreeName] = useState("");
  // Seed lazily from the workspace's actual checked-out branch when
  // it's already hydrated at mount; the fill-if-empty effect below
  // covers late hydration. A user pick (or the branch popover's
  // main/master fallback seed) makes the value non-empty and is never
  // clobbered.
  const [baseBranch, setBaseBranch] = useState<string>(
    () => paneWorkspaceBranch ?? "",
  );
  useEffect(() => {
    if (!paneWorkspaceBranch) return;
    setBaseBranch((prev) => (prev === "" ? paneWorkspaceBranch : prev));
  }, [paneWorkspaceBranch]);

  // Zone 1 — nothing renders above the composer anymore: the scope
  // lives BELOW it (`belowComposerSlot`) — `ThreadScopeRow` while the
  // thread is empty, the Context Row once it has messages (see below).
  // `undefined` (no resolvable project root) keeps the Composer's
  // default cwd label as before.
  const zone1Override = workspaceProjectRoot ? null : undefined;

  // Old ProjectPicker onChange behavior, verbatim: clear any active
  // draft (Stage C Bug-2 pattern — a draft pinned to this slot would
  // otherwise re-render on top of the activated workspace), then
  // activate the picked project's first workspace, else open the
  // new-workspace dialog seeded with the path.
  const handleScopeSelectProject = useCallback(
    (targetProjectPath: string) => {
      useChatDraftStore.getState().setActiveDraft(null);
      const snapshot = useAppStore.getState().appState;
      const groups = snapshot
        ? groupWorkspacesByProject(snapshot.workspaces, homeDir)
        : [];
      const targetGroup = groups.find(
        (g) => g.projectPath === targetProjectPath,
      );
      const target = targetGroup?.workspaces[0];
      if (target) {
        activateWorkspace(target.workspace_id).catch(console.error);
      } else {
        setShowNewWorkspaceDialog(true, targetProjectPath);
      }
    },
    [homeDir, setShowNewWorkspaceDialog],
  );

  const handleScopeSelectHomeWorkspace = useCallback((wsId: string) => {
    useChatDraftStore.getState().setActiveDraft(null);
    activateWorkspace(wsId).catch(console.error);
  }, []);

  const belowComposerSlot =
    messages.length === 0 && workspaceProjectRoot ? (
      <ThreadScopeRow
        location={{
          kind: "workspace",
          isHome: isHomeWorkspace,
          onSelectHomeWorkspace: handleScopeSelectHomeWorkspace,
          onSelectProject: handleScopeSelectProject,
        }}
        // `null` for a home-rooted pane — no project, so the checkout +
        // branch controls hide and the home scope hint renders.
        projectPath={isHomeWorkspace ? null : workspaceProjectRoot}
        checkoutMode={checkoutMode}
        worktreeName={worktreeName}
        baseBranch={baseBranch}
        disabled={isSending}
        onChangeCheckoutMode={setCheckoutMode}
        onChangeWorktreeName={setWorktreeName}
        onChangeBaseBranch={setBaseBranch}
        // Real workspace on this surface — the status cluster has a
        // git branch to report from its first render, unlike
        // `DraftChatSurface`, which has no workspace yet and doesn't
        // pass this.
        trailing={<WorkspaceStatusCluster />}
      />
    ) : messages.length > 0 && workspaceProjectRoot ? (
      // Context Row (design: `.design-import/Context Row.dc.html`) —
      // once the thread has messages, scope is committed (the first
      // send locked it in), so this half turns read-only: static
      // project/branch labels on the left, the same passive git/PR
      // status cluster on the right that the old bottom
      // `WorkspaceContextBar` used to show (now hidden for this pane —
      // see `useAgentChatPaneActive`).
      <div className="flex w-full items-center justify-between gap-2.5 px-1">
        <div className="flex min-w-0 items-center gap-0.5 text-xs font-semibold text-foreground/90">
          <span
            className="inline-flex h-[27px] shrink-0 items-center gap-1.5 rounded-md px-2"
            title={workspaceProjectRoot}
          >
            {isHomeWorkspace ? (
              <Home className="size-3.5 text-status-remote" />
            ) : (
              <Folder className="size-3.5 text-muted-foreground" />
            )}
            <span className="max-w-[140px] truncate">
              {isHomeWorkspace ? "Home" : basename(workspaceProjectRoot)}
            </span>
          </span>
          {paneWorkspaceBranch && (
            <>
              <span className="select-none text-muted-foreground/50">·</span>
              <span
                className="inline-flex h-[27px] shrink-0 items-center gap-1.5 rounded-md px-2 font-mono"
                title={`Branch: ${paneWorkspaceBranch}`}
              >
                <GitBranch className="size-3 text-muted-foreground" />
                <span className="max-w-[160px] truncate">
                  {paneWorkspaceBranch}
                </span>
              </span>
            </>
          )}
        </div>
        <WorkspaceStatusCluster />
      </div>
    ) : undefined;

  /** Thread Scope deferred-worktree first send. Replaces `handleSubmit`
   *  ONLY while the thread is empty AND the checkout control is set to
   *  "New worktree" (see `composerOnSubmit` below) — every other send
   *  goes through the unmodified `handleSubmit`.
   *
   *  Order of operations (mirrors `materializeAndSend`'s worktree arm):
   *   1. Create the sibling worktree via the shared
   *      `createDeferredWorktree` (name precedence: typed name →
   *      `generateBranchName(firstMessage)` → random fallback) off the
   *      selected base branch.
   *   2. `prestartWorktreeSession` — attach a chat pane + start a
   *      session on the NEW workspace before anything else, so the
   *      first turn below can't land ahead of the adapter registering
   *      the thread (`session_not_found`). Carries this pane's picker
   *      config so the new session matches what the user configured.
   *   3. Send the first turn into the new thread with the same
   *      skills/attachment-block/mode-prefix/image pipeline
   *      `handleSubmit` uses. Attachments staged on THIS pane's thread
   *      are injected into the turn's text/images (their chips stay
   *      keyed to the old thread and are cleared below — the content
   *      travels, the chip UI does not).
   *      Limitation vs `handleSubmit`: the stale GitHub-detail
   *      re-fetch pass is skipped — acceptable for a first turn, the
   *      chips were fetched moments ago on this same surface.
   *   4. Clear this pane's composer + chips and activate the new
   *      workspace.
   *
   *  Failure policy: worktree/session resources created by earlier
   *  steps are not rolled back (same as materialize); the composer
   *  text stays in THIS pane so the user can retry. */
  const handleDeferredWorktreeSubmit = useCallback(() => {
    if (sendInFlightRef.current) return;
    if (!workspaceProjectRoot) return;
    const rawText = draft.trim();
    if (!rawText) return;
    const plan = planSubmit({ rawText, provider, effort });
    sendInFlightRef.current = true;
    setIsSending(true);

    // Instant feedback (Bug 2 fix): mirror the warm-send pattern — build
    // the display thumbnails, append the optimistic user bubble into THIS
    // pane's thread, and clear the composer BEFORE the multi-second
    // worktree-create + session-start work. Previously the composer sat
    // dead (text still visible, no bubble) until all of that finished.
    // A client nonce lets us roll the bubble back on failure; on success
    // we drop it here and let the freshly-prestarted thread's own
    // optimistic bubble carry over as the pane flips.
    const snapshotAttachments = threadId
      ? useAgentChatStore.getState().threads[threadId]?.stagedAttachments ?? []
      : [];
    const imageDisplaySources = buildImageDisplaySources(snapshotAttachments);
    const clientNonce =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `nonce-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    if (threadId) {
      appendUserMessage(threadId, plan.text, clientNonce, imageDisplaySources);
      setInputDraft(threadId, "");
      requestScrollToBottom();
    }

    /** Roll the optimistic bubble back and restore the composer text so a
     *  failed first send never strands an orphan bubble or eats the
     *  user's prompt. Mirrors the warm-send / Continue-run rollback. */
    const rollback = () => {
      if (threadId) {
        removeUserMessageByNonce(threadId, clientNonce);
        setInputDraft(threadId, rawText);
      }
    };

    void (async () => {
      try {
        const created = await createDeferredWorktree(
          workspaceProjectRoot,
          worktreeName,
          baseBranch,
          rawText,
        );
        const prestarted = await prestartWorktreeSession(
          created.workspaceId,
          {
            model,
            // Same plan/ask coupling as materialize's
            // `effectivePermissionMode`: both modes boot the SDK with
            // writes locked off.
            permissionMode:
              mode === "plan" || mode === "ask"
                ? "plan"
                : permissionMode ?? DEFAULT_THREAD_PERMISSION_MODE,
            effort,
            contextWindow,
            mode,
          },
          // Skip the `waitForWorkspaceCwd` poll when the create response
          // already carries the new worktree's cwd.
          created.cwd,
        );
        if (!prestarted) {
          // Degenerate: the new workspace hasn't hydrated into the
          // store (event-delivery reordering). The worktree exists but
          // there is no session to send into — restore the composer text
          // (and drop the optimistic bubble) and just flip over; the
          // user can re-send in the new pane (its mount-effect starts the
          // session).
          rollback();
          toast.warning(
            "Worktree created — send your message again in the new workspace.",
          );
          useChatDraftStore.getState().setActiveDraft(null);
          activateWorkspace(created.workspaceId).catch(console.error);
          return;
        }
        const skillBodies = resolveSkillBodies(rawText, skillsRegistry);
        // Images staged at attach time; await only stragglers, then read
        // fresh so staging patches (new attachment objects) are visible.
        const imageIds = imageAttachmentIds(snapshotAttachments);
        if (imageIds.length > 0) await awaitImageStaging(imageIds);
        const freshAttachments = threadId
          ? useAgentChatStore.getState().threads[threadId]?.stagedAttachments ??
            []
          : [];
        const unstaged = unstagedImageAttachments(freshAttachments);
        if (unstaged.length > 0) {
          rollback();
          toast.error(
            "An attached image failed to upload — remove it and try again.",
          );
          return;
        }
        const attachmentBlock = buildAttachmentBlock(
          activeAttachments(rawText, freshAttachments),
        );
        const imageRefs = buildImageRefs(freshAttachments);
        const sdkText = applyAllPrefixes(
          rawText,
          mode,
          effort,
          skillBodies,
          attachmentBlock,
        );
        // Optimistic echo into the NEW thread's slice so the pane that
        // mounts on activation renders the user bubble immediately.
        useAgentChatStore
          .getState()
          .appendUserMessage(
            prestarted.threadId,
            plan.text,
            undefined,
            imageDisplaySources,
          );
        await agentChatSendTurn(provider, {
          thread_id: prestarted.threadId,
          text: sdkText,
          images: imageRefs,
          model_override: null,
          effort_override: plan.effortOverride,
          permission_mode_override: null,
        });
        // Success — drop this pane's optimistic bubble + chips (the new
        // thread carries its own bubble now) and flip to the new
        // workspace.
        if (threadId) {
          removeUserMessageByNonce(threadId, clientNonce);
          clearStagedAttachments(threadId);
        }
        useChatDraftStore.getState().setActiveDraft(null);
        activateWorkspace(created.workspaceId).catch(console.error);
      } catch (err) {
        rollback();
        toast.error(`Failed to start in a new worktree: ${err}`);
      } finally {
        sendInFlightRef.current = false;
        setIsSending(false);
      }
    })();
  }, [
    workspaceProjectRoot,
    draft,
    provider,
    effort,
    mode,
    model,
    permissionMode,
    contextWindow,
    threadId,
    worktreeName,
    baseBranch,
    skillsRegistry,
    appendUserMessage,
    removeUserMessageByNonce,
    requestScrollToBottom,
    setInputDraft,
    clearStagedAttachments,
  ]);

  // Intercept ONLY the empty-thread + worktree-mode first send;
  // everything else stays on the unmodified `handleSubmit` path.
  const composerOnSubmit =
    messages.length === 0 &&
    checkoutMode === "worktree" &&
    !isHomeWorkspace &&
    workspaceProjectRoot !== null
      ? handleDeferredWorktreeSubmit
      : handleSubmit;

  // AskUserQuestion prompts render as a composer-attached panel
  // (one-question-per-card pattern, similar to Claude.ai) rather
  // than inline in the transcript. Only the first pending
  // user-input request surfaces; MessageList
  // reduces user-input items to a tiny marker so the transcript
  // doesn't duplicate the prompt.
  const pendingUserInput = useMemo<ChatViewItem | null>(() => {
    for (const m of messages) {
      if (
        m.kind === "permission_request" &&
        m.request_kind === "user-input" &&
        m.resolution.state === "pending"
      ) {
        return m;
      }
    }
    return null;
  }, [messages]);

  const handleSubmitUserInput = useCallback(
    (output: AskUserQuestionOutput) => {
      if (!pendingUserInput || pendingUserInput.kind !== "permission_request") {
        return;
      }
      handleRespond(pendingUserInput.request_id, {
        decision: "allow",
        updated_input: output,
      });
    },
    [pendingUserInput, handleRespond],
  );

  const pendingInputPanelEl =
    pendingUserInput && pendingUserInput.kind === "permission_request" ? (
      <ComposerPendingInputPanel
        // Remount when the pending request_id changes so per-question
        // state (picks / free-text / current index) resets for a new
        // prompt instead of leaking from the previous one.
        key={pendingUserInput.request_id}
        item={pendingUserInput}
        onSubmit={handleSubmitUserInput}
      />
    ) : null;

  const composerEl = (
    <Composer
      draft={draft}
      cwd={cwd}
      // An empty pane reads as a new chat: "Describe what you want the
      // agent to do…" until the first turn lands (design D10 copy).
      isDraft={messages.length === 0}
      // In a subagent drill-in the composer stays parent-bound; only the
      // placeholder changes to make that explicit (design copy).
      placeholderOverride={
        enteredSubagent ? "Steering goes to the orchestrator…" : undefined
      }
      zone1Override={zone1Override}
      belowComposerSlot={belowComposerSlot}
      provider={provider}
      model={model}
      permissionMode={permissionMode}
      effort={effort}
      contextWindow={contextWindow}
      activeModel={activeModel}
      effortLabelMap={effortLabelMap}
      permissionModes={permissionModes}
      ultrathinkInBodyText={ultrathinkInBodyText}
      // Real streaming only — a follow-up sent while this is true gets
      // QUEUED, not blocked. `sending` blocks re-submit during the send
      // RPC ack-lag without blocking queueing.
      streaming={streaming}
      sending={isSending}
      // Dead-run recovery (issue #154): a Continue chip in the composer
      // strip when the last run died and nothing is in flight.
      interrupted={interrupted}
      onContinueRun={handleContinueRun}
      sessionReady={sessionReady}
      showProviderPicker={ENABLE_PROVIDER_PICKER}
      mode={mode}
      stagedAttachments={stagedAttachments}
      onRemoveAttachment={(id) => {
        if (!threadId) return;
        // Best-effort delete of the backend staging file if this chip
        // was an image that already staged.
        const staged = useAgentChatStore
          .getState()
          .threads[threadId]?.stagedAttachments.find((a) => a.id === id)
          ?.stagedImage;
        if (staged) discardStagedImage(staged.path);
        removeStagedAttachment(threadId, id);
      }}
      onToggleExpandPr={handleToggleExpandPr}
      onAttachFile={handleAttachFile}
      onAttachFolder={handleAttachFolder}
      onAttachIssue={handleAttachIssue}
      onAttachPr={handleAttachPr}
      onAttachImage={handleAttachImage}
      modelSupportsImages={activeModel?.supports_images ?? false}
      isGithubRepo={isGithubRepo}
      ghAuthenticated={ghAuthenticated}
      onDraftChange={(next) => {
        if (!threadId) return;
        setInputDraft(threadId, next);
      }}
      onSubmit={composerOnSubmit}
      onStop={handleStop}
      onProviderModelChange={handleProviderModelChange}
      onModelChange={handleModelChange}
      onPermissionModeChange={handlePermissionModeChange}
      onEffortChange={handleEffortChange}
      onContextWindowChange={handleContextWindowChange}
      onModeActivate={handleModeActivate}
      onModeRemove={handleModeRemove}
    />
  );

  const debugBannerEl =
    mode === "debug" && hasDebugActivity && debugActivityResolved ? (
      <DebugCleanupBanner
        onCleanup={triggerDebugCleanup}
        busy={cleanupInFlight}
      />
    ) : null;

  return (
    <div
      ref={paneRootRef}
      className="flex h-full w-full flex-col bg-background"
    >
      {messages.length === 0 ? (
        <ChatHomeLanding composer={composerEl} />
      ) : (
        <>
          {enteredSubagent ? (
            // Drill-in: breadcrumb sub-header + read-only sub-transcript.
            // The composer below stays wired to the parent thread.
            <>
              <SubagentBreadcrumb
                subagent={enteredSubagent}
                ordinal={subagentOrdinal(messages, enteredSubagent.id)}
                onBack={handleExitSubagent}
              />
              <div className="flex-1 min-h-0 w-full overflow-y-auto">
                <SubagentView
                  subagent={enteredSubagent}
                  requests={enteredSubagentRequests}
                />
              </div>
            </>
          ) : (
            <ChatTranscript
              messages={messages}
              streaming={streaming || isSending}
              stalled={stalled}
              interrupted={interrupted}
              scrollToBottomSignal={scrollToBottomSignal}
              sessionStartedAt={sessionStartedAt}
              provider={provider}
              onRespondToRequest={handleRespond}
              onAcceptPlan={handleAcceptPlan}
              onRejectPlan={handleRejectPlan}
              onCancelQueued={handleCancelQueued}
              onSendQueuedNow={handleSendQueuedNow}
              onEnterSubagent={handleEnterSubagent}
              workspaceId={workspaceIdForPane}
            />
          )}
          {/* Docked live subagent activity bar (design "A living status,
              docked by the composer"): one bar for the whole thread,
              hidden while drilled into a subagent (the design only shows
              it in the conversation view) and hidden entirely while idle. */}
          {!enteredSubagent && (
            <div className="px-7 pt-2.5">
              <SubagentActivityBar
                messages={messages}
                threadId={threadId}
                onJump={handleJumpToSubagentCard}
              />
            </div>
          )}
          {/* Composer region (design D10): groups the whole composer
              column — AskUserQuestion panel, debug banner, and the
              composer card — below the scrolling transcript. */}
          <div className="pt-3.5">
            {pendingInputPanelEl}
            {debugBannerEl}
            {composerEl}
          </div>
        </>
      )}
      <DebugExitDialog
        open={debugExitDialog !== null}
        onChoose={(choice) => debugExitDialog?.resolve(choice)}
      />
    </div>
  );
}

const EMPTY_MESSAGES: ChatViewItem[] = [];
const EMPTY_REQUESTS: PermissionRequestItem[] = [];
const EMPTY_ATTACHMENTS: Attachment[] = [];
