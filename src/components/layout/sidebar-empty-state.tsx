import { useEffect, useState } from "react";
import "@fontsource/instrument-serif/latin-400-italic.css";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { formatKeyCombo } from "@/components/ui/menu-chrome";
import bustUrl from "@/assets/plates/briareus-bust.webp";
import titanUrl from "@/assets/plates/briareus-titan.webp";
import orreryUrl from "@/assets/plates/orrery.webp";
import baudotUrl from "@/assets/plates/baudot.webp";
import peacockUrl from "@/assets/plates/peacock.webp";
import { useNewAgentAction } from "./sidebar-action-row";

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

export interface PlateEntry {
  src: string;
  fig: string;
  caption: string;
  quote: string;
  source: string;
}

// Attributions are deliberately careful: "After" marks a paraphrased
// translation, "per" marks a saying that survives only through a later writer.
export const PLATES: PlateEntry[] = [
  {
    src: titanUrl,
    fig: "Fig. 01",
    caption: "The hundred-handed",
    quote: "Many hands make light work.",
    source: "Heywood’s Proverbs, 1546",
  },
  {
    src: bustUrl,
    fig: "Fig. 02",
    caption: "Briareus, at rest",
    quote: "Rest is not idleness.",
    source: "John Lubbock, 1894",
  },
  {
    src: orreryUrl,
    fig: "Fig. 03",
    caption: "The orrery, stilled",
    quote: "Order is Heaven’s first law.",
    source: "Alexander Pope, 1734",
  },
  {
    src: baudotUrl,
    fig: "Fig. 04",
    caption: "The telegraph exchange",
    quote: "What hath God wrought?",
    source: "Samuel Morse, 1844",
  },
  {
    src: peacockUrl,
    fig: "Fig. 05",
    caption: "The eyes of Argus",
    quote: "Two at a time would sleep; the rest kept watch.",
    source: "After Ovid, Metamorphoses I",
  },
  {
    src: orreryUrl,
    fig: "Fig. 03",
    caption: "The orrery, stilled",
    quote: "Give me a place to stand, and I will move the earth.",
    source: "Archimedes, per Pappus",
  },
  {
    src: bustUrl,
    fig: "Fig. 02",
    caption: "Briareus, at rest",
    quote: "Make haste slowly.",
    source: "Augustus, per Suetonius",
  },
  {
    src: titanUrl,
    fig: "Fig. 01",
    caption: "The hundred-handed",
    quote: "Well begun is half done.",
    source: "After Horace, Epistles I.2",
  },
  {
    src: peacockUrl,
    fig: "Fig. 05",
    caption: "The eyes of Argus",
    quote: "They also serve who only stand and wait.",
    source: "John Milton, 1673",
  },
  {
    src: baudotUrl,
    fig: "Fig. 04",
    caption: "The telegraph exchange",
    quote: "Little strokes fell great oaks.",
    source: "Poor Richard’s Almanack, 1750",
  },
];

const LAST_PLATE_KEY = "codemux.sidebar.emptyPlate";

/** A uniformly random index in `[0, count)` other than `last`. `rand` is a
 *  number in `[0, 1)`; injected so the no-repeat rule is testable. */
export function pickPlateIndex(last: number, count: number, rand: number): number {
  if (count <= 1) return 0;
  const hasLast = Number.isInteger(last) && last >= 0 && last < count;
  const pick = Math.floor(rand * (hasLast ? count - 1 : count));
  return hasLast && pick >= last ? pick + 1 : pick;
}

function readLastPlate(): number {
  try {
    const raw = localStorage.getItem(LAST_PLATE_KEY);
    return raw === null ? -1 : Number(raw);
  } catch {
    return -1;
  }
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
    <div aria-hidden className="cm-plate relative size-[200px] opacity-65">
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

  return (
    <div
      data-sidebar-empty
      className="flex min-h-[360px] flex-1 flex-col items-center justify-center gap-4 px-3 py-8 text-center"
    >
      <Plate src={plate.src} />
      <span className="font-mono text-micro uppercase tracking-[0.14em] text-muted-foreground/80">
        <b className="font-medium text-status-working">{plate.fig}</b> — {plate.caption}
      </span>
      <figure className="flex flex-col items-center gap-2">
        <blockquote
          style={{ fontFamily: "'Instrument Serif', Georgia, serif" }}
          className="max-w-[15.5rem] text-[23px] italic leading-[1.1] text-foreground/90"
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
          Hovering it wakes the plate (see `.cm-plate` in globals.css). */}
      <button
        type="button"
        data-cta
        onClick={handleNewAgent}
        className="mt-1 inline-flex h-8 items-center gap-2.5 border border-border px-3 font-mono text-caption uppercase tracking-[0.1em] text-foreground/85 transition-colors duration-150 hover:border-foreground/60 hover:bg-foreground/5 hover:text-foreground"
      >
        Put them to work
        {newAgentKeys && (
          <span className="text-muted-foreground/70">{formatKeyCombo(newAgentKeys)}</span>
        )}
      </button>
    </div>
  );
}
