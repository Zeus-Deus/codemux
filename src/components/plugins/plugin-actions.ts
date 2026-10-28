/**
 * Running plugin actions and applying what they ask of this client. The
 * target is captured before anything is awaited; results that arrive after
 * the user moved away never write into whatever became active — they are
 * offered as a toast instead.
 */
import { composerFor } from "@/contrib/composer-registry";
import { mobileLayoutAvailable } from "@/hooks/use-mobile-layout";
import { openExternalUrl } from "@/lib/open-url";
import { openNotificationTarget } from "@/lib/open-notification-target";
import { toast } from "@/lib/toast";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { createBrowserPane } from "@/tauri/commands";
import type { PluginAction, PluginActionContext, PluginClientIntent, PluginInstallation } from "./contract";
import { planPluginIntents, pluginUnsupportedIntentCount, pluginUnsupportedIntentNotice, type PluginIntentStep } from "./intents";
import { newPluginOperationId, pluginActionHandler, pluginFailure, pluginFailureNeedsDetails } from "./model";
import { platformInvoke } from "./platform-adapter";
import { notifyPluginThreadActivity } from "./thread-status";
import {
  currentPluginLocationKey,
  findThreadLocation,
  parsePluginComposerKey,
  pluginComposerKey,
  workspaceActiveThreadId,
  workspaceById,
} from "./workspace-context";

/** What the user acted on, fixed before anything is awaited. */
export interface PluginInvocationTarget {
  readonly installation: PluginInstallation;
  readonly workspaceId: string | null;
  readonly threadId: string | null;
  /** The composer an insert may write to; only actions beside a chat have one. */
  readonly composerKey: string | null;
  readonly locationKey: string;
}

/** Captures where the user is acting from, now. */
export function capturePluginTarget(input: {
  installation: PluginInstallation;
  workspaceId: string | null;
  threadId: string | null;
  /** Whether results may write into this chat's composer. */
  composer: boolean;
}): PluginInvocationTarget {
  return {
    installation: input.installation,
    workspaceId: input.workspaceId,
    threadId: input.threadId,
    composerKey:
      input.composer && input.workspaceId !== null && input.threadId !== null
        ? pluginComposerKey(input.workspaceId, input.threadId)
        : null,
    locationKey: currentPluginLocationKey(),
  };
}

/** Page ids for `ui.showPluginPage`; `PlatformPluginPage` resolves them live. */
// @ cannot begin a renderer plugin id, so its pages and contributions cannot collide.
export const PLATFORM_PAGE_PREFIX = "@platform:";
export const platformPageId = (installationId: string, viewId: string) =>
  `${PLATFORM_PAGE_PREFIX}${installationId}:${viewId}`;
export function parsePlatformPageId(id: string): { installationId: string; viewId: string } | null {
  if (!id.startsWith(PLATFORM_PAGE_PREFIX)) return null;
  const rest = id.slice(PLATFORM_PAGE_PREFIX.length);
  const at = rest.indexOf(":");
  return at <= 0 ? null : { installationId: rest.slice(0, at), viewId: rest.slice(at + 1) };
}

/**
 * Right-panel contribution id for a side-panel view. Keyed by installation,
 * which a reinstall from the same source keeps, so the workspace's saved
 * panel survives it while another source's plugin with the same id never
 * takes it over.
 */
export const platformSidePanelId = (installationId: string, viewId: string) => `@${installationId}:view.${viewId}`;

/** The phone layout, where the right panel is not shown (see `useMobileLayout`). */
function phoneLayout(): boolean {
  return (
    mobileLayoutAvailable() && typeof window.matchMedia === "function" && window.matchMedia("(max-width: 1023px)").matches
  );
}

/** Whether the captured chat is the one its workspace shows, with that workspace on screen. */
function threadOnScreen(target: Pick<PluginInvocationTarget, "workspaceId" | "threadId">): boolean {
  if (selectActiveWorkspaceId(useAppStore.getState()) !== target.workspaceId) return false;
  return target.threadId === null || workspaceActiveThreadId(workspaceById(target.workspaceId)) === target.threadId;
}

/**
 * Side panels open beside the chat they were asked from; anything else opens
 * as a page. A side panel takes its context from the chat its workspace shows,
 * so when the user has since moved, even to another chat in the same
 * workspace, that chat is shown first, and nothing opens if it can't be.
 */
export async function openPlatformView(
  target: Pick<PluginInvocationTarget, "installation" | "workspaceId" | "threadId">,
  viewId: string,
): Promise<void> {
  const view = target.installation.manifest.views.find((entry) => entry.id === viewId);
  if (!view || view.placement === "conversation") return;
  const besideChat = view.placement === "side-panel" && target.workspaceId !== null;
  let workspaceId = target.workspaceId;
  if (besideChat && target.threadId !== null && !threadOnScreen(target)) {
    const revealed = await revealThread(target.threadId);
    if (revealed === null) return;
    workspaceId = revealed;
  }
  const ui = useUIStore.getState();
  // Settings replaces the whole app, and a utility page the workspace.
  if (ui.showSettings) ui.setShowSettings(false);
  // A phone has no right panel: the side panel opens full screen beside the
  // same chat (`PlatformPluginPage` gives it the active chat's context).
  if (besideChat && workspaceId !== null && !phoneLayout()) {
    ui.closeUtilityPages();
    if (ui.addProjectSheet) ui.setAddProjectSheet(null);
    ui.setRightPanelTab(workspaceId, `plugin:${platformSidePanelId(target.installation.installationId, view.id)}` as never);
    return;
  }
  ui.setShowPluginPage(platformPageId(target.installation.installationId, view.id));
}

/** Shows a chat; resolves to its workspace once it is on screen, or null when it can't be. */
async function revealThread(threadId: string): Promise<string | null> {
  const location = findThreadLocation(threadId);
  if (location === null) {
    toast.info("That chat is not open.");
    return null;
  }
  const workspaceId = location.workspace.workspace_id;
  return (await openNotificationTarget({ workspace_id: workspaceId, pane_id: location.paneId })) ? workspaceId : null;
}

/** Only http(s) URLs open anywhere; the host and the view bridge check too. */
export function pluginPreviewUrl(raw: string): string | null {
  try {
    const url = new URL(raw);
    return (url.protocol === "http:" || url.protocol === "https:") && url.username === "" && url.password === ""
      ? url.toString()
      : null;
  } catch {
    return null;
  }
}

/**
 * The chat pane an in-app preview opens beside: the invoking chat, while it
 * is still open in the workspace the user invoked from and that workspace is
 * still the one on screen. Phones have no split panes, so links go to the
 * browser there.
 */
export function pluginPreviewPane(target: Pick<PluginInvocationTarget, "workspaceId" | "threadId">): string | null {
  if (target.workspaceId === null || target.threadId === null || phoneLayout()) return null;
  if (selectActiveWorkspaceId(useAppStore.getState()) !== target.workspaceId) return null;
  const location = findThreadLocation(target.threadId);
  return location !== null && location.workspace.workspace_id === target.workspaceId ? location.paneId : null;
}

/**
 * Opens a plugin's link in a Codemux browser pane beside the invoking chat
 * when the result is applied where the user still is; an offered link (the
 * user moved on, or clicked it later) goes to the system browser, so nothing
 * opens inside a workspace the user is not looking at.
 */
async function openPluginPreview(
  target: PluginInvocationTarget,
  url: string,
  mode: "apply" | "offer",
): Promise<void> {
  const safe = pluginPreviewUrl(url);
  if (safe === null) {
    toast.warning(`${target.installation.manifest.name} request ignored`, { description: "Links must use http or https." });
    return;
  }
  const paneId = mode === "apply" ? pluginPreviewPane(target) : null;
  if (paneId !== null) {
    try {
      await createBrowserPane(paneId, safe);
      return;
    } catch {
      // No browser pane here (or the pane went away): the system browser still opens it.
    }
  }
  await openExternalUrl(safe);
}

function runNavigation(target: PluginInvocationTarget, step: Extract<PluginIntentStep, { kind: "navigate" }>): void {
  const { intent } = step;
  switch (intent.type) {
    case "thread.reveal":
      void revealThread(intent.threadId);
      return;
    case "view.open":
      void openPlatformView(target, intent.viewId);
      return;
    case "preview.open":
      void openPluginPreview(target, intent.url, step.mode);
  }
}

const offerLabel = (step: Extract<PluginIntentStep, { kind: "navigate" }>) =>
  step.intent.type === "thread.reveal"
    ? { title: "wants to open a chat", action: "Open chat" }
    : step.intent.type === "view.open"
      ? { title: "wants to open a view", action: "Open" }
      : { title: "suggested a link", action: "Open link" };

/** Applies a result's intents to this client, judged against the captured target. */
export function applyPluginIntents(
  target: PluginInvocationTarget,
  intents: ReadonlyArray<PluginClientIntent>,
  unsupported = 0,
): void {
  const name = target.installation.manifest.name;
  if (unsupported > 0) {
    const notice = pluginUnsupportedIntentNotice(name, unsupported);
    toast.warning(notice.title, { description: notice.description });
  }
  const steps = planPluginIntents({
    origin: {
      installationId: target.installation.installationId,
      locationKey: target.locationKey,
      composerKey: target.composerKey,
      threadId: target.threadId,
    },
    manifest: target.installation.manifest,
    intents,
    currentLocationKey: currentPluginLocationKey(),
    // A link applies (opens in a pane beside the chat) only where that pane exists now.
    canOpenPreview: pluginPreviewPane(target) !== null,
  });
  for (const step of steps) {
    if (step.kind === "rejected") {
      toast.warning(`${name} request ignored`, { description: step.reason });
      continue;
    }
    if (step.kind === "insert") {
      const composer = parsePluginComposerKey(step.composerKey);
      const handle = composer ? composerFor(composer.workspaceId, composer.threadId) : null;
      if (!handle) {
        toast.error("Couldn't add to chat", { description: "That chat's composer isn't open." });
        continue;
      }
      handle.insertText(step.text);
      if (!step.visible && composer) {
        toast.info(`${name} added text to a draft`, {
          description: "It is in the composer of the chat you started from.",
          action: { label: "Open chat", onClick: () => void revealThread(composer.threadId) },
        });
      }
      continue;
    }
    if (step.mode === "apply") {
      runNavigation(target, step);
      continue;
    }
    const label = offerLabel(step);
    toast.info(`${name} ${label.title}`, {
      ...(step.intent.type === "preview.open" ? { description: step.intent.url } : {}),
      // Clicked later, from wherever the user is: never an in-app pane in another workspace.
      action: { label: label.action, onClick: () => runNavigation(target, { ...step, mode: "offer" }) },
    });
  }
}

/** Opens Settings → Plugins, where a stopped plugin's details and repair live. */
export const openPluginSettings = (installationId?: string) => useUIStore.getState().openPluginSettings(installationId);

/** Runs a declared action against the target the user picked it from. */
export async function runPluginAction(
  target: PluginInvocationTarget,
  action: PluginAction,
  context: PluginActionContext,
): Promise<void> {
  const { installation } = target;
  const name = installation.manifest.name;
  try {
    const result = await platformInvoke({
      installationId: installation.installationId,
      generation: installation.generation,
      handler: pluginActionHandler(action.id),
      context,
      operationId: newPluginOperationId(),
    });
    if (result.message !== undefined && result.message.trim() !== "") {
      toast.success(action.title, { description: result.message });
    }
    const intents = result.intents ?? [];
    const unsupported = pluginUnsupportedIntentCount(result);
    if (intents.length > 0 || unsupported > 0) applyPluginIntents(target, intents, unsupported);
  } catch (error) {
    const failure = pluginFailure(error);
    toast.error(`${name}: ${action.title} failed`, {
      description: failure.message,
      // A dormant plugin first fails here; point at its details in Settings.
      ...(pluginFailureNeedsDetails(failure) ? { action: { label: "Details", onClick: () => openPluginSettings(installation.installationId) } } : {}),
    });
  } finally {
    if (context.threadId !== undefined) {
      notifyPluginThreadActivity({ threadId: context.threadId, installationId: installation.installationId });
    }
  }
}
