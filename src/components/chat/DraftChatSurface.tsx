import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { LoaderCircle } from "lucide-react";

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
import { activeAttachments } from "@/lib/agent-chat/attachment-tokens";
import {
  awaitImageStaging,
  beginImageStaging,
  discardStagedImage,
} from "@/lib/agent-chat/image-staging";
import {
  materializeAndSend,
  type MaterializePhase,
} from "@/lib/agent-chat/materialize";
import type {
  UserMessageImage,
  UserMessageItem,
} from "@/lib/agent-chat/types";
import { resolveSkillBodies } from "@/lib/agent-chat/skill-tokens";
import { hasUltrathinkInBodyText } from "@/lib/agent-chat/ultrathink";
import { basename } from "@/lib/path";
import { toast } from "@/lib/toast";
import {
  useAgentChatStore,
  type Attachment,
} from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import { selectActiveSkills, useSkillsStore } from "@/stores/skills-store";
import {
  selectActiveDraft,
  useChatDraftStore,
  type ChatDraft,
} from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { capabilityDefaults } from "@/lib/agent-chat/capability-defaults";
import {
  checkGhStatus,
  checkGithubRepo,
  getGithubIssueByPath,
  getGithubPrByPath,
  getGithubPrDiffByPath,
  primeChatMcp,
  readFileForAttachment,
  readFolderForAttachment,
} from "@/tauri/commands";
import type {
  FileMatch,
  FolderMatch,
  GitHubIssue,
  PullRequestInfo,
} from "@/tauri/types";

import { ChatHomeLanding } from "./ChatHomeLanding";
import { Composer } from "./Composer";
import { UserMessage } from "./UserMessage";
import type { ActivePillMode } from "./pickers/ModePill";
import { ThreadScopeRow } from "./pickers/ThreadScopeRow";

/** Grace period between `markPromoted` and `clearDraft`. Gives any
 *  in-flight selector a chance to observe the promotion before the
 *  draft is swept. Matches the window called out in §9 of the plan. */
const CLEAR_AFTER_PROMOTION_MS = 5000;

/** Stable empty array so the attachment-slice selector doesn't return
 *  a fresh reference every render and retrigger the chip-strip render
 *  effect in an infinite loop. */
const EMPTY_ATTACHMENTS: Attachment[] = [];

/**
 * Top-level draft surface. Reads the active draft, wires a Composer
 * against it, and promotes the draft via `materializeAndSend` on the
 * first message send.
 */
export function DraftChatSurface() {
  const draft = useChatDraftStore(selectActiveDraft);
  if (!draft) return null;
  // Key on the draft id so a slot swap (home → project) remounts with a
  // fresh resolved-cwd effect instead of carrying stale state.
  return <DraftChatSurfaceInner key={draft.draftId} draft={draft} />;
}

function DraftChatSurfaceInner({ draft }: { draft: ChatDraft }) {
  const capabilities = useProviderCapabilities((s) =>
    selectCapabilities(s, draft.provider),
  );
  const activeModel = selectModel(capabilities, draft.model);
  const effortLabelMap = capabilities?.effort_label_map ?? {};
  const permissionModes = capabilities?.permission_modes ?? null;

  const updateDraftInput = useChatDraftStore((s) => s.updateDraftInput);
  const updateDraftConfig = useChatDraftStore((s) => s.updateDraftConfig);
  const updateDraftTarget = useChatDraftStore((s) => s.updateDraftTarget);
  const setActiveDraft = useChatDraftStore((s) => s.setActiveDraft);
  const clearDraft = useChatDraftStore((s) => s.clearDraft);

  // Step 8 Stage 2 — staged attachments live on the agent-chat slice
  // keyed by `draft.threadId` (pre-minted; survives materialize). The
  // chip strip + popup pick → onAttachFile flow is identical to
  // AgentChatPane's; the only divergence is that the attachment block
  // is forwarded to `materializeAndSend` rather than read from a
  // mounted slice's `handleSubmit`.
  const stagedAttachments = useAgentChatStore(
    (s) => s.threads[draft.threadId]?.stagedAttachments ?? EMPTY_ATTACHMENTS,
  );
  const addStagedAttachment = useAgentChatStore((s) => s.addStagedAttachment);
  const updateStagedAttachment = useAgentChatStore(
    (s) => s.updateStagedAttachment,
  );
  const removeStagedAttachment = useAgentChatStore(
    (s) => s.removeStagedAttachment,
  );
  const clearStagedAttachments = useAgentChatStore(
    (s) => s.clearStagedAttachments,
  );

  // Home directory is hydrated once at App mount into useAppStore.
  // Reading it from there (rather than re-fetching via a local Tauri
  // roundtrip on every DraftChatSurface mount) closes a race where
  // the user hit Send before the roundtrip resolved: the seed effect
  // below would still be blocked on its gate, `draft.target.kind`
  // stayed on "home", and materializeAndSend went down the
  // `createHomeRootedWorkspace` path — minting a duplicate workspace
  // instead of reusing the active sidebar workspace.
  const appHomeDir = useAppStore((s) => s.homeDir);

  const existingWorkspaceCwd = useAppStore((s) => {
    if (draft.target.kind !== "existing_workspace") return null;
    const wsId = draft.target.workspaceId;
    return s.appState?.workspaces.find((w) => w.workspace_id === wsId)?.cwd ?? null;
  });

  // Identity + project path of the currently active sidebar workspace.
  // Split into two primitive selectors (instead of a single object
  // selector) so Zustand's default Object.is equality doesn't see a
  // fresh reference every render and retrigger the seed effect in an
  // infinite loop. Used to seed a home draft's target when the user
  // lands on Home while a project workspace is active, so the composer
  // mirrors the sidebar context instead of sitting on "Select project."
  // We seed to `existing_workspace` (not `project`) because the user's
  // intent is "send into the active workspace I'm looking at," not
  // "spawn a new worktree for this project" — the latter would double
  // the workspace on submit.
  const activeSidebarWorkspaceId = useAppStore((s) => {
    const st = s.appState;
    if (!st?.active_workspace_id) return null;
    const ws = st.workspaces.find(
      (w) => w.workspace_id === st.active_workspace_id,
    );
    return ws ? ws.workspace_id : null;
  });
  const activeSidebarProjectPath = useAppStore((s) => {
    const st = s.appState;
    if (!st?.active_workspace_id) return null;
    const ws = st.workspaces.find(
      (w) => w.workspace_id === st.active_workspace_id,
    );
    return ws ? ws.project_root ?? ws.cwd ?? null : null;
  });

  // Seed the home draft's target with the active sidebar workspace on
  // mount. Gated on appHomeDir so a home-rooted workspace (where
  // `project_root === $HOME`) stays on the general Home landing rather
  // than being mistaken for a real project. Runs once per draft: as
  // soon as the target flips away from "home" the guard short-circuits
  // every subsequent render, so the user's later picker changes stick.
  //
  // Skipped entirely when `draft.lockedToHome` is set — that flag is
  // raised by the sidebar's explicit "New agent" button, whose tooltip
  // promises "New chat in home directory". Honouring that beats
  // mirroring the sidebar context when the user has explicitly asked
  // for the home landing.
  useEffect(() => {
    if (draft.lockedToHome) return;
    if (draft.target.kind !== "home") return;
    if (appHomeDir === null) return;
    if (!activeSidebarWorkspaceId || !activeSidebarProjectPath) return;
    if (activeSidebarProjectPath === appHomeDir) return;
    updateDraftTarget(draft.draftId, {
      kind: "existing_workspace",
      workspaceId: activeSidebarWorkspaceId,
    });
  }, [
    draft.lockedToHome,
    draft.target.kind,
    draft.draftId,
    activeSidebarWorkspaceId,
    activeSidebarProjectPath,
    appHomeDir,
    updateDraftTarget,
  ]);

  // Project root of the workspace targeted by an `existing_workspace`
  // draft. Scopes the ThreadScopeRow's checkout/branch controls to the
  // targeted workspace's project (not unrelated ones).
  const existingWorkspaceProjectRoot = useAppStore((s) => {
    if (draft.target.kind !== "existing_workspace") return null;
    const wsId = draft.target.workspaceId;
    const ws = s.appState?.workspaces.find((w) => w.workspace_id === wsId);
    return ws?.project_root ?? ws?.cwd ?? null;
  });

  // Thread Scope redesign — resolved project root fed to `ThreadScopeRow`
  // for the checkout + branch controls. `null` for home drafts (no
  // project) and for an `existing_workspace` target whose workspace
  // hasn't hydrated into app-state yet.
  const scopeProjectPath = useMemo(() => {
    switch (draft.target.kind) {
      case "home":
        return null;
      case "project":
        return draft.target.projectPath;
      case "existing_workspace":
        return existingWorkspaceProjectRoot;
    }
  }, [draft.target, existingWorkspaceProjectRoot]);

  const displayCwd = useMemo(() => {
    switch (draft.target.kind) {
      case "home":
        return null; // LocationControl owns this slot
      case "project":
        return draft.target.projectPath;
      case "existing_workspace":
        return existingWorkspaceCwd;
    }
  }, [draft.target, existingWorkspaceCwd]);

  // Synchronous guard against duplicate submissions while a single
  // materialise is in flight. Mirrors AgentChatPane's send-in-flight
  // ref pattern.
  const sendInFlightRef = useRef(false);

  // Instant-feedback state (Bug 2 fix): once the user submits, we render
  // the just-sent conversation (their bubble + a phase status line) in
  // place of the landing, so the composer never sits dead for the
  // multi-second workspace/session bring-up. Cleared on failure (restore
  // composer); on success the surface unmounts as the live pane flips in.
  const [pending, setPending] = useState<DraftPendingState | null>(null);

  // Warm the MCP servers as soon as the draft mounts so the up-front
  // prime cost overlaps the user composing, instead of blocking the
  // first `agent_chat_start_session`. Fire-and-forget; the backend
  // returns immediately and warms in the background.
  useEffect(() => {
    void primeChatMcp().catch(() => {
      /* best-effort */
    });
  }, []);

  const handleSubmit = useCallback(() => {
    if (sendInFlightRef.current) return;
    // Re-read fresh state so a same-tick keystroke then Enter sees the
    // just-written text.
    const state = useChatDraftStore.getState();
    let currentDraft = state.draftsById[draft.draftId];
    if (!currentDraft) return;
    if (currentDraft.promoting) return;
    const text = currentDraft.inputDraft.trim();
    if (!text) return;

    // Submit-time salvage: if we're about to send a "home" draft while
    // a project-rooted sidebar workspace is active, silently re-route
    // the target to `existing_workspace` before handing off to
    // materialize. The mount-time seed effect normally catches this,
    // but if the user types-and-sends fast enough that the effect
    // hasn't run yet (React `useEffect` runs post-paint, and the
    // appHomeDir selector might still be null on the very first tick)
    // we'd otherwise hit `createHomeRootedWorkspace` and mint a
    // duplicate workspace.
    //
    // Skipped when `lockedToHome` is set — same rationale as the
    // mount-time effect: explicit "New agent" clicks must materialise
    // in the home directory regardless of sidebar context.
    if (
      !currentDraft.lockedToHome &&
      currentDraft.target.kind === "home" &&
      activeSidebarWorkspaceId !== null &&
      activeSidebarProjectPath !== null &&
      appHomeDir !== null &&
      activeSidebarProjectPath !== appHomeDir
    ) {
      state.updateDraftTarget(currentDraft.draftId, {
        kind: "existing_workspace",
        workspaceId: activeSidebarWorkspaceId,
      });
      // Re-read after the mutation so the snapshot handed to
      // materializeAndSend carries the corrected target.
      const refreshed = useChatDraftStore
        .getState()
        .draftsById[currentDraft.draftId];
      if (refreshed) currentDraft = refreshed;
    }

    // Resolve the cwd the backend should launch under.
    const cwdForSession = (() => {
      switch (currentDraft.target.kind) {
        case "home":
          return appHomeDir;
        case "project":
          return currentDraft.target.projectPath;
        case "existing_workspace":
          return existingWorkspaceCwd;
      }
    })();

    if (!cwdForSession) {
      toast.error("Unable to resolve working directory for this chat.");
      return;
    }

    sendInFlightRef.current = true;
    const chat = useAgentChatStore.getState();
    // Resolve any `/skill-name` tokens in the draft text against the
    // skills registry. Same parser the live pane uses; bodies are
    // injected as a per-turn prefix by `materializeAndSend`.
    const skillBodies = resolveSkillBodies(
      text,
      selectActiveSkills(useSkillsStore.getState()),
    );
    // Step 8 Stage 2 — snapshot staged attachments at submit time
    // (live store read avoids stale closure if a chip resolved
    // between the last render and Enter). Stage 2.1: filter to
    // attachments whose `@<basename>` token is still in the textarea
    // text, so deleting a token excludes the file from the first
    // turn injection.
    const liveSlice = useAgentChatStore.getState().threads[currentDraft.threadId];
    const liveAttachments = liveSlice?.stagedAttachments ?? [];
    const attachmentBlock = buildAttachmentBlock(
      activeAttachments(text, liveAttachments),
    );
    // The staged images as `data:` URLs so both the pending conversation
    // below AND the optimistic user bubble in the new pane render their
    // thumbnails immediately (built from the in-memory bytes).
    const imageDisplaySources = buildImageDisplaySources(liveAttachments);

    // Thread Scope redesign — when the checkout popover is set to
    // "worktree", pass along the project root to fork from so
    // `materializeAndSend` defers worktree creation to this submit
    // (auto-naming from `text` when the name field was left empty).
    // `null` for "current" checkout or a home target (nothing to
    // fork), which keeps every pre-existing target-resolution path
    // byte-identical.
    const worktreeProjectPath =
      currentDraft.checkoutMode === "worktree"
        ? currentDraft.target.kind === "project"
          ? currentDraft.target.projectPath
          : currentDraft.target.kind === "existing_workspace"
            ? existingWorkspaceProjectRoot
            : null
        : null;

    // Instant feedback (Bug 2 fix): render the just-sent conversation and
    // clear the composer NOW, before the multi-second workspace/session
    // work begins. The status line advances through the real phases via
    // `materializeAndSend`'s `onPhase` hook.
    const finalDraft = currentDraft;
    setPending({
      text,
      images: imageDisplaySources,
      phase: worktreeProjectPath ? "creating-worktree" : "creating-workspace",
    });
    updateDraftInput(finalDraft.draftId, "");

    void (async () => {
      // Images stage at attach time; await only stragglers, then read
      // fresh so staging patches (new attachment objects) are visible.
      const imageIds = imageAttachmentIds(liveAttachments);
      if (imageIds.length > 0) await awaitImageStaging(imageIds);
      const freshAttachments =
        useAgentChatStore.getState().threads[finalDraft.threadId]
          ?.stagedAttachments ?? [];
      const unstaged = unstagedImageAttachments(freshAttachments);
      if (unstaged.length > 0) {
        // A staged image never landed — block the send (the raw-bytes
        // fallback is gone) and restore the composer.
        setPending(null);
        updateDraftInput(finalDraft.draftId, text);
        toast.error(
          "An attached image failed to upload — remove it and try again.",
        );
        sendInFlightRef.current = false;
        return;
      }
      const imageRefs = buildImageRefs(freshAttachments);

      const result = await materializeAndSend(
        finalDraft,
        text,
        cwdForSession,
        {
          markPromoting: state.markPromoting,
          markMaterialized: state.markMaterialized,
          markPromoted: state.markPromoted,
          markSendFailed: state.markSendFailed,
          ensureThread: chat.ensureThread,
          appendUserMessage: chat.appendUserMessage,
          removeUserMessageByNonce: chat.removeUserMessageByNonce,
          setModel: chat.setModel,
          setPermissionMode: chat.setPermissionMode,
          setSessionLaunchMode: chat.setSessionLaunchMode,
          setEffort: chat.setEffort,
          setContextWindow: chat.setContextWindow,
          setFastMode: chat.setFastMode,
          setMode: chat.setMode,
        },
        skillBodies,
        attachmentBlock,
        imageRefs,
        imageDisplaySources,
        worktreeProjectPath,
        (phase) => setPending((p) => (p ? { ...p, phase } : p)),
      );
      if (result.success) {
        // Flip to the real pane. The pending view stays mounted until
        // this surface unmounts, and the live pane renders the same
        // optimistically-appended bubble — flicker-free.
        setActiveDraft(null);
        // Clear the chips per-turn so the fresh AgentChatPane mount
        // doesn't render leftover chips from the draft surface.
        clearStagedAttachments(finalDraft.threadId);
        // §9: keep the draft entry for a grace period so any in-flight
        // UI can still observe the promotion before cleanup.
        const draftIdToClear = finalDraft.draftId;
        setTimeout(() => clearDraft(draftIdToClear), CLEAR_AFTER_PROMOTION_MS);
      } else {
        // `materializeAndSend` already rolled back its optimistic slice
        // bubble; drop the pending view and restore the composer text so
        // the user can retry. The error also surfaces via
        // `draft.lastSendError` on the composer.
        setPending(null);
        updateDraftInput(finalDraft.draftId, text);
        toast.error(`Send failed: ${result.error}`);
      }
      sendInFlightRef.current = false;
    })();
  }, [
    draft.draftId,
    appHomeDir,
    existingWorkspaceCwd,
    existingWorkspaceProjectRoot,
    activeSidebarWorkspaceId,
    activeSidebarProjectPath,
    setActiveDraft,
    clearDraft,
    clearStagedAttachments,
    updateDraftInput,
  ]);

  // Step 8 Stage 2 — chip lifecycle for the `@` mention popup.
  // Mirrors AgentChatPane's `handleAttachFile`: stage with
  // isLoading=true, fire `read_file_for_attachment`, patch the chip
  // with the resolved body (or an error indicator on failure). The
  // slice key is `draft.threadId` (pre-minted; reused by materialize)
  // so chips survive the draft → real-thread transition without
  // a separate transfer step.
  const handleAttachFile = useCallback(
    (match: FileMatch) => {
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const filename = basename(match.path);
      addStagedAttachment(draft.threadId, {
        id,
        kind: "file",
        ref: match.absolute_path,
        metadata: { label: filename, isLoading: true },
      });
      void (async () => {
        try {
          // `displayCwd` is the resolved cwd for the draft; null when
          // Home is selected. Passing null means relative_path comes
          // back null, which is fine — the chip falls back to the
          // filename and the agent reads via absolute path anyway.
          const info = await readFileForAttachment(
            match.absolute_path,
            displayCwd ?? null,
          );
          updateStagedAttachment(draft.threadId, id, {
            resolvedContent: buildFileResolvedContent(info),
            metadata: {
              label: filename,
              lineCount: info.lineCount,
              bytes: info.bytes,
              isLoading: false,
              fetchedAt: Date.now(),
            },
          });
        } catch (err) {
          updateStagedAttachment(draft.threadId, id, {
            metadata: {
              label: filename,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [
      draft.threadId,
      displayCwd,
      addStagedAttachment,
      updateStagedAttachment,
    ],
  );

  /** Step 8 Stage 6 — image counterpart for the draft surface.
   *  Mirrors AgentChatPane's `handleAttachImage`: validates the MIME,
   *  stages a chip with isLoading=true, resolves with the decoded
   *  bytes. Uses `draft.threadId` so chips survive the materialize
   *  transition. Resolved bytes live in-memory only — a session
   *  restart re-prompts the user to re-paste. */
  const handleAttachImage = useCallback(
    async (file: File) => {
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
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const label = file.name || `pasted-image-${Date.now()}.png`;
      addStagedAttachment(draft.threadId, {
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
        const buffer = await file.arrayBuffer();
        const bytes = new Uint8Array(buffer);
        updateStagedAttachment(draft.threadId, id, {
          resolvedImage: { mime: file.type, bytes },
          metadata: {
            label,
            bytes: file.size,
            isLoading: false,
            fetchedAt: Date.now(),
          },
        });
        // Stage the bytes to disk NOW (off the send path); patches the
        // chip with `stagedImage` when it lands, or a chip error on
        // failure.
        beginImageStaging(draft.threadId, id, bytes, file.type);
      } catch (err) {
        updateStagedAttachment(draft.threadId, id, {
          metadata: {
            label,
            bytes: file.size,
            isLoading: false,
            error: String(err),
          },
        });
      }
    },
    [draft.threadId, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 3 — folder counterpart for the draft surface.
   *  Uses `draft.threadId` so chips survive the materialize → real
   *  thread transition without a transfer step. */
  const handleAttachFolder = useCallback(
    (match: FolderMatch) => {
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const folderName = basename(match.path);
      addStagedAttachment(draft.threadId, {
        id,
        kind: "folder",
        ref: match.absolute_path,
        metadata: { label: folderName, isLoading: true },
      });
      void (async () => {
        try {
          const info = await readFolderForAttachment(
            match.absolute_path,
            displayCwd ?? null,
            2,
          );
          updateStagedAttachment(draft.threadId, id, {
            resolvedContent: buildFolderResolvedContent(info),
            metadata: {
              label: folderName,
              isLoading: false,
              fetchedAt: Date.now(),
            },
          });
        } catch (err) {
          updateStagedAttachment(draft.threadId, id, {
            metadata: {
              label: folderName,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [
      draft.threadId,
      displayCwd,
      addStagedAttachment,
      updateStagedAttachment,
    ],
  );

  /** Step 8 Stage 4 — issue counterpart for the draft surface. Same
   *  loading-then-resolved chip lifecycle as AgentChatPane; targets
   *  `draft.threadId` so chips survive the materialize handoff. */
  const handleAttachIssue = useCallback(
    (summary: GitHubIssue) => {
      if (!displayCwd) return;
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const initialState = (summary.state.toLowerCase() === "closed"
        ? "closed"
        : "open") as "open" | "closed";
      addStagedAttachment(draft.threadId, {
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
          const detail = await getGithubIssueByPath(displayCwd, summary.number);
          updateStagedAttachment(draft.threadId, id, {
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
          updateStagedAttachment(draft.threadId, id, {
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
    [
      draft.threadId,
      displayCwd,
      addStagedAttachment,
      updateStagedAttachment,
    ],
  );

  /** Step 8 Stage 5 — PR counterpart for the draft surface. */
  const handleAttachPr = useCallback(
    (summary: PullRequestInfo) => {
      if (!displayCwd) return;
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
      addStagedAttachment(draft.threadId, {
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
          const [detail, diff] = await Promise.all([
            getGithubPrByPath(displayCwd, summary.number),
            getGithubPrDiffByPath(displayCwd, summary.number, false),
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
          updateStagedAttachment(draft.threadId, id, {
            resolvedContent: buildPrResolvedContent(detail, diff),
            metadata: {
              label: `#${detail.number} ${detail.title}`,
              state: resolvedState,
              fetchedAt: Date.now(),
              isLoading: false,
            },
          });
        } catch (err) {
          updateStagedAttachment(draft.threadId, id, {
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
    [
      draft.threadId,
      displayCwd,
      addStagedAttachment,
      updateStagedAttachment,
    ],
  );

  // Step 8 Stage 4 — preflight GitHub status. Mirrors AgentChatPane.
  // `null` means "not yet known"; the popup keeps GitHub entries
  // disabled while the preflight is in flight to avoid a flicker.
  const [isGithubRepo, setIsGithubRepo] = useState<boolean | null>(null);
  const [ghAuthenticated, setGhAuthenticated] = useState<boolean | null>(null);
  useEffect(() => {
    if (!displayCwd) {
      setIsGithubRepo(false);
      setGhAuthenticated(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const [repo, gh] = await Promise.all([
          checkGithubRepo(displayCwd),
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
  }, [displayCwd]);

  // The textarea onStop is a no-op during a draft — there's no session
  // to interrupt. The button only shows when streaming=true, which we
  // set only during the in-flight materialise window. Stopping
  // mid-materialise would leave orphan state on the backend; we
  // intentionally don't wire it.
  const handleStop = useCallback(() => {
    // No-op — see comment above.
  }, []);

  // Provider change during a draft is cheap because nothing is live —
  // just update the draft's recorded provider. A side effect: the
  // permission-mode default differs between providers, so we seed a
  // sensible default whenever the provider flips.
  const handleProviderModelChange = useCallback(
    (nextProvider: ChatDraft["provider"], nextModel: string) => {
      if (nextProvider === draft.provider) {
        updateDraftConfig(draft.draftId, { model: nextModel });
        return;
      }
      const defaults = capabilityDefaults(nextProvider, nextModel);
      updateDraftConfig(draft.draftId, {
        provider: nextProvider,
        model: nextModel,
        // Commit the next provider's native permission default (via
        // capabilityDefaults → defaultPermissionModeForProvider). The
        // picker can *display* a fallback for null/unknown values without
        // mutating the draft, so leaving this null would make first-send
        // launch state diverge from the visible "Full access" label.
        permissionMode: defaults.permissionMode,
        effort: defaults.effort,
        contextWindow: defaults.contextWindow,
        // Speed tiers are provider/model-scoped: never carry a Fast
        // selection across a provider switch.
        fastMode: false,
      });
    },
    [draft.draftId, draft.provider, updateDraftConfig],
  );

  const handleModelChange = useCallback(
    (next: string) => {
      const nextModel = capabilities?.models.find((item) => item.id === next);
      updateDraftConfig(draft.draftId, {
        model: next,
        ...(draft.fastMode && nextModel && !nextModel.supports_fast_mode
          ? { fastMode: false }
          : {}),
      });
    },
    [draft.draftId, draft.fastMode, capabilities, updateDraftConfig],
  );
  const handlePermissionModeChange = useCallback(
    (next: string) =>
      updateDraftConfig(draft.draftId, { permissionMode: next }),
    [draft.draftId, updateDraftConfig],
  );
  const handleEffortChange = useCallback(
    (next: string) => updateDraftConfig(draft.draftId, { effort: next }),
    [draft.draftId, updateDraftConfig],
  );
  const handleContextWindowChange = useCallback(
    (next: string) =>
      updateDraftConfig(draft.draftId, { contextWindow: next }),
    [draft.draftId, updateDraftConfig],
  );
  const handleFastModeChange = useCallback(
    (next: boolean) =>
      updateDraftConfig(draft.draftId, { fastMode: next }),
    [draft.draftId, updateDraftConfig],
  );

  // Drafts have no live session yet, so mode activation / removal
  // is state-only: we just stamp the new mode on the draft. On
  // materialize, `effectivePermissionMode` reads `draft.mode` and
  // overrides `permission_mode` for the `start_session` call (Plan
  // boots the SDK straight into plan mode — no restart needed
  // because there's no session to restart). Ask / Debug are
  // tracked at the slice level only; their prompt-wrappers land in
  // Stages 4 & 6.
  const handleModeActivate = useCallback(
    (newMode: ActivePillMode) => {
      updateDraftConfig(draft.draftId, { mode: newMode });
    },
    [draft.draftId, updateDraftConfig],
  );
  const handleModeRemove = useCallback(() => {
    updateDraftConfig(draft.draftId, { mode: "default" });
  }, [draft.draftId, updateDraftConfig]);

  // Thread Scope redesign — the row rendered below the composer via
  // `belowComposerSlot`. Replaces the above-composer ProjectPicker
  // (home target) / WorktreePicker + DerivativeBranchPicker (project /
  // existing_workspace targets) that used to occupy `zone1Override`.
  // Worktree creation is no longer immediate: `checkoutMode` +
  // `worktreeName` + `baseBranch` are just draft config now, and
  // `handleSubmit` passes the resolved project path through to
  // `materializeAndSend`, which creates the worktree (auto-named from
  // the first message when the name is empty) as part of the same
  // submit that sends it.
  const handleChangeTarget = useCallback(
    (target: ChatDraft["target"]) => updateDraftTarget(draft.draftId, target),
    [draft.draftId, updateDraftTarget],
  );
  const handleChangeCheckoutMode = useCallback(
    (mode: "current" | "worktree") =>
      updateDraftConfig(draft.draftId, { checkoutMode: mode }),
    [draft.draftId, updateDraftConfig],
  );
  const handleChangeWorktreeName = useCallback(
    (name: string) => updateDraftConfig(draft.draftId, { worktreeName: name }),
    [draft.draftId, updateDraftConfig],
  );
  const handleChangeBaseBranch = useCallback(
    (branch: string) => updateDraftConfig(draft.draftId, { baseBranch: branch }),
    [draft.draftId, updateDraftConfig],
  );

  const belowComposerSlot = (
    <ThreadScopeRow
      location={{
        kind: "draft",
        target: draft.target,
        onChangeTarget: handleChangeTarget,
      }}
      projectPath={scopeProjectPath}
      checkoutMode={draft.checkoutMode ?? "current"}
      worktreeName={draft.worktreeName ?? ""}
      baseBranch={draft.baseBranch ?? ""}
      disabled={draft.promoting}
      onChangeCheckoutMode={handleChangeCheckoutMode}
      onChangeWorktreeName={handleChangeWorktreeName}
      onChangeBaseBranch={handleChangeBaseBranch}
    />
  );

  // Design D10/D12 → Thread Scope: home drafts read "Message the
  // agent…"; project / existing_workspace drafts keep the existing
  // isDraft-derived default ("Describe what you want the agent to
  // do…") by passing `undefined`.
  const placeholderOverride =
    draft.target.kind === "home" ? "Message the agent…" : undefined;

  const composerEl = (
    <Composer
      draft={draft.inputDraft}
      cwd={displayCwd}
      provider={draft.provider}
      model={draft.model}
      permissionMode={draft.permissionMode}
      effort={draft.effort}
      contextWindow={draft.contextWindow}
      fastMode={draft.fastMode ?? false}
      activeModel={activeModel}
      effortLabelMap={effortLabelMap}
      permissionModes={permissionModes}
      ultrathinkInBodyText={hasUltrathinkInBodyText(draft.inputDraft)}
      // While promoting, treat the composer like a streaming chat so
      // the submit button disables and a spinner could be added later.
      streaming={draft.promoting}
      // Drafts do not need a backend session to be considered ready —
      // Enter-to-send is always available as long as text is present.
      sessionReady={true}
      // No live session yet — drives the draft-variant placeholder copy.
      isDraft={true}
      placeholderOverride={placeholderOverride}
      showProviderPicker={true}
      showStopButton={false}
      errorMessage={draft.lastSendError}
      // Thread Scope redesign — the cwd label above the composer moved
      // into ThreadScopeRow's location control below it; `null` hides
      // Composer's own zone-1 slot rather than falling back to a plain
      // cwd string.
      zone1Override={null}
      belowComposerSlot={belowComposerSlot}
      mode={draft.mode}
      stagedAttachments={stagedAttachments}
      onRemoveAttachment={(id) => {
        // Best-effort delete of the backend staging file if this chip
        // was an image that already staged.
        const staged = useAgentChatStore
          .getState()
          .threads[draft.threadId]?.stagedAttachments.find((a) => a.id === id)
          ?.stagedImage;
        if (staged) discardStagedImage(staged.path);
        removeStagedAttachment(draft.threadId, id);
      }}
      onAttachFile={handleAttachFile}
      onAttachFolder={handleAttachFolder}
      onAttachIssue={handleAttachIssue}
      onAttachPr={handleAttachPr}
      onAttachImage={handleAttachImage}
      modelSupportsImages={activeModel?.supports_images ?? false}
      isGithubRepo={isGithubRepo}
      ghAuthenticated={ghAuthenticated}
      onDraftChange={(next) => updateDraftInput(draft.draftId, next)}
      onSubmit={handleSubmit}
      onStop={handleStop}
      onProviderModelChange={handleProviderModelChange}
      onModelChange={handleModelChange}
      onPermissionModeChange={handlePermissionModeChange}
      onEffortChange={handleEffortChange}
      onContextWindowChange={handleContextWindowChange}
      onFastModeChange={handleFastModeChange}
      onModeActivate={handleModeActivate}
      onModeRemove={handleModeRemove}
    />
  );

  // GUI chrome (Agent Chat Beta) suppresses the placeholder header the
  // same way it suppresses AgentChatPaneHeader for a sole-root chat
  // pane — the titlebar's "Agent Chat" draft pill covers the label
  // (see TitleBarDraftSlots in title-bar.tsx). Legacy chrome keeps the
  // h-7 band so the draft doesn't look "naked" next to a real pane.
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);

  return (
    <div className="flex h-full w-full flex-col bg-background">
      {!enableAgentChat && <DraftSurfaceHeader />}
      <div className="flex-1 min-h-0 overflow-hidden">
        {pending ? (
          <DraftPendingConversation pending={pending} composer={composerEl} />
        ) : (
          <ChatHomeLanding composer={composerEl} />
        )}
      </div>
    </div>
  );
}

/** Local instant-feedback state for a submit that's mid-materialise. */
interface DraftPendingState {
  text: string;
  images: UserMessageImage[];
  phase: MaterializePhase;
}

const PHASE_LABEL: Record<MaterializePhase, string> = {
  "creating-worktree": "Creating worktree…",
  "creating-workspace": "Setting up workspace…",
  "starting-session": "Starting session…",
  sending: "Sending…",
};

/**
 * The just-sent conversation, rendered the instant the user submits so
 * the composer never sits dead through the multi-second workspace +
 * session bring-up. Shows the sent user bubble (with `data:` URL
 * thumbnails) above a shimmering status line that advances through the
 * real materialize phases, with the (cleared) composer pinned below —
 * the same column geometry as the live pane, so the hand-off to the real
 * `AgentChatPane` is flicker-free.
 */
function DraftPendingConversation({
  pending,
  composer,
}: {
  pending: DraftPendingState;
  composer: ReactNode;
}) {
  const item: UserMessageItem = {
    kind: "user_message",
    id: "draft-pending",
    seq: 0,
    text: pending.text,
    images: pending.images.length > 0 ? pending.images : undefined,
  };
  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex-1 min-h-0 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-[760px] flex-col gap-4 px-7 py-6">
          <UserMessage item={item} />
          <div
            className="flex items-center gap-[13px] pt-0.5"
            role="status"
            aria-label="Starting the agent"
          >
            <span className="flex w-[29px] shrink-0 justify-center">
              <LoaderCircle
                className="h-[15px] w-[15px] animate-spin text-accent-ember"
                strokeWidth={1.6}
                aria-hidden
              />
            </span>
            <span className="shimmer text-[12.5px] font-semibold">
              {PHASE_LABEL[pending.phase]}
            </span>
          </div>
        </div>
      </div>
      <div className="mx-auto w-full max-w-[760px] px-7 pb-4">{composer}</div>
    </div>
  );
}

/**
 * Placeholder chrome that matches AgentChatPaneHeader's visual band
 * (h-7 border-b) so the draft surface doesn't look "naked" next to a
 * materialized pane. Drafts have no session yet, so the session
 * selector / split / close controls from the real pane header don't
 * apply — we only borrow the silhouette.
 */
function DraftSurfaceHeader() {
  return (
    <header
      className="flex h-7 shrink-0 items-center gap-1 border-b border-border/50 bg-background px-1.5"
      data-testid="draft-surface-header"
    >
      <span className="px-1.5 text-xs text-muted-foreground">Agent Chat</span>
    </header>
  );
}
