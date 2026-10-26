import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { Puzzle, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { PluginCardReference } from "./contract";
import { resolvePluginCardPresentation } from "./cards";
import { usePlatformCatalogue } from "./platform-adapter";
import { PluginBoundary } from "./PluginBoundary";
import { PluginViewFrame } from "./PluginViewFrame";

/**
 * The text the tool result recorded, readable everywhere. It stays plain: the
 * card's history must not depend on the plugin still being installed, trusted
 * or running.
 */
export function PluginCardSummary({
  card,
  pluginName,
  reason,
  onRetry,
}: {
  card: PluginCardReference;
  pluginName: string | null;
  reason: string | null;
  onRetry?: () => void;
}) {
  return (
    <div
      className="flex flex-col gap-1 rounded-lg border border-border px-3 py-2 text-label"
      data-plugin-card-summary={card.installationId}
    >
      <span className="flex items-center gap-1.5 text-muted-foreground">
        <Puzzle aria-hidden className="size-3.5 shrink-0" />
        <span className="truncate">{pluginName ?? "Plugin card"}</span>
      </span>
      <p className="whitespace-pre-wrap break-words text-body text-foreground">{card.summary}</p>
      {reason === null ? null : (
        <div className="flex items-center justify-between gap-2 text-muted-foreground">
          <span>{reason}</span>
          {onRetry ? (
            <Button size="xs" variant="ghost" onClick={onRetry}>
              <RotateCcw />
              Try again
            </Button>
          ) : null}
        </div>
      )}
    </div>
  );
}

/** How far outside the viewport a card's frame is kept mounted. */
const CARD_FRAME_MARGIN = "400px";
/** The frame's height before its document reports one. */
const CARD_FRAME_INITIAL_HEIGHT = 96;

/**
 * Mounts the card's frame only while the row is near the viewport, so a long
 * transcript does not hold a document and bridge per card. A card scrolled
 * away keeps its space, so the transcript does not jump.
 */
function NearViewport({ children }: { children: ReactNode }) {
  const boxRef = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(typeof IntersectionObserver === "undefined");
  const [reservedHeight, setReservedHeight] = useState(CARD_FRAME_INITIAL_HEIGHT);
  useEffect(() => {
    const box = boxRef.current;
    if (box === null || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[entries.length - 1];
        if (entry === undefined) return;
        if (!entry.isIntersecting) {
          const height = box.getBoundingClientRect().height;
          if (height > 0) setReservedHeight(height);
        }
        setNear(entry.isIntersecting);
      },
      { rootMargin: CARD_FRAME_MARGIN },
    );
    observer.observe(box);
    return () => observer.disconnect();
  }, []);
  return (
    <div ref={boxRef} data-plugin-card-frame={near ? "live" : "parked"}>
      {near ? children : <div aria-hidden className="rounded-lg border border-border" style={{ height: reservedHeight }} />}
    </div>
  );
}

/**
 * The chat a transcript shows, for cards rendered deep inside it. Provided by
 * `ChatTranscript`, which relays it into the transcript cache's portal.
 */
export const PluginTranscriptThreadContext = createContext<string | null>(null);

interface PluginConversationCardProps {
  card: PluginCardReference;
  workspaceId: string | null;
  /** Defaults to the surrounding transcript's chat. */
  threadId?: string | null;
}

/**
 * A card an agent's plugin call left in the conversation. Whatever the
 * plugin or catalogue holds, the transcript keeps at least the recorded
 * summary.
 */
export function PluginConversationCard(props: PluginConversationCardProps) {
  return (
    <PluginBoundary
      label="Plugin card"
      resetKey={`${props.card.installationId}:${props.card.viewId}:${props.card.summary}`}
      fallback={<PluginCardSummary card={props.card} pluginName={null} reason="This card couldn't be shown." />}
    >
      <LivePluginConversationCard {...props} />
    </PluginBoundary>
  );
}

function LivePluginConversationCard({ card, workspaceId, threadId: threadIdProp }: PluginConversationCardProps) {
  const transcriptThreadId = useContext(PluginTranscriptThreadContext);
  const threadId = threadIdProp === undefined ? transcriptThreadId : threadIdProp;
  const { catalogue, loaded } = usePlatformCatalogue();
  const presentation = resolvePluginCardPresentation({
    card,
    // A host without the platform never answers; the summary is the card then.
    supported: !loaded ? null : catalogue === null ? false : catalogue.conversationCardsSupported !== false,
    installations: loaded && catalogue !== null ? catalogue.installations : null,
    executionDisabled: catalogue?.executionDisabled === true,
  });
  if (presentation.kind === "summary") {
    return <PluginCardSummary card={card} pluginName={presentation.pluginName} reason={presentation.reason} />;
  }
  return (
    <div className="grid gap-2" data-plugin-card={card.installationId}>
      <PluginCardSummary card={card} pluginName={presentation.installation.manifest.name} reason={null} />
      <NearViewport>
        <PluginViewFrame
          // A new generation is new code: it must not inherit the old document.
          key={presentation.installation.generation}
          installation={presentation.installation}
          view={presentation.view}
          placement="conversation"
          workspaceId={workspaceId}
          threadId={threadId}
          card={card}
          renderFailure={(message, retry) => (
            <div className="flex items-center justify-between gap-2 rounded-lg border border-border px-3 py-2 text-label text-muted-foreground">
              <span>{message}</span>
              <Button size="xs" variant="ghost" onClick={retry}>
                <RotateCcw />
                Try again
              </Button>
            </div>
          )}
        />
      </NearViewport>
    </div>
  );
}
