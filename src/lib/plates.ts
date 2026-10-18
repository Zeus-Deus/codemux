/**
 * The engraved plates shared by the empty sidebar and plugins (exported
 * through `@codemux/sdk`). Each plate is paired with a real line from the
 * era, or older, that happens to describe the product.
 */
import bustUrl from "@/assets/plates/briareus-bust.webp";
import titanUrl from "@/assets/plates/briareus-titan.webp";
import orreryUrl from "@/assets/plates/orrery.webp";
import baudotUrl from "@/assets/plates/baudot.webp";
import peacockUrl from "@/assets/plates/peacock.webp";

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

/** localStorage key holding the index of the plate the sidebar last drew. */
export const LAST_PLATE_KEY = "codemux.sidebar.emptyPlate";

/** Index into PLATES of the plate the empty sidebar last drew, or -1. */
export function readLastPlate(): number {
  try {
    const raw = localStorage.getItem(LAST_PLATE_KEY);
    return raw === null ? -1 : Number(raw);
  } catch {
    return -1;
  }
}
