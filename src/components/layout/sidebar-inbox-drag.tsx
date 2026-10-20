import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  type DragMoveEvent,
  type DragStartEvent,
  type KeyboardCoordinateGetter,
  type PointerSensorOptions,
} from "@dnd-kit/core";
import {
  SortableContext,
  useSortable,
  type SortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical, Pin, PinOff } from "lucide-react";
import { SidebarDragContext } from "./sidebar-drag-context";
import {
  planInboxDrop,
  type InboxDrop,
  type InboxSection,
} from "./sidebar-inbox-order";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { useReducedMotion } from "motion/react";
import { selectReduceMotion, useSettingsStore } from "@/stores/settings-store";

const PINNED = "__inbox-pinned-boundary__";
const ACTIVE = "__inbox-active-boundary__";
const LEADING = "__inbox-creating-cards__";
const DragDisabledContext = createContext(true);

class InboxPointerSensor extends PointerSensor {
  static activators = [
    {
      eventName: "onPointerDown" as const,
      handler: (event: ReactPointerEvent, options: PointerSensorOptions) => {
        const target = event.target;
        // React portal events bubble through the card even outside its DOM.
        if (!(target instanceof Node) || !event.currentTarget.contains(target))
          return false;
        if (event.shiftKey || event.metaKey || event.ctrlKey || event.altKey)
          return false;
        if (
          target instanceof Element &&
          !target.closest("[data-inbox-drag-handle]") &&
          target.closest(
            "button,a,input,textarea,select,[contenteditable=true],[role=menuitem]",
          )
        )
          return false;
        return PointerSensor.activators[0].handler(event, options);
      },
    },
  ];
}

export function SortableInboxCard({
  id,
  title,
  children,
}: {
  id: string;
  title: string;
  children: ReactNode;
}) {
  const disabled = useContext(DragDisabledContext);
  const {
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    listeners,
    attributes,
    isDragging,
  } = useSortable({
    id,
    disabled,
    transition: { duration: 200, easing: "ease" },
  });
  return (
    <div
      ref={setNodeRef}
      data-inbox-sortable={id}
      className="group/inbox-sortable relative"
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: isDragging ? 0 : undefined,
      }}
      onPointerDown={(event) => listeners?.onPointerDown?.(event)}
    >
      {children}
      <button
        type="button"
        ref={setActivatorNodeRef}
        {...attributes}
        {...listeners}
        data-inbox-drag-handle
        aria-label={`Move "${title}"`}
        className="absolute -left-2 top-1/2 z-[1] flex h-7 w-4 -translate-y-1/2 touch-none items-center justify-center rounded-sm bg-sidebar text-muted-foreground opacity-0 hover:text-foreground focus-visible:opacity-100 group-hover/inbox-sortable:opacity-100 pointer-coarse:opacity-60"
      >
        <GripVertical className="size-3" />
      </button>
    </div>
  );
}

function Boundary({
  id,
  dragging,
  target,
  resting,
  empty,
  wrapping = false,
}: {
  id: string;
  dragging: boolean;
  target: boolean;
  resting: boolean;
  empty: boolean;
  wrapping?: boolean;
}) {
  const { setNodeRef, transform, transition } = useSortable({
    id,
    disabled: { draggable: true },
  });
  return (
    <div
      ref={setNodeRef}
      data-inbox-sortable={id}
      data-pinned-divider={
        id === ACTIVE && resting && !dragging ? "" : undefined
      }
      className="pointer-events-none relative mx-1"
      style={{
        height: id === ACTIVE && resting ? 13 : 0,
        transform: CSS.Transform.toString(transform),
        transition,
      }}
    >
      {dragging ? (
        <>
          <div
            className={cn(
              "absolute inset-x-0 top-0 flex h-6 items-center gap-2 text-label",
              target ? "text-foreground" : "text-muted-foreground",
            )}
          >
            <span>{id === PINNED ? "Pinned" : "Active"}</span>
            <span
              className={cn(
                "h-px flex-1",
                target ? "bg-foreground/40" : "bg-border/60",
              )}
            />
          </div>
          {empty && (
            <div className="absolute inset-x-0 top-6 flex h-9 items-center justify-center rounded-md border border-dashed border-border text-caption text-muted-foreground">
              {id === PINNED
                ? "Drop here to pin"
                : wrapping
                  ? "Unpin into Wrapping up"
                  : "Drop here to keep active"}
            </div>
          )}
        </>
      ) : resting && id === ACTIVE ? (
        <div className="absolute inset-x-0 top-1.5 h-px bg-border/60" />
      ) : null}
    </div>
  );
}

function ActiveLeading({ children }: { children: ReactNode }) {
  const { setNodeRef, transform, transition } = useSortable({
    id: LEADING,
    disabled: { draggable: true },
  });
  return (
    <div
      ref={setNodeRef}
      data-inbox-sortable={LEADING}
      style={{ transform: CSS.Transform.toString(transform), transition }}
    >
      {children}
    </div>
  );
}

interface Props {
  pinned: readonly string[];
  active: readonly string[];
  allPinned: readonly string[];
  allActive: readonly string[];
  allIds: readonly string[];
  wraps: ReadonlySet<string>;
  hasContentBelow: boolean;
  filter: string | null;
  activeLeading: ReactNode;
  renderCard: (id: string, index: number, sortable: boolean) => ReactNode;
  onWrappingPreview: (id: string | null) => void;
  onWrappingDrop: (id: string, keyboard: boolean) => void;
}

interface Measurement {
  top: number;
  height: number;
}
interface Drag {
  id: string;
  from: InboxSection;
  to: InboxSection;
  index: number;
  measurements: Map<string, Measurement>;
  items: string[];
  start: number;
  scrollTop: number;
  lastY: number;
  point: { x: number; y: number };
  width: number;
  keyboard: boolean;
  valid: boolean;
  filter: string | null;
  wraps: boolean;
}

export function InboxDragList(props: Props) {
  const container = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const dragRef = useRef(drag);
  const latest = useRef(props);
  latest.current = props;
  const lastPointer = useRef<{ x: number; y: number } | null>(null);
  const order = useSidebarInboxStore((state) => state.order);
  const loaded = useSidebarInboxStore((state) => state.loaded);
  const saving = useSidebarInboxStore((state) => state.pendingDrop);
  const systemReducedMotion = useReducedMotion();
  const settingReducedMotion = useSettingsStore(selectReduceMotion);
  const sensors = useSensors(
    useSensor(InboxPointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: keyboardCoordinates,
      scrollBehavior: "auto",
    }),
  );
  const items = [PINNED, ...props.pinned, ACTIVE, LEADING, ...props.active];
  const itemsKey = JSON.stringify(items);
  const scrollElement = () =>
    container.current?.closest<HTMLElement>('[data-sidebar="content"]');
  function update(next: Drag | null) {
    dragRef.current = next;
    setDrag(next);
  }
  function preview(current: Drag) {
    const pinned = props.pinned.filter((id) => id !== current.id);
    const active = props.active.filter((id) => id !== current.id);
    const wrapping = current.to === "active" && props.wraps.has(current.id);
    if (!wrapping)
      (current.to === "pinned" ? pinned : active).splice(
        current.index,
        0,
        current.id,
      );
    const ids = [PINNED, ...pinned, ACTIVE, LEADING, ...active];
    const positions = new Map<string, number>();
    let height = 0;
    for (const id of ids) {
      positions.set(id, height);
      height +=
        id === PINNED
          ? 24 + (pinned.length === 0 ? 36 : 0)
          : id === ACTIVE
            ? 24 + (active.length === 0 ? 36 : 0)
            : (current.measurements.get(id)?.height ?? 88);
    }
    return { positions, height, pinned, active, wrapping };
  }
  const layout = drag ? preview(drag) : null;
  const strategy: SortingStrategy = ({ index }) => {
    if (!drag || !layout) return null;
    const id = drag.items[index];
    const measurement = drag.measurements.get(id);
    const top = layout.positions.get(id);
    if (!measurement || top === undefined) return null;
    return {
      x: 0,
      y: drag.start + top - measurement.top,
      scaleX: 1,
      scaleY: 1,
    };
  };
  function cancel() {
    // Cancel the active sensor too, including its capture-phase click guard.
    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        code: "Escape",
        key: "Escape",
        bubbles: false,
      }),
    );
  }
  useEffect(() => {
    const pointer = (event: PointerEvent) => {
      lastPointer.current = { x: event.clientX, y: event.clientY };
    };
    window.addEventListener("pointermove", pointer, true);
    window.addEventListener("pointerup", pointer, true);
    return () => {
      window.removeEventListener("pointermove", pointer, true);
      window.removeEventListener("pointerup", pointer, true);
    };
  }, []);
  useEffect(() => {
    if (!drag) return;
    window.addEventListener("blur", cancel);
    return () => {
      window.removeEventListener("blur", cancel);
    };
  }, [Boolean(drag)]);
  useEffect(() => {
    const current = dragRef.current;
    if (
      current &&
      (JSON.stringify(current.items) !== itemsKey ||
        current.filter !== props.filter ||
        current.wraps !== props.wraps.has(current.id))
    )
      cancel();
  }, [itemsKey, props.filter, props.wraps.has(drag?.id ?? "")]);
  useEffect(() => () => latest.current.onWrappingPreview(null), []);

  function start(event: DragStartEvent) {
    const id = String(event.active.id);
    if (
      !container.current ||
      !items.includes(id) ||
      id === PINNED ||
      id === ACTIVE
    )
      return;
    // Realize offscreen cards once, then freeze their measured geometry.
    container.current.classList.add("inbox-dragging");
    const measurements = new Map<string, Measurement>();
    for (const row of container.current.querySelectorAll<HTMLElement>(
      "[data-inbox-sortable]",
    )) {
      const rect = row.getBoundingClientRect();
      const key = row.dataset.inboxSortable;
      if (key) measurements.set(key, { top: rect.top, height: rect.height });
    }
    const rect = event.active.rect.current.initial;
    const pointer =
      event.activatorEvent instanceof PointerEvent
        ? event.activatorEvent
        : null;
    const point = {
      x: pointer?.clientX ?? rect?.left ?? 0,
      y: pointer?.clientY ?? (rect ? rect.top + rect.height / 2 : 0),
    };
    if (!pointer) lastPointer.current = point;
    const from = props.pinned.includes(id) ? "pinned" : "active";
    update({
      id,
      from,
      to: from,
      index: props[from].indexOf(id),
      items,
      measurements,
      start: measurements.get(PINNED)?.top ?? 0,
      scrollTop: scrollElement()?.scrollTop ?? 0,
      lastY: point.y,
      point,
      width: rect?.width ?? 260,
      keyboard: !pointer,
      valid: true,
      filter: props.filter,
      wraps: props.wraps.has(id),
    });
    // The move that exceeds the activation threshold can also be the last
    // move before release. Preview it even before the sensor emits a delta.
    if (pointer && lastPointer.current)
      move({ active: event.active, delta: { x: 0, y: 0 } });
  }

  function move(event: Pick<DragMoveEvent, "active" | "delta">) {
    const current = dragRef.current;
    const root = container.current;
    if (!current || !root) return;
    if (current.keyboard) return;
    const initial = event.active.rect.current.initial;
    const point = current.keyboard
      ? {
          x: initial?.left ?? 0,
          y: (initial ? initial.top + initial.height / 2 : 0) + event.delta.y,
        }
      : (lastPointer.current ?? current.point);
    const scrollDelta = (scrollElement()?.scrollTop ?? 0) - current.scrollTop;
    const currentLayout = preview(current);
    const boundary =
      current.start + (currentLayout.positions.get(ACTIVE) ?? 0) - scrollDelta;
    const bounds = root.getBoundingClientRect();
    const viewport = scrollElement()?.getBoundingClientRect();
    const valid =
      point.x >= bounds.left &&
      point.x <= bounds.right &&
      point.y >= Math.max(bounds.top, viewport?.top ?? bounds.top) &&
      // A wrapping card lands in the shelf below this list. Its entire
      // sidebar lane stays a valid destination, including that shelf's slot.
      point.y <=
        (props.wraps.has(current.id) && viewport
          ? viewport.bottom
          : Math.min(bounds.bottom, viewport?.bottom ?? bounds.bottom));
    let to = current.to;
    if (valid && point.y < current.lastY && point.y <= boundary + 24)
      to = "pinned";
    if (valid && point.y > current.lastY && point.y >= boundary) to = "active";
    const candidates = props[to].filter((id) => id !== current.id);
    const index = candidates.filter((id) => {
      const measured = current.measurements.get(id);
      return (
        measured && point.y > measured.top + measured.height / 2 - scrollDelta
      );
    }).length;
    const next = { ...current, to, index, point, lastY: point.y, valid };
    update(next);
    props.onWrappingPreview(
      valid && to === "active" && props.wraps.has(current.id)
        ? current.id
        : null,
    );
  }

  function keyboardCoordinates(
    event: KeyboardEvent,
    { currentCoordinates }: Parameters<KeyboardCoordinateGetter>[1],
  ) {
    const current = dragRef.current;
    if (!current || (event.code !== "ArrowUp" && event.code !== "ArrowDown"))
      return;
    let { to, index } = current;
    const count =
      to === "active" && props.wraps.has(current.id)
        ? 0
        : props[to].filter((id) => id !== current.id).length;
    if (event.code === "ArrowUp") {
      if (index > 0) index--;
      else if (to === "active") {
        to = "pinned";
        index = props.pinned.filter((id) => id !== current.id).length;
      }
    } else {
      if (index < count) index++;
      else if (to === "pinned") {
        to = "active";
        index = 0;
      }
    }
    const next = { ...current, to, index };
    update(next);
    props.onWrappingPreview(
      to === "active" && props.wraps.has(current.id) ? current.id : null,
    );
    const layout = preview(next);
    const top = layout.positions.get(current.id) ?? layout.height;
    return {
      x: currentCoordinates.x,
      y:
        current.start +
        top -
        ((scrollElement()?.scrollTop ?? 0) - current.scrollTop),
    };
  }

  function clear() {
    container.current?.classList.remove("inbox-dragging");
    props.onWrappingPreview(null);
    update(null);
  }
  function end(event: DragMoveEvent) {
    move(event);
    const current = dragRef.current;
    clear();
    if (!current?.valid) return;
    const plan: InboxDrop | null = planInboxDrop({
      id: current.id,
      from: current.from,
      to: current.to,
      index: current.index,
      visible: props[current.to],
      destinationIds:
        current.to === "pinned" ? props.allPinned : props.allActive,
      allIds: props.allIds,
      order,
      wraps: props.wraps.has(current.id),
    });
    if (!plan) return;
    if (plan.destination === "wrapping")
      props.onWrappingDrop(current.id, current.keyboard);
    void useSidebarInboxStore
      .getState()
      .applyDrop(plan, current.from === "pinned")
      .catch((error: unknown) => {
        if (current.keyboard && plan.destination === "wrapping")
          requestAnimationFrame(() =>
            container.current
              ?.querySelector<HTMLElement>(
                `[data-inbox-sortable="${window.CSS.escape(current.id)}"] [data-inbox-drag-handle]`,
              )
              ?.focus(),
          );
        toast.error("Could not move workspace", {
          description: error instanceof Error ? error.message : String(error),
        });
      });
  }
  const verb = drag
    ? !drag.valid
      ? "Release to cancel"
      : drag.to === drag.from
        ? "Move"
        : drag.to === "pinned"
          ? "Pin"
          : props.wraps.has(drag.id)
            ? "Unpin → Wrapping up"
            : "Unpin"
    : "";
  const originalHeight = drag
    ? [...drag.measurements.values()].reduce((sum, row) => sum + row.height, 0)
    : 0;
  const extra = layout ? layout.height - originalHeight : 0;
  const children = [
    <Boundary
      key={PINNED}
      id={PINNED}
      dragging={Boolean(drag)}
      target={Boolean(drag?.valid && drag.to === "pinned")}
      resting={false}
      empty={Boolean(drag?.valid && layout?.pinned.length === 0)}
    />,
    ...props.pinned.map((id, index) => props.renderCard(id, index, true)),
    <Boundary
      key={ACTIVE}
      id={ACTIVE}
      dragging={Boolean(drag)}
      target={Boolean(drag?.valid && drag.to === "active")}
      resting={props.pinned.length > 0 && props.hasContentBelow}
      empty={Boolean(drag?.valid && layout?.active.length === 0)}
      wrapping={layout?.wrapping}
    />,
    <ActiveLeading key={LEADING}>{props.activeLeading}</ActiveLeading>,
    ...props.active.map((id, index) =>
      props.renderCard(id, props.pinned.length + index, true),
    ),
  ];
  return (
    <DndContext
      sensors={sensors}
      onDragStart={start}
      onDragMove={move}
      onDragEnd={end}
      onDragCancel={clear}
      collisionDetection={({ active }) => [{ id: active.id }]}
      modifiers={[
        ({ transform, activeNodeRect }) => ({
          ...transform,
          x: 0,
          y:
            drag && activeNodeRect
              ? Math.max(
                  transform.y,
                  drag.start +
                    24 -
                    ((scrollElement()?.scrollTop ?? 0) - drag.scrollTop) -
                    activeNodeRect.top,
                )
              : transform.y,
        }),
      ]}
      accessibility={{
        announcements: {
          onDragStart: () =>
            "Workspace picked up. Move across the Pinned and Active boundary; Escape cancels.",
          onDragOver: () => undefined,
          onDragMove: () => undefined,
          onDragCancel: () => "Workspace move cancelled.",
          onDragEnd: () => "Workspace placed.",
        },
      }}
    >
      <SidebarDragContext.Provider value={Boolean(drag)}>
        <DragDisabledContext.Provider value={!loaded || saving}>
          <SortableContext items={items} strategy={strategy}>
            <div
              ref={container}
              className={cn("relative", drag && "inbox-dragging")}
              data-inbox-drag-list
              style={{
                paddingBottom: Math.max(0, extra),
                marginBottom: Math.min(0, extra),
              }}
            >
              {children}
              {drag?.valid && layout && !layout.wrapping && (
                <div
                  aria-hidden="true"
                  className="pointer-events-none absolute inset-x-0 rounded-lg border border-dashed border-foreground/25 bg-surface-1"
                  style={{
                    top: layout.positions.get(drag.id),
                    height: (drag.measurements.get(drag.id)?.height ?? 88) - 6,
                  }}
                />
              )}
            </div>
          </SortableContext>
        </DragDisabledContext.Provider>
        <span className="sr-only" role="status" aria-live="polite">
          {drag
            ? `${verb}. Destination: ${layout?.wrapping ? "Wrapping up" : drag.to === "pinned" ? "Pinned" : "Active"}.`
            : ""}
        </span>
        {createPortal(
          <DragOverlay
            zIndex={40}
            dropAnimation={{
              duration: systemReducedMotion || settingReducedMotion ? 0 : 150,
              easing: "ease-out",
            }}
          >
            {drag && (
              <div
                inert
                aria-hidden="true"
                data-inbox-drag-overlay
                className={cn(
                  "inbox-dragging relative rounded-lg bg-sidebar shadow-lg ring-1 ring-foreground/20",
                  drag.keyboard && "ring-2 ring-sidebar-ring",
                )}
                style={{ width: drag.width }}
              >
                {props.renderCard(drag.id, 0, false)}
                <span className="absolute -top-2 right-2 flex items-center gap-1 rounded-md border border-border bg-sidebar px-1.5 py-0.5 text-label text-foreground shadow-sm">
                  {drag.to === "pinned" ? (
                    <Pin className="size-3" />
                  ) : (
                    <PinOff className="size-3" />
                  )}
                  {verb}
                </span>
              </div>
            )}
          </DragOverlay>,
          document.body,
        )}
      </SidebarDragContext.Provider>
    </DndContext>
  );
}
