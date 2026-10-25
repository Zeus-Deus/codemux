import { useEffect, useRef } from "react";
import { FolderTree, PanelRight, PanelsTopLeft, Puzzle } from "lucide-react";
import { registry } from "@/contrib/registry";
import type { Contribution } from "@/contrib/types";
import { COMPOSER_STATUS_AREA, PALETTE_AREA, RIGHT_PANEL_AREA } from "@/sdk/areas";
import type { ComposerStatusProps, PaletteCommandData, RightPaneData } from "@/sdk/host";
import { useSidebarViewStore } from "@/components/layout/sidebar-view-store";
import { selectUtilityPage } from "@/lib/utility-pages";
import { toast } from "@/lib/toast";
import { useUIStore } from "@/stores/ui-store";
import type { PluginInstallation } from "./contract";
import { pluginActionEntries, pluginAttentionNotice, pluginRunnableInstallations, pluginViewEntries } from "./model";
import {
  applyPluginIntents,
  capturePluginTarget,
  openPluginSettings,
  platformPageId,
  platformSidePanelId,
  runPluginAction,
} from "./plugin-actions";
import { PLATFORM_CONTRIBUTION_SOURCE, refreshPlatformCatalogue, usePlatformCatalogue } from "./platform-adapter";
import { PlatformSidePanel } from "./PlatformPluginViews";
import { pluginSidebarLayoutOptions } from "./plugin-sidebar-layout";
import {
  PluginThreadStatusChips,
  PluginThreadStatusDriver,
  pluginThreadStatusSlotData,
} from "./PluginThreadStatuses";
import { readPluginStartPage, resolvePluginStartPage, writePluginStartPage } from "./start-page";
import { pluginProjectIdOf, workspaceActiveThreadId, workspaceById } from "./workspace-context";

function threadTarget(installation: PluginInstallation, workspaceId: string | null) {
  const workspace = workspaceById(workspaceId);
  const threadId = workspaceActiveThreadId(workspace);
  return {
    target: capturePluginTarget({ installation, workspaceId: workspace?.workspace_id ?? null, threadId, composer: true }),
    context: {
      ...(workspace ? { projectId: pluginProjectIdOf(workspace) } : {}),
      ...(threadId === null ? {} : { threadId }),
    },
  };
}

const statusChip = (props: ComposerStatusProps) => <PluginThreadStatusChips {...props} />;

/** Stable render functions per side panel, so the right panel keeps its mounted view. */
const sidePanelRenders = new Map<string, (props: { workspaceId: string }) => React.ReactNode>();
function sidePanelRender(installationId: string, viewId: string) {
  const key = `${installationId}\0${viewId}`;
  let render = sidePanelRenders.get(key);
  if (!render) {
    render = ({ workspaceId }) => (
      <PlatformSidePanel installationId={installationId} viewId={viewId} workspaceId={workspaceId} />
    );
    sidePanelRenders.set(key, render);
  }
  return render;
}

/**
 * Offers isolated platform plugins through the app's existing slots: palette
 * actions and view openers, side panels in the right panel, compact chat
 * statuses, and sidebar layouts. Everything is derived from the live
 * catalogue, so a plugin that stops, is disabled or becomes unavailable
 * disappears from every entry point at once. Mount once at the app root.
 */
export function PlatformPluginContributions() {
  const { catalogue } = usePlatformCatalogue();
  const installations = pluginRunnableInstallations(catalogue);
  const layoutChoice = useSidebarViewStore((state) => state.pluginLayout);

  // Keyed by what is offered, not by record identity: a status-only catalogue
  // refresh must not re-register (and remount) every contribution.
  const actions = pluginActionEntries(installations, "palette");
  const views = pluginViewEntries(installations);
  const layouts = pluginSidebarLayoutOptions(installations);
  const signature = JSON.stringify([
    actions.map(({ installation, action }) => [installation.installationId, installation.generation, action.id, action.title]),
    views.map(({ installation, view }) => [installation.installationId, installation.generation, view.id, view.title, view.placement]),
    layouts.map((layout) => [layout.key, layout.title]),
    layoutChoice,
  ]);
  const offered = useRef({ signature: "", value: { actions, views, layouts } });
  if (offered.current.signature !== signature) offered.current = { signature, value: { actions, views, layouts } };

  useEffect(() => {
    const latest = offered.current.value;
    const contributions: Contribution[] = [
      {
        id: "@platform:thread-status",
        area: COMPOSER_STATUS_AREA,
        source: PLATFORM_CONTRIBUTION_SOURCE,
        title: "Plugin status",
        render: statusChip,
        data: pluginThreadStatusSlotData,
      },
    ];
    for (const { installation, action } of latest.actions) {
      const name = installation.manifest.name;
      const data: PaletteCommandData = {
        // The plugin is named, so an action can't pass for a core command.
        label: `${action.title} · ${name}`,
        icon: Puzzle,
        keywords: ["plugin", name, action.description ?? ""].join(" "),
        run: ({ workspaceId }) => {
          const { target, context } = threadTarget(installation, workspaceId);
          void runPluginAction(target, action, context);
        },
      };
      contributions.push({
        // Installation ids are unique where manifest ids are not (two sources, one id).
        id: `@${installation.installationId}:action.${action.id}`,
        area: PALETTE_AREA,
        source: PLATFORM_CONTRIBUTION_SOURCE,
        title: `${action.title} · ${name}`,
        data,
      });
    }
    for (const { installation, view } of latest.views) {
      const name = installation.manifest.name;
      if (view.placement === "side-panel") {
        const paneData: RightPaneData = { icon: PanelRight };
        contributions.push({
          id: platformSidePanelId(installation.installationId, view.id),
          area: RIGHT_PANEL_AREA,
          source: PLATFORM_CONTRIBUTION_SOURCE,
          title: view.title,
          data: paneData,
          render: sidePanelRender(installation.installationId, view.id),
        });
      }
      const open: PaletteCommandData = {
        label: `Open ${view.title} · ${name}`,
        icon: PanelsTopLeft,
        keywords: ["plugin", "open", "view", name].join(" "),
        // A side panel needs a chat to sit beside.
        requiresWorkspace: view.placement === "side-panel",
        run: ({ workspaceId }) => {
          const { target } = threadTarget(installation, workspaceId);
          applyPluginIntents(target, [{ type: "view.open", viewId: view.id }]);
        },
      };
      contributions.push({
        id: `@${installation.installationId}:open.${view.id}`,
        area: PALETTE_AREA,
        source: PLATFORM_CONTRIBUTION_SOURCE,
        title: `Open ${view.title} · ${name}`,
        data: open,
      });
    }
    for (const option of latest.layouts) {
      if (option.key === layoutChoice) continue;
      const data: PaletteCommandData = {
        label: `Show ${option.title} in sidebar`,
        icon: FolderTree,
        keywords: ["sidebar", "layout", "projects", "plugin", option.pluginName].join(" "),
        run: () => useSidebarViewStore.getState().setPluginLayout(option.key),
      };
      contributions.push({
        id: `@${option.installationId}:layout.${option.kind}.${option.localId}`,
        area: PALETTE_AREA,
        source: PLATFORM_CONTRIBUTION_SOURCE,
        data,
      });
    }
    if (layoutChoice !== null) {
      const data: PaletteCommandData = {
        label: "Show Projects in sidebar",
        icon: FolderTree,
        keywords: "sidebar layout projects default",
        run: () => useSidebarViewStore.getState().setPluginLayout(null),
      };
      contributions.push({ id: "@platform:layout.default", area: PALETTE_AREA, source: PLATFORM_CONTRIBUTION_SOURCE, data });
    }
    return registry.registerMany(contributions);
  }, [signature, layoutChoice]);

  // A window coming back may have missed catalogue signals while asleep.
  useEffect(() => {
    const onFocus = () => void refreshPlatformCatalogue();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  return (
    <>
      <PluginThreadStatusDriver />
      <PluginStartPageLauncher />
      <PluginAttentionToast />
    </>
  );
}

const SEEN_ATTENTION_KEY = "codemux:plugins:seen-attention";

/**
 * Says once when plugins stop or become unavailable (often right after an app
 * update), pointing at their details. A new problem has a new key, so an old
 * dismissal never hides it. Settings keeps showing the state either way.
 */
function PluginAttentionToast() {
  const { catalogue } = usePlatformCatalogue();
  const notice = catalogue === null || catalogue.executionDisabled ? null : pluginAttentionNotice(catalogue.installations);
  const key = notice?.key ?? null;
  useEffect(() => {
    if (notice === null) return;
    let seen: string | null = null;
    try {
      seen = localStorage.getItem(SEEN_ATTENTION_KEY);
    } catch {
      // No storage: the toast may repeat, which is fine.
    }
    if (seen === notice.key) return;
    try {
      localStorage.setItem(SEEN_ATTENTION_KEY, notice.key);
    } catch {
      // As above.
    }
    const show = notice.tone === "error" ? toast.error : toast.warning;
    show(notice.title, {
      description: notice.description,
      action: { label: "Details", onClick: () => openPluginSettings(notice.entries.length === 1 ? notice.entries[0]?.installationId : undefined) },
    });
    // Keyed on the problem, not on the record objects that carry it.
  }, [key]);
  return null;
}

/** How long the start page may wait for the plugin list before the app just starts. */
const START_PAGE_WAIT_MS = 4_000;

/**
 * Opens this device's start page once per app load, if its plugin can serve
 * it by the time the plugin list loads. It never holds the app: anything
 * missing, failed, slow or not allowed leaves the usual start, and a page the
 * person opened meanwhile wins.
 */
function PluginStartPageLauncher() {
  const { catalogue, loaded } = usePlatformCatalogue();
  useEffect(() => {
    if (launchState !== "waiting") return;
    const preference = readPluginStartPage();
    if (preference === null) {
      launchState = "settled";
      return;
    }
    const timer = setTimeout(() => {
      launchState = "settled";
    }, START_PAGE_WAIT_MS);
    return () => clearTimeout(timer);
  }, []);
  useEffect(() => {
    if (launchState !== "waiting") return;
    const resolution = resolvePluginStartPage({
      preference: readPluginStartPage(),
      catalogue: !loaded ? "pending" : catalogue === null ? "unavailable" : "ready",
      installations: catalogue?.installations ?? [],
      executionDisabled: catalogue?.executionDisabled === true,
    });
    if (resolution.kind === "pending") return;
    launchState = "settled";
    if (resolution.kind !== "open") return;
    if (resolution.upgraded !== null) writePluginStartPage(resolution.upgraded);
    const ui = useUIStore.getState();
    if (ui.showSettings || selectUtilityPage(ui) !== null) return;
    ui.setShowPluginPage(platformPageId(resolution.installationId, resolution.viewId));
  }, [catalogue, loaded]);
  return null;
}

let launchState: "waiting" | "settled" = "waiting";

/** Test-only reset. */
export function resetPluginStartPageForTests(): void {
  launchState = "waiting";
}
