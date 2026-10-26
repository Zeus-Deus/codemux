/**
 * Plugin layouts for the sidebar's Projects view: declared views (rules over
 * the host's own project grouping) and native trees (a plugin-served
 * arrangement of the rows the host lists). The choice is remembered by
 * installation and view/tree id: a reinstall from the same source keeps it,
 * and another source's plugin with the same id never inherits it. It applies
 * only while that installation is active and offers it with its capability
 * granted. Otherwise the sidebar shows its own grouping, and the choice waits.
 */
import { useEffect, useRef, useState } from "react";
import { create } from "zustand";
import { useSidebarViewStore } from "@/components/layout/sidebar-view-store";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useAppStore } from "@/stores/app-store";
import type { AppStateSnapshot } from "@/tauri/types";
import type { PluginInstallation, PluginNativeTree, PluginSidebarView, PluginTreeNode } from "./contract";
import { MAX_PLUGIN_TREE_NODES } from "./contract";
import { isPluginActive, newPluginOperationId, pluginRunnableInstallations } from "./model";
import { pluginTreeNotice } from "./native-tree";
import {
  platformReadTree,
  pluginTreeTopic,
  usePlatformChangeRevision,
  usePlatformCatalogue,
} from "./platform-adapter";
import { pluginProjectIdOf, workspaceChats } from "./workspace-context";

interface LayoutOptionFields {
  readonly key: string;
  readonly installationId: string;
  /** The tree or view id within the plugin. */
  readonly localId: string;
  readonly pluginName: string;
  readonly title: string;
}

export type PluginSidebarLayoutOption =
  | (LayoutOptionFields & { readonly kind: "tree"; readonly tree: PluginNativeTree })
  | (LayoutOptionFields & { readonly kind: "view"; readonly view: PluginSidebarView });

/** Tree and view ids share no namespace in the manifest, so their keys must not collide. */
export const pluginSidebarTreeKey = (installationId: string, treeId: string) => `${installationId}/tree:${treeId}`;
export const pluginSidebarViewKey = (installationId: string, viewId: string) => `${installationId}/${viewId}`;

/** Trees read chats, so they need that grant as well as their own. */
const offersNativeTrees = (installation: PluginInstallation) =>
  isPluginActive(installation) &&
  installation.grants.capabilities.includes("ui.native-trees") &&
  installation.grants.capabilities.includes("threads.read");

const offersSidebarViews = (installation: PluginInstallation) =>
  isPluginActive(installation) && installation.grants.capabilities.includes("ui.sidebar-views");

/** A rule value from a newer contract drops that one view; the rest of the plugin still loads. */
function isKnownSidebarView(view: PluginSidebarView): boolean {
  return (
    view.groupBy === "project" &&
    (view.emptyProjects === undefined || view.emptyProjects === "show" || view.emptyProjects === "hide") &&
    (view.quietAfterDays === undefined ||
      (Number.isInteger(view.quietAfterDays) && view.quietAfterDays >= 1 && view.quietAfterDays <= 365))
  );
}

/** Pass runnable installations, so safe mode contributes nothing. */
export function pluginSidebarLayoutOptions(
  installations: ReadonlyArray<PluginInstallation>,
): ReadonlyArray<PluginSidebarLayoutOption> {
  return installations.flatMap((installation): PluginSidebarLayoutOption[] => {
    const { installationId } = installation;
    const pluginName = installation.manifest.name;
    const views = offersSidebarViews(installation)
      ? (installation.manifest.sidebarViews ?? []).filter(isKnownSidebarView).map(
          (view): PluginSidebarLayoutOption => ({
            kind: "view",
            key: pluginSidebarViewKey(installationId, view.id),
            installationId,
            localId: view.id,
            pluginName,
            title: view.title,
            view,
          }),
        )
      : [];
    const trees = offersNativeTrees(installation)
      ? (installation.manifest.nativeTrees ?? []).map(
          (tree): PluginSidebarLayoutOption => ({
            kind: "tree",
            key: pluginSidebarTreeKey(installationId, tree.id),
            installationId,
            localId: tree.id,
            pluginName,
            title: tree.title,
            tree,
          }),
        )
      : [];
    return [...views, ...trees];
  });
}

function findTreeInstallation(
  installations: ReadonlyArray<PluginInstallation>,
  key: string,
): { installation: PluginInstallation; tree: PluginNativeTree } | null {
  for (const installation of installations) {
    if (!offersNativeTrees(installation)) continue;
    for (const tree of installation.manifest.nativeTrees ?? []) {
      if (pluginSidebarTreeKey(installation.installationId, tree.id) === key) return { installation, tree };
    }
  }
  return null;
}

const NODE_KINDS = new Set(["group", "project", "thread"]);

/** Drops malformed nodes rather than the whole tree; the host validates too. */
export function sanitizePluginTreeNodes(nodes: unknown): ReadonlyArray<PluginTreeNode> {
  if (!Array.isArray(nodes)) return [];
  return nodes.slice(0, MAX_PLUGIN_TREE_NODES).filter((node): node is PluginTreeNode => {
    if (typeof node !== "object" || node === null) return false;
    const record = node as Record<string, unknown>;
    if (typeof record.id !== "string" || !NODE_KINDS.has(record.kind as string)) return false;
    if (record.parentId !== undefined && typeof record.parentId !== "string") return false;
    if (record.kind === "group") return typeof record.title === "string" && record.title !== "";
    if (record.kind === "project") return typeof record.projectId === "string";
    return typeof record.threadId === "string";
  });
}

interface TreeSnapshot {
  readonly key: string;
  readonly installationId: string;
  readonly generation: string;
  /** The last snapshot this generation served; kept while a refresh fails. */
  readonly nodes: ReadonlyArray<PluginTreeNode> | null;
  readonly failed: boolean;
}

const useTreeSnapshotStore = create<{ snapshot: TreeSnapshot | null }>()(() => ({ snapshot: null }));

function report(
  key: string,
  installationId: string,
  generation: string,
  update: { kind: "identity" } | { kind: "ready"; nodes: ReadonlyArray<PluginTreeNode> } | { kind: "failed" },
) {
  useTreeSnapshotStore.setState(({ snapshot }) => {
    // A new installation or generation may have other grants; its predecessor's rows go.
    const same =
      snapshot !== null &&
      snapshot.key === key &&
      snapshot.installationId === installationId &&
      snapshot.generation === generation;
    if (update.kind === "identity" && same) return { snapshot };
    return {
      snapshot: {
        key,
        installationId,
        generation,
        nodes: update.kind === "ready" ? update.nodes : same ? snapshot.nodes : null,
        failed: update.kind === "failed",
      },
    };
  });
}

/**
 * How long host changes gather before the chosen tree is read again. Not reset
 * by later changes, so a steady stream of snapshots still refreshes the tree
 * within this long, and costs at most one read per window.
 */
export const PLUGIN_TREE_HOST_REFRESH_MS = 750;
/** Thread metadata and time-based grouping can change without a workspace delta. */
export const PLUGIN_TREE_HOST_POLL_MS = 30_000;

/**
 * The host-owned facts a tree arranges that the plugin is never told about:
 * which chats each workspace holds, its project, title and nesting, and
 * whether its agent is busy (a turn that ends moves the chat's activity).
 */
function hostTreeFingerprint(appState: AppStateSnapshot | null | undefined): string {
  if (!appState) return "";
  const statuses = appState.pane_statuses ?? {};
  return appState.workspaces
    .map((ws) =>
      [
        ws.workspace_id,
        pluginProjectIdOf(ws),
        ws.parent_workspace_id ?? "",
        ws.title,
        ws.last_active_at ?? "",
        ws.pinned_at ?? "",
        workspaceChats(ws)
          .map((chat) => `${chat.threadId}:${statuses[chat.paneId] ?? ""}`)
          .join(","),
        getWorkspaceStatus(ws.surfaces, statuses) ?? "",
      ].join("\u0001"),
    )
    .join("\u0002");
}

/** Counts settled host changes while `enabled`; the mount read already covers the first state seen. */
function useHostTreeRevision(enabled: boolean): number {
  const fingerprint = useAppStore((state) => (enabled ? hostTreeFingerprint(state.appState) : null));
  const [revision, setRevision] = useState(0);
  const latest = useRef(fingerprint);
  latest.current = fingerprint;
  const seen = useRef<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (fingerprint === null) {
      seen.current = null;
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = null;
      return;
    }
    if (seen.current === null) {
      seen.current = fingerprint;
      return;
    }
    if (fingerprint === seen.current || timer.current !== null) return;
    timer.current = setTimeout(() => {
      timer.current = null;
      seen.current = latest.current;
      if (document.visibilityState !== "hidden") setRevision((value) => value + 1);
    }, PLUGIN_TREE_HOST_REFRESH_MS);
  }, [fingerprint]);
  useEffect(() => {
    if (!enabled) return;
    const refresh = () => {
      if (document.visibilityState !== "hidden") setRevision((value) => value + 1);
    };
    const poll = setInterval(refresh, PLUGIN_TREE_HOST_POLL_MS);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      clearInterval(poll);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [enabled]);
  useEffect(
    () => () => {
      if (timer.current !== null) clearTimeout(timer.current);
    },
    [],
  );
  return revision;
}

export interface PluginSidebarLayoutState {
  readonly options: ReadonlyArray<PluginSidebarLayoutOption>;
  /** The remembered choice when it can be shown now. */
  readonly selected: PluginSidebarLayoutOption | null;
  /** The chosen tree's nodes; null until served, so the built-in grouping shows meanwhile. */
  readonly nodes: ReadonlyArray<PluginTreeNode> | null;
  /** The chosen declared view's rules. */
  readonly view: PluginSidebarView | null;
  /** One line when a tree read failed. */
  readonly notice: string | null;
}

/** The offered layouts and the remembered one when it can be shown now. Reads nothing. */
export function usePluginSidebarLayoutChoice(): Pick<PluginSidebarLayoutState, "options" | "selected"> {
  const { catalogue } = usePlatformCatalogue();
  const remembered = useSidebarViewStore((state) => state.pluginLayout);
  const options = pluginSidebarLayoutOptions(pluginRunnableInstallations(catalogue));
  return { options, selected: options.find((option) => option.key === remembered) ?? null };
}

/**
 * Reads the chosen tree while the Projects view shows it: on mount, when the
 * plugin's generation changes, when the plugin signals `tree:<id>`, and shortly
 * after the host's own workspaces, chats or activity change (a plugin is not
 * told about those). A visible tree also refreshes every 30 seconds for
 * metadata-only changes and time-based grouping. Never per row. Declared views need no reads. Only the
 * Projects view calls this; anything else that only needs the choice uses
 * `usePluginSidebarLayoutChoice`, so each trigger costs one read.
 */
export function usePluginSidebarLayout(): PluginSidebarLayoutState {
  const { catalogue } = usePlatformCatalogue();
  const remembered = useSidebarViewStore((state) => state.pluginLayout);
  const installations = pluginRunnableInstallations(catalogue);
  const options = pluginSidebarLayoutOptions(installations);
  const selected = options.find((option) => option.key === remembered) ?? null;
  const found = remembered === null || selected?.kind !== "tree" ? null : findTreeInstallation(installations, remembered);
  const installationId = found?.installation.installationId ?? null;
  const generation = found?.installation.generation ?? null;
  const treeId = found?.tree.id ?? null;
  const revision = usePlatformChangeRevision(
    installationId === null || generation === null || treeId === null
      ? null
      : { installationId, generation, topic: pluginTreeTopic(treeId) },
  );
  const hostRevision = useHostTreeRevision(installationId !== null && generation !== null && treeId !== null);

  useEffect(() => {
    if (remembered === null || installationId === null || generation === null || treeId === null) return;
    report(remembered, installationId, generation, { kind: "identity" });
    let cancelled = false;
    platformReadTree({ installationId, generation, treeId, operationId: newPluginOperationId() }).then(
      (result) => {
        if (!cancelled) {
          report(remembered, installationId, generation, { kind: "ready", nodes: sanitizePluginTreeNodes(result?.nodes) });
        }
      },
      () => {
        if (!cancelled) report(remembered, installationId, generation, { kind: "failed" });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [remembered, installationId, generation, treeId, revision, hostRevision]);

  const snapshot = useTreeSnapshotStore((state) => state.snapshot);
  const current =
    snapshot !== null &&
    snapshot.key === remembered &&
    snapshot.installationId === installationId &&
    snapshot.generation === generation
      ? snapshot
      : null;
  return {
    options,
    selected,
    nodes: selected?.kind === "tree" ? (current?.nodes ?? null) : null,
    view: selected?.kind === "view" ? selected.view : null,
    notice:
      selected?.kind !== "tree" || current === null
        ? null
        : pluginTreeNotice(selected.title, { failed: current.failed, hasNodes: current.nodes !== null }),
  };
}

/** Test-only reset. */
export function resetPluginSidebarLayoutForTests(): void {
  useTreeSnapshotStore.setState({ snapshot: null });
}
