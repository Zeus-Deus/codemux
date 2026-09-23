import {
  Component,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ErrorInfo,
  type ReactNode,
} from "react";
import { Info, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { addonTreeKey, useAddonsStore } from "@/stores/addons-store";
import { addonInvoke, mountAddon } from "@/lib/addons/bridge";
import {
  composerForWorkspace,
  subscribeAddonComposers,
} from "@/lib/addons/composer-registry";
import {
  addonMessage,
  addonEnabled,
  type AddonMount,
  type AddonNode,
} from "@/lib/addons/types";
import { AddonRenderer } from "./addon-renderer";
interface AddonViewProps {
  id: string;
  view: string;
  workspaceId: string;
  kind?: "panels" | "composerViews";
  composerId?: string;
  /** Accessible name, "<view title> — <add-on name>". */
  label?: string;
  /** Render as a labelled region. Off when the caller already provides one. */
  region?: boolean;
}
/** How long an explanation for a refused click stays in the view. */
const NOTICE_MS = 8000;
function errorCode(cause: unknown): unknown {
  return typeof cause === "object" && cause !== null && "data" in cause
    ? (cause as { data?: { code?: unknown } }).data?.code
    : undefined;
}
/** A refused click or link is transient: the view stays usable and says why. */
function refusal(cause: unknown, link: boolean): string {
  const code = errorCode(cause);
  if (code === "CONTEXT_STALE")
    return "This view changed before your action reached the add-on. Try again.";
  if (link && code === "PERMISSION_DENIED")
    return "This add-on is not allowed to open links.";
  return addonMessage(cause);
}
function Diagnostic({ children }: { children: ReactNode }) {
  return (
    <div role="alert" className="rounded-md border p-3 text-body">
      <p className="font-medium">Add-on unavailable</p>
      <p className="mt-1 text-muted-foreground">{children}</p>
    </div>
  );
}
/** The view's own surface: a labelled region, or a plain box inside a
 *  surface that already has one. */
function ViewFrame({
  label,
  region = true,
  children,
}: {
  label?: string;
  region?: boolean;
  children: ReactNode;
}) {
  const Region = region ? "section" : "div";
  return (
    <Region
      aria-label={region ? (label ?? "Add-on view") : undefined}
      data-testid="addon-view"
      className="h-full min-h-0 overflow-auto p-3"
    >
      {children}
    </Region>
  );
}
/**
 * Contains a render failure to the add-on's own surface. The rest of the
 * app keeps working, and a new `resetKey` tries again.
 */
class AddonBoundary extends Component<
  { resetKey: string; fallback: ReactNode; children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[addon-view] Add-on view failed to render", error, info);
  }
  componentDidUpdate(previous: { resetKey: string }) {
    if (this.state.failed && previous.resetKey !== this.props.resetKey)
      this.setState({ failed: false });
  }
  render() {
    return !this.state.failed ? this.props.children : this.props.fallback;
  }
}
export function AddonView(props: AddonViewProps) {
  const { id, view, workspaceId, kind = "panels", label, region } = props;
  // Only a failure outside the add-on's tree lands here, and it stays until
  // this view is opened again.
  return (
    <AddonBoundary
      resetKey={`${id}/${view}/${workspaceId}/${kind}`}
      fallback={
        <ViewFrame label={label} region={region}>
          <Diagnostic>
            CodeMux could not display this add-on view. Close it and open it
            again to retry.
          </Diagnostic>
        </ViewFrame>
      }
    >
      <AddonViewBody {...props} />
    </AddonBoundary>
  );
}
function AddonViewBody({
  id,
  view,
  workspaceId,
  kind = "panels",
  composerId,
  label,
  region = true,
}: AddonViewProps) {
  const currentComposer = useSyncExternalStore(
    subscribeAddonComposers,
    () => composerId ?? composerForWorkspace(workspaceId),
    () => null,
  );
  const ready = useAddonsStore((s) => s.ready);
  const revision = useAddonsStore((s) => s.contextRevision);
  const hostEpoch = useAddonsStore((s) => s.hostEpochs[id] ?? 0);
  const installation = useAddonsStore((s) =>
    s.installed.find((i) => i.manifest.id === id),
  );
  const canMount = !!installation && addonEnabled(installation);
  const linksAllowed =
    installation?.manifest.permissions?.includes("external.open") === true;
  const [mounted, setMounted] = useState<AddonMount | null>(null);
  // The mount a late refusal must still belong to before it is shown.
  const live = useRef<AddonMount | null>(null);
  useEffect(() => {
    live.current = mounted;
  }, [mounted]);
  // `error` is fatal for this mount (it never mounted); `notice` explains a
  // refused click and never hides the tree.
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const tree = useAddonsStore((s) =>
    mounted
      ? s.trees[addonTreeKey(mounted.generation, mounted.viewId)]
      : undefined,
  );
  const failed = useAddonsStore((s) =>
    mounted ? s.failures[mounted.generation] : undefined,
  );
  useEffect(() => {
    setMounted(null);
    setError(null);
    setNotice(null);
    if (!ready || !canMount) return;
    let closed = false;
    let target: AddonMount | null = null;
    const unmount = () => {
      if (target) {
        const current = target;
        void addonInvoke("addon_unmount", { id, ...current }).catch(() => {});
        useAddonsStore.setState((s) => ({
          trees: Object.fromEntries(
            Object.entries(s.trees).filter(
              ([key]) =>
                key !== addonTreeKey(current.generation, current.viewId),
            ),
          ),
        }));
      }
    };
    void mountAddon(id, view, kind, workspaceId, currentComposer)
      .then((result) => {
        target = result;
        if (closed) unmount();
        else setMounted(result);
      })
      .catch((cause) => {
        if (!closed) setError(addonMessage(cause));
      });
    return () => {
      closed = true;
      unmount();
    };
  }, [
    id,
    view,
    workspaceId,
    kind,
    currentComposer,
    ready,
    revision,
    canMount,
    hostEpoch,
    installation?.digest,
    installation?.dataGeneration,
  ]);
  useEffect(() => {
    if (tree)
      void addonInvoke("addon_ui_ack", {
        id,
        generation: tree.generation,
        viewId: tree.viewId,
        revision: tree.revision,
      }).catch(() => {});
  }, [tree, id]);
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(null), NOTICE_MS);
    return () => window.clearTimeout(timer);
  }, [notice]);
  function event(
    node: AddonNode,
    event: "press" | "change",
    value: string | boolean | null,
  ) {
    const callbackId = node.eventListeners[event]?.callbackId;
    if (!mounted || !callbackId) return;
    const current = mounted;
    void addonInvoke("addon_ui_event", {
      id,
      ...current,
      nodeId: node.id,
      event,
      callbackId,
      value,
    }).catch((cause) => {
      if (live.current === current) setNotice(refusal(cause, false));
    });
  }
  return (
    <ViewFrame label={label} region={region}>
      {error || failed || installation?.status === "failed-disabled" ? (
        <Diagnostic>
          {error ||
            failed ||
            installation?.failure ||
            "The add-on stopped. Retry or disable it in Settings → Add-ons."}
        </Diagnostic>
      ) : tree ? (
        <>
          {notice && (
            <div
              role="status"
              className="mb-2 flex items-start gap-2 rounded-md border bg-muted py-1 pl-2 pr-1 text-label text-muted-foreground"
            >
              <Info className="mt-1 size-3.5 shrink-0" aria-hidden />
              <span className="min-w-0 flex-1 py-0.5">{notice}</span>
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                aria-label="Dismiss add-on notice"
                onClick={() => setNotice(null)}
              >
                <X />
              </Button>
            </div>
          )}
          <AddonBoundary
            resetKey={`${tree.generation}/${tree.viewId}/${tree.revision}`}
            fallback={
              <Diagnostic>
                CodeMux could not display this add-on view. It will try again
                when the add-on updates it.
              </Diagnostic>
            }
          >
            <AddonRenderer
              nodes={tree.tree.children}
              event={event}
              linksAllowed={linksAllowed}
              link={(node, url) => {
                if (!mounted) return;
                const current = mounted;
                void addonInvoke("addon_ui_link", {
                  id,
                  ...current,
                  nodeId: node.id,
                  url,
                }).catch((cause) => {
                  if (live.current === current) setNotice(refusal(cause, true));
                });
              }}
            />
          </AddonBoundary>
        </>
      ) : (
        <p role="status" className="text-body text-muted-foreground">
          Loading add-on…
        </p>
      )}
    </ViewFrame>
  );
}
