import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Puzzle, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Empty, EmptyActions, EmptyDescription, EmptyTitle } from "@/components/ui/empty";
import { Spinner } from "@/components/ui/spinner";
import { useHtmlRenderTheme } from "@/components/chat/HtmlRenderFrame";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { cn } from "@/lib/utils";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import type { PluginCardReference, PluginInstallation, PluginView } from "./contract";
import { newPluginOperationId, pluginViewHandler } from "./model";
import { applyPluginIntents, capturePluginTarget, type PluginInvocationTarget } from "./plugin-actions";
import {
  platformInvoke,
  platformReadCardView,
  platformReadView,
  pluginViewTopic,
  usePlatformChangeRevision,
  usePlatformPluginsStore,
} from "./platform-adapter";
import { notifyPluginThreadActivity } from "./thread-status";
import {
  bindPluginViewChannel,
  buildPluginViewDocument,
  makePluginViewHost,
  newPluginViewToken,
  pluginViewBridgeFeatures,
  pluginViewFailure,
  type PluginViewBridgeFeature,
  type PluginViewContext,
  type PluginViewHost,
} from "./view-bridge";
import { pluginProjectIdOf, workspaceById } from "./workspace-context";

type LoadState =
  | { readonly kind: "loading" }
  | {
      readonly kind: "ready";
      /** What the document was read for; a document never serves another identity. */
      readonly identity: string;
      readonly document: string;
      readonly token: string;
      /** Baked into the document; the host must offer the same ones. */
      readonly features: ReadonlyArray<PluginViewBridgeFeature>;
    }
  | { readonly kind: "failed"; readonly identity: string; readonly message: string };

const LOADING: LoadState = { kind: "loading" };

/** A location no current location equals: answers to a view nobody is looking at are only offered. */
const BACKGROUND_LOCATION = "\0background";

const viewPlatform = (): PluginViewContext["platform"] => {
  if (!isRemoteClient()) return "desktop";
  return typeof window.matchMedia === "function" && window.matchMedia("(max-width: 767px)").matches
    ? "mobile"
    : "web";
};

export function PluginViewNotice({
  title,
  description,
  action,
}: {
  title: string;
  description: string;
  action?: ReactNode;
}) {
  return (
    <Empty className="size-full justify-center" data-plugin-view-notice>
      <Puzzle aria-hidden className="mb-1 size-5 text-muted-foreground" />
      <EmptyTitle>{title}</EmptyTitle>
      <EmptyDescription>{description}</EmptyDescription>
      {action ? <EmptyActions>{action}</EmptyActions> : null}
    </Empty>
  );
}

/**
 * One plugin view document in a sandboxed frame (`allow-scripts` only, opaque
 * origin, no network by CSP). Keyed by installation generation by its
 * callers, so a refreshed or re-enabled plugin reloads instead of letting an
 * old document call into new code. A frame that navigates away from its
 * document is stopped.
 */
export function PluginViewFrame(props: {
  readonly installation: PluginInstallation;
  readonly view: PluginView;
  readonly placement: PluginViewContext["placement"];
  readonly workspaceId: string | null;
  readonly threadId: string | null;
  /** A conversation card: served only after the host checks where the card came from. */
  readonly card?: PluginCardReference;
  /** A card's frame sizes to content; failures render this instead of a page notice. */
  readonly renderFailure?: (message: string, retry: () => void) => ReactNode;
}) {
  const { installation, view } = props;
  const theme = useHtmlRenderTheme();
  const changesSupported = usePlatformPluginsStore((state) => state.catalogue?.changesSupported === true);
  const features = pluginViewBridgeFeatures({ placement: props.placement, changes: changesSupported });
  const frameRef = useRef<HTMLIFrameElement>(null);
  const hostRef = useRef<PluginViewHost | null>(null);
  const loads = useRef(0);
  const [load, setLoad] = useState<LoadState>({ kind: "loading" });
  /** The identity whose document tried to navigate away; a new one starts clean. */
  const [stoppedIdentity, setStoppedIdentity] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [cardHeight, setCardHeight] = useState<number | null>(null);

  // Read by the bridge on each message, so it sees current values without being rebuilt.
  const live = { props, theme, features };
  const latest = useRef(live);
  useEffect(() => {
    latest.current = live;
  });

  const { installationId, generation } = installation;
  const cardKey = props.card === undefined ? null : JSON.stringify(props.card);
  const identity = JSON.stringify([installationId, generation, view.id, cardKey, attempt]);
  const stopped = stoppedIdentity === identity;
  useEffect(() => {
    let cancelled = false;
    const card = latest.current.props.card;
    const read =
      card === undefined
        ? platformReadView({ installationId, generation, viewId: view.id })
        : platformReadCardView({ installationId, generation, card });
    read.then(
      (result) => {
        if (cancelled) return;
        const token = newPluginViewToken();
        const { features } = latest.current;
        setLoad({
          kind: "ready",
          identity,
          document: buildPluginViewDocument(result.html, token, features),
          token,
          features,
        });
      },
      (error: unknown) => {
        if (!cancelled) setLoad({ kind: "failed", identity, message: pluginViewFailure(error).message });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [installationId, generation, view.id, cardKey, attempt, identity]);

  // A document read for an earlier generation, view or card is never shown
  // (or bound) for the current one, even for the render before the new read.
  const current = load.kind !== "loading" && load.identity === identity ? load : LOADING;
  const document = current.kind === "ready" ? current.document : null;
  const token = current.kind === "ready" ? current.token : null;
  const documentFeatures = current.kind === "ready" ? current.features : null;
  // A layout effect, so the host listens before the frame's first script can post.
  useLayoutEffect(() => {
    if (document === null || token === null || documentFeatures === null) return;
    loads.current = 0;
    const context = (): PluginViewContext => {
      const current = latest.current;
      const workspace = workspaceById(current.props.workspaceId);
      return {
        theme: current.theme.scheme,
        styles: current.theme.variables,
        platform: viewPlatform(),
        placement: current.props.placement,
        view: { installationId, viewId: view.id },
        thread:
          current.props.threadId === null
            ? null
            : { threadId: current.props.threadId, projectId: workspace ? pluginProjectIdOf(workspace) : null },
        features: documentFeatures,
        ...(current.props.card === undefined
          ? {}
          : {
              card: {
                viewId: current.props.card.viewId,
                summary: current.props.card.summary,
                ...(current.props.card.data === undefined ? {} : { data: current.props.card.data }),
              },
            }),
      };
    };
    const channel = bindPluginViewChannel({
      token,
      source: () => frameRef.current?.contentWindow ?? null,
      receive: (data) => host.receive(data),
    });
    const host = makePluginViewHost<PluginInvocationTarget>({
      token,
      post: channel.post,
      context,
      // Fixed when the view asks: a slow answer must not act on wherever the user went since.
      captureOrigin: () => {
        const current = latest.current.props;
        const target = capturePluginTarget({
          installation: current.installation,
          workspaceId: current.workspaceId,
          threadId: current.threadId,
          // Only a view beside a chat has a composer to write into.
          composer: current.placement === "side-panel",
        });
        // A view in a workspace the user is not looking at (or a hidden
        // window) may still run; what it asks for is offered, never applied.
        const shown =
          window.document.visibilityState !== "hidden" &&
          (current.workspaceId === null || current.workspaceId === selectActiveWorkspaceId(useAppStore.getState()));
        return shown ? target : { ...target, locationKey: BACKGROUND_LOCATION };
      },
      invoke: async (name, input, origin) => {
        try {
          return await platformInvoke({
            installationId,
            generation,
            handler: pluginViewHandler(view.id, name),
            ...(input === undefined ? {} : { input }),
            ...(origin.threadId === null ? {} : { context: { threadId: origin.threadId } }),
            operationId: newPluginOperationId(),
          });
        } finally {
          if (origin.threadId !== null) notifyPluginThreadActivity({ threadId: origin.threadId, installationId });
        }
      },
      onIntents: (intents, origin, unsupported) => applyPluginIntents(origin, intents, unsupported),
      features: documentFeatures,
      onResize: setCardHeight,
    });
    hostRef.current = host;
    return () => {
      channel.dispose();
      host.dispose();
      hostRef.current = null;
    };
  }, [document, token, documentFeatures, installationId, generation, view.id, attempt]);

  // The theme or the chat beside a side panel changed: the view hears it, without reloading.
  useEffect(() => {
    hostRef.current?.sendContext();
  }, [theme, props.threadId]);

  // Only a mounted document listens; a hidden or closed view holds no subscription.
  const topic = pluginViewTopic(view.id);
  const revision = usePlatformChangeRevision(features.includes("changes") ? { installationId, generation, topic } : null);
  const seenRevision = useRef(revision);
  useEffect(() => {
    const seen = seenRevision.current;
    seenRevision.current = revision;
    if (revision !== undefined && seen !== revision) hostRef.current?.changed(topic);
  }, [revision, topic]);

  const retry = () => {
    setStoppedIdentity(null);
    setLoad({ kind: "loading" });
    setAttempt((value) => value + 1);
  };
  if (current.kind === "failed" && props.renderFailure) return props.renderFailure(current.message, retry);
  if (stopped && props.renderFailure) return props.renderFailure("The card tried to leave its page.", retry);
  if (current.kind === "failed") {
    return (
      <PluginViewNotice
        title={`Couldn't load ${view.title}`}
        description={current.message}
        action={
          <Button size="sm" variant="outline" onClick={retry}>
            <RotateCcw />
            Try again
          </Button>
        }
      />
    );
  }
  if (stopped) {
    return (
      <PluginViewNotice
        title={`${view.title} was stopped`}
        description="The view tried to leave its page."
        action={
          <Button size="sm" variant="outline" onClick={retry}>
            <RotateCcw />
            Reload view
          </Button>
        }
      />
    );
  }
  if (document === null) {
    return props.placement === "conversation" ? null : (
      <div className="flex size-full items-center justify-center">
        <Spinner />
      </div>
    );
  }
  return (
    <iframe
      key={`${generation}:${attempt}`}
      ref={frameRef}
      srcDoc={document}
      title={`${installation.manifest.name}: ${view.title}`}
      // Scripts only: no same-origin (no app storage or IPC), popups, forms or top navigation.
      sandbox="allow-scripts"
      referrerPolicy="no-referrer"
      data-plugin-view={`${installationId}:${view.id}`}
      onLoad={() => {
        loads.current += 1;
        if (loads.current > 1) {
          hostRef.current?.dispose();
          setStoppedIdentity(identity);
          return;
        }
        hostRef.current?.sendContext();
      }}
      className={cn(
        "block w-full border-0 bg-transparent",
        props.placement === "conversation" ? "h-24 rounded-lg border border-border" : "h-full",
      )}
      style={{
        colorScheme: theme.scheme,
        ...(props.placement === "conversation" && cardHeight !== null ? { height: cardHeight } : {}),
      }}
    />
  );
}
