/**
 * Geometry fingerprint of a rendered element — a test helper.
 *
 * jsdom has no layout, so a test that wants to say "these two controls are
 * the same shape" or "this control does not resize between states" has to
 * read classes. Reading them as a *sorted set of geometry classes* instead
 * of as literal pixel values keeps the assertion about the relationship
 * (same shape / unchanged shape) rather than about the current value, so a
 * token migration that renames `h-[34px]` to a scale step does not break it.
 *
 * Modelled on `primaryGeometry()` in review-detail.test.tsx, which is the
 * idiom the rest of the suite should follow. Colour classes are excluded:
 * only size, spacing and radius count as geometry. Font size counts, since
 * it sets the box; `text-[#…]` is a colour and is filtered out.
 */
export function geometryFingerprint(element: Element): string {
  return Array.from(element.classList)
    .filter(
      (cls) =>
        (/^(h-|w-|size-|min-w-|min-h-|max-w-|max-h-|px-|py-|p-|gap-|rounded|text-\[)/.test(
          cls,
        ) &&
          !cls.startsWith("text-[#")) ||
        /^text-(micro|caption|label|body-sm|body|body-lg)$/.test(cls),
    )
    .sort()
    .join(" ");
}
