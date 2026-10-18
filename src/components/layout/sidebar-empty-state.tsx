import { useEffect, useState } from "react";
import { useContributions } from "@/contrib/react/use-contributions";
import { ContribBoundary, ContribRender } from "@/contrib/react/boundary";
import { SIDEBAR_EMPTY_STATE_AREA } from "@/sdk/areas";
import "@fontsource/instrument-serif/latin-400-italic.css";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { formatKeyCombo } from "@/components/ui/menu-chrome";
import { LAST_PLATE_KEY, PLATES, readLastPlate } from "@/lib/plates";
import { useNewAgentAction } from "./sidebar-action-row";

export { PLATES, type PlateEntry } from "@/lib/plates";

/**
 * The sidebar's "nothing running" state.
 *
 * It borrows the website's language — engraved plates, a mono "Fig." caption,
 * an italic serif line — so an empty sidebar reads as a quiet page of the same
 * book rather than a missing list. Each plate is paired with a real line from
 * the era (or older) that happens to describe the product. One pairing is
 * drawn at random every time the sidebar empties out, never the same one
 * twice in a row.
 */

/** A uniformly random index in `[0, count)` other than `last`. `rand` is a
 *  number in `[0, 1)`; injected so the no-repeat rule is testable. */
export function pickPlateIndex(last: number, count: number, rand: number): number {
  if (count <= 1) return 0;
  const hasLast = Number.isInteger(last) && last >= 0 && last < count;
  const pick = Math.floor(rand * (hasLast ? count - 1 : count));
  return hasLast && pick >= last ? pick + 1 : pick;
}

/**
 * An engraving drawn as a mask over a theme colour, so one asset prints light
 * ink on dark themes and dark ink on light ones (the site gets the same look
 * with an invert + blend, which can't follow custom app themes). The plate
 * prints in once, then drifts and catches a slow pass of light.
 */
function Plate({ src }: { src: string }) {
  const layers = `url(${src}), radial-gradient(closest-side, #000 38%, transparent)`;
  return (
    // Fluid up to 200px: the sidebar can be dragged down to 180px, where a
    // fixed-size plate would spill out of the content area and scroll it
    // sideways. overflow-hidden keeps the drift's scale inside the box.
    <div
      aria-hidden
      className="cm-plate relative aspect-square w-full max-w-[200px] shrink-0 overflow-hidden opacity-65"
    >
      <div className="cm-plate-drift absolute inset-0">
        <div
          className="absolute inset-0 overflow-hidden"
          style={{
            maskImage: layers,
            WebkitMaskImage: layers,
            maskSize: "contain, 100% 100%",
            WebkitMaskSize: "contain, 100% 100%",
            maskPosition: "center",
            WebkitMaskPosition: "center",
            maskRepeat: "no-repeat",
            WebkitMaskRepeat: "no-repeat",
            maskComposite: "intersect",
            WebkitMaskComposite: "source-in",
          }}
        >
          <div className="absolute inset-0 bg-muted-foreground" />
          <div className="cm-plate-sheen absolute -inset-y-1/2 left-0 w-[45%] bg-linear-to-r from-transparent via-foreground to-transparent" />
        </div>
      </div>
    </div>
  );
}

export function SidebarEmptyState({ filterName }: { filterName: string | null }) {
  const { getKeysForAction } = useResolvedKeybinds();
  const newAgentKeys = getKeysForAction("newAgent");
  const handleNewAgent = useNewAgentAction();

  // Drawn once per mount: the inbox mounts this each time its last active
  // workspace goes away, so every idle stretch gets its own plate. The draw
  // is pure and the write happens in an effect, so StrictMode's doubled
  // initializer can't record a plate that never rendered.
  const [index] = useState(() =>
    pickPlateIndex(readLastPlate(), PLATES.length, Math.random()),
  );
  useEffect(() => {
    try {
      localStorage.setItem(LAST_PLATE_KEY, String(index));
    } catch {
      // Storage unavailable: an occasional repeat is harmless.
    }
  }, [index]);

  const plate = PLATES[index];
  // A plugin may replace the plate illustration; the first one wins.
  const pluginPlate = useContributions(SIDEBAR_EMPTY_STATE_AREA)[0];

  return (
    <div
      data-sidebar-empty
      className="flex min-h-[360px] min-w-0 flex-1 flex-col items-center justify-center gap-4 px-3 py-8 text-center wrap-anywhere"
    >
      {pluginPlate?.render ? (
        <ContribBoundary id={pluginPlate.id} variant="block">
          <ContribRender render={pluginPlate.render} />
        </ContribBoundary>
      ) : (
        <Plate src={plate.src} />
      )}
      <span className="font-mono text-micro uppercase tracking-[0.14em] text-muted-foreground/80">
        <b className="font-medium text-status-working">{plate.fig}</b> — {plate.caption}
      </span>
      <figure className="flex flex-col items-center gap-2">
        <blockquote
          style={{ fontFamily: "'Instrument Serif', Georgia, serif" }}
          // Display size for the serif quote, above the UI type scale on
          // purpose, but in rem so it still follows the interface size.
          className="max-w-[15.5rem] text-[1.4375rem] italic leading-[1.1] text-foreground/90"
        >
          {`“${plate.quote}”`}
        </blockquote>
        <figcaption className="font-mono text-micro uppercase tracking-[0.1em] text-muted-foreground/70">
          — {plate.source}
        </figcaption>
      </figure>
      {filterName && (
        <span className="text-label text-muted-foreground">
          Nothing running in <span className="font-mono">{filterName}</span>
        </span>
      )}
      {/* Square, mono, outlined — the site's button, scaled to the sidebar.
          Hovering it wakes the plate (see `.cm-plate` in globals.css). In a
          narrow sidebar the shortcut drops to its own line and the label
          wraps, rather than the button overflowing. */}
      <button
        type="button"
        data-cta
        onClick={handleNewAgent}
        className="mt-1 inline-flex min-h-8 max-w-full flex-wrap items-center justify-center gap-x-2.5 gap-y-0.5 border border-border px-3 py-1.5 font-mono text-caption uppercase leading-tight tracking-[0.1em] text-foreground/85 transition-colors duration-150 hover:border-foreground/60 hover:bg-surface-2 hover:text-foreground"
      >
        <span className="text-balance">Put them to work</span>
        {newAgentKeys && (
          <span className="whitespace-nowrap text-muted-foreground/70">
            {formatKeyCombo(newAgentKeys)}
          </span>
        )}
      </button>
    </div>
  );
}
