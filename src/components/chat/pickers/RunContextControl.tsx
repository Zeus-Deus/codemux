import {
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import {
  ChevronDown,
  Folder,
  FolderGit2,
  Monitor,
  Pencil,
  Server,
} from "lucide-react";

import {
  DEVICE_ROW,
  DeviceMenuFooter,
  DeviceOptions,
  SelectedMark,
  useDeviceTarget,
} from "@/components/hosts/device-picker";
import { Command, CommandItem, CommandList } from "@/components/ui/command";
import { Eyebrow } from "@/components/ui/eyebrow";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { useHosts } from "@/stores/hosts-store";

import { focusCmdkOnOpen } from "./focus-cmdk-root";

type CheckoutMode = "current" | "worktree";

const WORKTREE_VALUE = "checkout-worktree";

const TRIGGER =
  "group/run inline-flex h-6 min-w-0 shrink-0 items-center gap-1.5 rounded-md px-2 text-label font-medium text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground focus-visible:bg-surface-2 focus-visible:text-foreground data-[state=open]:bg-surface-2 data-[state=open]:text-foreground disabled:cursor-not-allowed disabled:opacity-50";

// The two labels share one grid cell: the device at rest, the workspace on
// hover, keyboard focus, or while the menu is open.
const LABEL_REST =
  "col-start-1 row-start-1 max-w-[140px] justify-self-start truncate transition-[opacity,translate] duration-150 ease-out motion-reduce:transition-none group-hover/run:-translate-y-1 group-hover/run:opacity-0 group-focus-visible/run:-translate-y-1 group-focus-visible/run:opacity-0 group-data-[state=open]/run:-translate-y-1 group-data-[state=open]/run:opacity-0";
const LABEL_REVEAL =
  "col-start-1 row-start-1 max-w-[180px] translate-y-1 justify-self-start truncate opacity-0 transition-[opacity,translate] duration-150 ease-out motion-reduce:transition-none group-hover/run:translate-y-0 group-hover/run:opacity-100 group-focus-visible/run:translate-y-0 group-focus-visible/run:opacity-100 group-data-[state=open]/run:translate-y-0 group-data-[state=open]/run:opacity-100";
// The cell eases between the two measured widths, so the pill never holds
// the wider label's blank space at rest.
const LABEL_WIDTH =
  "w-(--run-rest-w) transition-[width] duration-150 ease-out motion-reduce:transition-none group-hover/run:w-(--run-reveal-w) group-focus-visible/run:w-(--run-reveal-w) group-data-[state=open]/run:w-(--run-reveal-w)";
// Phones have no hover, and a local thread has only one device to name, so
// the workspace label shows outright there.
const MOBILE_LOCAL_REST = "in-[[data-mobile]]:hidden";
const MOBILE_LOCAL_REVEAL = "in-[[data-mobile]]:translate-y-0 in-[[data-mobile]]:opacity-100";
const MOBILE_LOCAL_WIDTH = "in-[[data-mobile]]:w-auto!";

export interface RunContextControlProps {
  /** Device the thread runs on (`HostView.id`); `null` = this device. */
  hostId: number | null;
  onChangeHostId: (hostId: number | null) => void;
  /** Whether the project offers checkout choices (a git project). Without
   *  them the control is a plain device picker. */
  showWorkspace: boolean;
  checkoutMode: CheckoutMode;
  worktreeName: string;
  disabled?: boolean;
  onChangeCheckoutMode: (mode: CheckoutMode) => void;
  onChangeWorktreeName: (name: string) => void;
}

/**
 * "Where does this thread run?" — device and workspace in one control, the
 * first item in the new-thread scope strip. At rest it names the device;
 * hovering swaps the label in place for the workspace ("New worktree", the
 * worktree's name once typed, or "Current checkout"). The menu lists the
 * devices (once any are configured) above the workspace choices, and the
 * "New worktree" row renames in place behind a pencil that only appears on
 * the hovered or highlighted row.
 */
export function RunContextControl({
  hostId,
  onChangeHostId,
  showWorkspace,
  checkoutMode,
  worktreeName,
  disabled,
  onChangeCheckoutMode,
  onChangeWorktreeName,
}: RunContextControlProps) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draftName, setDraftName] = useState("");
  // Mirrors `editing` synchronously: Enter, Esc, blur, and closing the menu
  // can all end one edit, and only the first of them may commit.
  const editingRef = useRef(false);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const hosts = useHosts();
  const device = useDeviceTarget(hostId);

  const isWorktree = checkoutMode === "worktree";
  const typedName = worktreeName.trim();
  const workspaceLabel = isWorktree
    ? typedName || "New worktree"
    : "Current checkout";
  // A selected device keeps the list too: if the host list failed to load,
  // "this device" is the only way back from a target that reads unavailable.
  const showDevices = hosts.length > 0 || !showWorkspace || hostId !== null;
  const mobileLocal = hostId === null;

  // Measure both labels so the pill eases between their widths. Left unset
  // (an unmeasured grid cell sizes to the wider label) until layout reports
  // a width, which jsdom never does.
  const restRef = useRef<HTMLSpanElement | null>(null);
  const revealRef = useRef<HTMLSpanElement | null>(null);
  const [widths, setWidths] = useState<{ rest: number; reveal: number } | null>(
    null,
  );
  useLayoutEffect(() => {
    if (!showWorkspace) return;
    const restEl = restRef.current;
    const revealEl = revealRef.current;
    // Rounded up: fractional text widths rounded down clip the last glyph.
    const measure = (el: HTMLElement | null) =>
      Math.ceil(el?.getBoundingClientRect().width ?? 0);
    const update = () => {
      const rest = measure(restEl);
      const reveal = measure(revealEl);
      setWidths((prev) =>
        rest > 0 && reveal > 0
          ? prev?.rest === rest && prev.reveal === reveal
            ? prev
            : { rest, reveal }
          : null,
      );
    };
    update();
    // Labels re-measure when their text changes or the web font lands.
    if (typeof ResizeObserver === "undefined" || !restEl || !revealEl) return;
    const observer = new ResizeObserver(update);
    observer.observe(restEl);
    observer.observe(revealEl);
    return () => observer.disconnect();
  }, [showWorkspace]);

  const focusMenu = () => {
    contentRef.current?.querySelector<HTMLElement>("[cmdk-root]")?.focus();
  };

  const startEditing = () => {
    if (!isWorktree) onChangeCheckoutMode("worktree");
    setDraftName(worktreeName);
    editingRef.current = true;
    setEditing(true);
  };

  const commitName = () => {
    if (!editingRef.current) return;
    editingRef.current = false;
    setEditing(false);
    onChangeWorktreeName(draftName.trim());
  };

  const cancelEditing = () => {
    editingRef.current = false;
    setEditing(false);
    // The input unmounts with focus inside it; hand focus back to cmdk so
    // the arrow keys keep working.
    requestAnimationFrame(focusMenu);
  };

  const handleOpenChange = (next: boolean) => {
    if (!next && editing) commitName();
    setOpen(next);
  };

  const pickCheckout = (mode: CheckoutMode) => {
    setOpen(false);
    onChangeCheckoutMode(mode);
  };

  const pickDevice = (next: number | null) => {
    setOpen(false);
    onChangeHostId(next);
  };

  // F2 renames the highlighted "New worktree" row, as in a file list.
  const handleMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "F2" || editing) return;
    const highlighted = event.currentTarget.querySelector(
      '[cmdk-item][data-selected="true"]',
    );
    if (highlighted?.getAttribute("data-value") !== WORKTREE_VALUE) return;
    event.preventDefault();
    startEditing();
  };

  const handleNameKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    // Keep cmdk's arrow/Enter handling on the root out of the text field.
    event.stopPropagation();
    // Enter that confirms an IME candidate belongs to the composition.
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === "Enter") {
      event.preventDefault();
      commitName();
      requestAnimationFrame(focusMenu);
    }
  };

  const DeviceIcon = device.selected || device.unavailable ? Server : Monitor;
  const WorkspaceIcon = isWorktree ? FolderGit2 : Folder;
  const accessibleName = showWorkspace
    ? `Run on: ${device.label} · ${workspaceLabel}`
    : `Run on: ${device.label}`;

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          aria-label={accessibleName}
          className={TRIGGER}
        >
          <span className="inline-flex shrink-0 items-center gap-0.5">
            <DeviceIcon
              className={cn(
                "size-3",
                device.selected ? "text-status-remote" : "text-muted-foreground",
              )}
            />
            {showWorkspace && (
              <WorkspaceIcon className="size-3 text-muted-foreground" />
            )}
          </span>
          {showWorkspace ? (
            <span
              className={cn(
                "inline-grid min-w-0 overflow-hidden",
                widths && LABEL_WIDTH,
                mobileLocal && MOBILE_LOCAL_WIDTH,
              )}
              style={
                widths
                  ? ({
                      "--run-rest-w": `${widths.rest}px`,
                      "--run-reveal-w": `${widths.reveal}px`,
                    } as CSSProperties)
                  : undefined
              }
            >
              <span
                ref={restRef}
                aria-hidden
                className={cn(LABEL_REST, mobileLocal && MOBILE_LOCAL_REST)}
              >
                {device.label}
              </span>
              <span
                ref={revealRef}
                aria-hidden
                className={cn(
                  LABEL_REVEAL,
                  isWorktree && typedName && "font-mono",
                  mobileLocal && MOBILE_LOCAL_REVEAL,
                )}
              >
                {workspaceLabel}
              </span>
            </span>
          ) : (
            <span className="max-w-[140px] truncate">{device.label}</span>
          )}
          <ChevronDown className="size-3 shrink-0 opacity-45" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        ref={contentRef}
        className="flex max-h-[var(--radix-popover-content-available-height)] w-[260px] flex-col p-1.5"
        align="start"
        side="top"
        collisionPadding={10}
        onOpenAutoFocus={focusCmdkOnOpen}
        onEscapeKeyDown={(event) => {
          // Esc while renaming reverts the name, leaving the menu open.
          if (!editing) return;
          event.preventDefault();
          cancelEditing();
        }}
      >
        <Command loop className="bg-transparent" onKeyDown={handleMenuKeyDown}>
          <CommandList
            className="max-h-[320px] min-h-0 flex-1 overflow-y-auto thin-scrollbar"
            onWheel={(e) => e.stopPropagation()}
          >
            {showDevices && (
              <>
                <Eyebrow className="block px-2 pb-1 pt-1">Run on</Eyebrow>
                <DeviceOptions hostId={hostId} onSelectHostId={pickDevice} />
              </>
            )}
            {showDevices && showWorkspace && (
              <div aria-hidden className="mx-2 my-1 h-px bg-border/60" />
            )}
            {showWorkspace && (
              <>
                <Eyebrow className="block px-2 pb-1 pt-1">Workspace</Eyebrow>
                <CommandItem
                  value="checkout-current"
                  onSelect={() => pickCheckout("current")}
                  showCheckmark={false}
                  className={DEVICE_ROW}
                >
                  <Folder className="size-3.5 text-muted-foreground" />
                  <span className="min-w-0 truncate">Current checkout</span>
                  {!isWorktree && (
                    <span className="ml-auto flex">
                      <SelectedMark />
                    </span>
                  )}
                </CommandItem>
                <CommandItem
                  value={WORKTREE_VALUE}
                  onSelect={() => {
                    if (!editing) pickCheckout("worktree");
                  }}
                  showCheckmark={false}
                  className={DEVICE_ROW}
                >
                  <FolderGit2 className="size-3.5 text-muted-foreground" />
                  <span className="shrink-0">New worktree</span>
                  {editing ? (
                    <input
                      autoFocus
                      value={draftName}
                      onChange={(e) => setDraftName(e.target.value)}
                      onKeyDown={handleNameKeyDown}
                      onBlur={commitName}
                      onClick={(e) => e.stopPropagation()}
                      onPointerDown={(e) => e.stopPropagation()}
                      placeholder="auto-name if empty"
                      aria-label="Worktree name"
                      spellCheck={false}
                      className="ml-auto h-5 min-w-0 flex-1 rounded-sm border border-accent-ember/50 bg-background px-1.5 font-mono text-caption text-foreground outline-none placeholder:text-muted-foreground/50"
                    />
                  ) : (
                    <span className="ml-auto flex min-w-0 items-center gap-1">
                      {typedName && (
                        <span className="min-w-0 truncate font-mono text-caption text-muted-foreground/70">
                          {typedName}
                        </span>
                      )}
                      <button
                        type="button"
                        aria-label="Rename worktree"
                        title="Rename (F2)"
                        onClick={(e) => {
                          e.stopPropagation();
                          startEditing();
                        }}
                        onPointerDown={(e) => e.stopPropagation()}
                        // cmdk's root turns Enter into a select of the
                        // highlighted row; keep the button's own activation.
                        onKeyDown={(e) => e.stopPropagation()}
                        className="grid size-5 shrink-0 place-items-center rounded-sm text-muted-foreground opacity-0 transition-opacity duration-150 group-hover/command-item:opacity-70 group-data-[selected=true]/command-item:opacity-70 hover:bg-surface-2 hover:text-foreground hover:opacity-100! focus-visible:opacity-100 in-[[data-mobile]]:opacity-70"
                      >
                        <Pencil className="size-3" />
                      </button>
                      {isWorktree && <SelectedMark />}
                    </span>
                  )}
                </CommandItem>
              </>
            )}
          </CommandList>
        </Command>
        <div aria-hidden className="mx-2 my-1 h-px bg-border/60" />
        <DeviceMenuFooter onDone={() => setOpen(false)} />
      </PopoverContent>
    </Popover>
  );
}
