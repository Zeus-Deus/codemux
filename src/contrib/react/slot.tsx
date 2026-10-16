import { ContribBoundary, ContribRender } from "./boundary";
import { useContributions } from "./use-contributions";

export interface SlotProps<P extends object> {
  /** Area whose contributions render here, in order. */
  area: string;
  /** Props passed to every contribution's render. */
  props?: P;
  /** Fallback presentation when a contribution throws. */
  variant?: "chip" | "block" | "silent";
}

/**
 * Renders every enabled contribution in `area`. With nothing registered it
 * returns null: no DOM node, no wrapper, so a plugin-free app is unchanged.
 */
export function Slot<P extends object>({ area, props, variant = "chip" }: SlotProps<P>) {
  const items = useContributions(area);
  if (items.length === 0) return null;
  return (
    <>
      {items.map((c) =>
        c.render ? (
          <ContribBoundary id={c.id} key={`${c.source ?? "core"}:${c.id}`} variant={variant}>
            <ContribRender render={c.render} props={props} />
          </ContribBoundary>
        ) : null,
      )}
    </>
  );
}
