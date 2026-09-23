import {
  Fragment,
  useCallback,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import Markdown from "react-markdown";
import {
  BookOpen,
  Check,
  CircleAlert,
  Code,
  FileText,
  Folder,
  GitBranch,
  Github,
  Info,
  Link,
  List,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Terminal,
  type LucideIcon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import type { AddonNode } from "@/lib/addons/types";
import { cn } from "@/lib/utils";
/** One glyph for every icon name the manifest and UI validators accept. */
const icons: Record<string, LucideIcon> = {
  "file-text": FileText,
  "git-branch": GitBranch,
  github: Github,
  list: List,
  check: Check,
  info: Info,
  settings: Settings,
  "book-open": BookOpen,
  link: Link,
  "refresh-cw": RefreshCw,
  plus: Plus,
  "circle-alert": CircleAlert,
  folder: Folder,
  terminal: Terminal,
  code: Code,
  search: Search,
};
export const ADDON_ICON_NAMES: readonly string[] = Object.keys(icons);
export const addonIcon = (name: string) => icons[name] ?? Info;
const spacing: Record<string, string> = {
  none: "gap-0",
  xs: "gap-1",
  sm: "gap-2",
  md: "gap-3",
  lg: "gap-4",
};
const colors: Record<string, string> = {
  default: "text-foreground",
  muted: "text-muted-foreground",
  success: "text-success",
  warning: "text-warning",
  danger: "text-destructive",
  accent: "text-primary",
};
const sizes: Record<string, string> = {
  xs: "text-label",
  sm: "text-body",
  md: "text-body-lg",
  lg: "text-lg",
};
const alignment: Record<string, string> = {
  start: "items-start",
  center: "items-center",
  end: "items-end",
  stretch: "items-stretch",
};
const columns = [
  "",
  "grid-cols-1",
  "grid-cols-2",
  "grid-cols-3",
  "grid-cols-4",
  "grid-cols-5",
  "grid-cols-6",
  "grid-cols-7",
  "grid-cols-8",
  "grid-cols-9",
  "grid-cols-10",
  "grid-cols-11",
  "grid-cols-12",
];
interface Props {
  nodes: AddonNode[];
  event: (
    node: AddonNode,
    event: "press" | "change",
    value: string | boolean | null,
  ) => void;
  link: (node: AddonNode, url: string) => void;
  /** False when the add-on may not open links; Markdown links then render
   *  as plain text instead of controls that can only be refused. */
  linksAllowed?: boolean;
}
/** The broker's limit for one UI event value (UTF-8 bytes). */
const MAX_VALUE_BYTES = 32768;
/** Echoes this field still expects from the add-on. Only an add-on that has
 *  stopped answering falls this far behind; its oldest echoes are forgotten. */
const MAX_PENDING_ECHOES = 1024;
const utf8 = new TextEncoder();
/** A small stand-in for a reported value (its length and 32-bit FNV-1a
 *  hash), so a long burst in a large text area keeps little memory. */
function fingerprint(value: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < value.length; i++)
    hash = Math.imul(hash ^ value.charCodeAt(i), 0x01000193);
  return `${value.length}:${hash >>> 0}`;
}
/**
 * TextField / TextArea adapter. The field owns what the user is typing: each
 * edit updates it at once and is reported to the add-on, and the add-on's
 * `value` echoes of those edits are ignored, so a slow round trip cannot
 * drop keystrokes or move the caret. A `value` the add-on sets on its own
 * (one this field never reported) replaces the text and keeps the caret.
 * Without a `value` the field is uncontrolled.
 */
function AddonTextInput({
  multiline,
  label,
  placeholder,
  disabled,
  value,
  className,
  onValue,
}: {
  multiline: boolean;
  label: string;
  placeholder: string;
  disabled: boolean;
  value: string | undefined;
  className: string;
  onValue: (value: string) => void;
}) {
  const [draft, setDraft] = useState(value ?? "");
  const [tooLong, setTooLong] = useState(false);
  const messageId = useId();
  const field = useRef<HTMLInputElement | HTMLTextAreaElement | null>(null);
  const current = useRef(draft);
  // Fingerprints of reported edits the add-on has not echoed yet, oldest first.
  const pending = useRef<string[]>([]);
  const composing = useRef(false);
  const deferred = useRef<string | undefined>(undefined);
  const caret = useRef<[number, number] | null>(null);
  const accept = (remote: string | undefined) => {
    if (remote === undefined) return;
    const echo = pending.current.indexOf(fingerprint(remote));
    if (echo !== -1) {
      // One of our own edits coming back, possibly late: drop it and every
      // older one, but never let it overwrite what was typed since.
      pending.current.splice(0, echo + 1);
      return;
    }
    if (remote === current.current) return;
    if (composing.current) {
      deferred.current = remote;
      return;
    }
    const el = field.current;
    caret.current =
      el && el.ownerDocument.activeElement === el
        ? [el.selectionStart ?? remote.length, el.selectionEnd ?? remote.length]
        : null;
    pending.current = [];
    current.current = remote;
    setTooLong(false);
    setDraft(remote);
  };
  useLayoutEffect(() => accept(value), [value]);
  useLayoutEffect(() => {
    const el = field.current;
    if (!caret.current || !el) return;
    const [start, end] = caret.current;
    caret.current = null;
    el.setSelectionRange(
      Math.min(start, draft.length),
      Math.min(end, draft.length),
    );
  }, [draft]);
  const attach = useCallback(
    (el: HTMLInputElement | HTMLTextAreaElement | null) => {
      field.current = el;
    },
    [],
  );
  const props = {
    ref: attach,
    value: draft,
    placeholder,
    disabled,
    "aria-invalid": tooLong || undefined,
    "aria-describedby": tooLong ? messageId : undefined,
    onChange: (
      e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>,
    ) => {
      const next = e.currentTarget.value;
      current.current = next;
      setDraft(next);
      const over = utf8.encode(next).byteLength > MAX_VALUE_BYTES;
      setTooLong(over);
      if (over) return;
      pending.current.push(fingerprint(next));
      if (pending.current.length > MAX_PENDING_ECHOES) pending.current.shift();
      onValue(next);
    },
    onCompositionStart: () => {
      composing.current = true;
    },
    onCompositionEnd: () => {
      composing.current = false;
      const later = deferred.current;
      deferred.current = undefined;
      accept(later);
    },
  };
  return (
    <div className={cn(className, "grid gap-1")}>
      <label className="grid gap-1">
        {label}
        {multiline ? <Textarea {...props} rows={3} /> : <Input {...props} />}
      </label>
      {tooLong && (
        <p id={messageId} className="text-label text-destructive">
          This text is too long to send to the add-on (32 KiB at most).
        </p>
      )}
    </div>
  );
}
function VirtualRows({
  rows,
  headers,
  label,
}: {
  rows: string[][];
  headers?: string[];
  label?: string;
}) {
  const [top, setTop] = useState(0);
  const rowHeight = 36;
  const start = Math.max(0, Math.floor(top / rowHeight) - 2);
  const end = Math.min(rows.length, start + 14);
  return (
    <div
      tabIndex={0}
      role={headers ? "table" : "list"}
      aria-label={label || (headers ? "Add-on table" : "Add-on list")}
      aria-rowcount={headers ? rows.length + 1 : undefined}
      className="max-h-72 overflow-auto rounded-md border"
      onScroll={(e) => setTop(e.currentTarget.scrollTop)}
    >
      {headers && (
        <div
          role="row"
          aria-rowindex={1}
          className="sticky top-0 z-10 flex bg-muted font-medium"
        >
          {headers.map((h, i) => (
            <span
              role="columnheader"
              className="min-w-0 flex-1 truncate px-2 py-2 text-label"
              key={i}
            >
              {h}
            </span>
          ))}
        </div>
      )}
      <div style={{ height: start * rowHeight }} aria-hidden />
      {rows.slice(start, end).map((row, index) => (
        <div
          key={start + index}
          role={headers ? "row" : "listitem"}
          aria-rowindex={headers ? start + index + 2 : undefined}
          aria-setsize={headers ? undefined : rows.length}
          aria-posinset={headers ? undefined : start + index + 1}
          className="flex h-9 items-center border-b text-label"
        >
          {row.map((cell, i) => (
            <span
              key={i}
              role={headers ? "cell" : undefined}
              className="min-w-0 flex-1 truncate px-2"
              title={cell}
            >
              {cell}
            </span>
          ))}
        </div>
      ))}
      <div style={{ height: (rows.length - end) * rowHeight }} aria-hidden />
    </div>
  );
}
/** Whether a Markdown link is one the broker may open (it checks again). */
function httpsHref(href: string | undefined): boolean {
  try {
    return new URL(href ?? "").protocol === "https:";
  } catch {
    return false;
  }
}
function plainText(node: AddonNode): string {
  return node.type === 3
    ? (node.data ?? "")
    : node.children.map(plainText).join("");
}
/** Semantic colors that have their own Button/Badge variant; the others are
 *  a text color on the neutral variant. */
const colorVariants: Record<string, "default" | "destructive"> = {
  accent: "default",
  danger: "destructive",
};
export function AddonRenderer({
  nodes,
  event,
  link,
  linksAllowed = true,
}: Props) {
  function render(node: AddonNode): ReactNode {
    if (node.type === 3) return node.data;
    if (node.type !== 1) return null;
    const p = node.properties;
    const text = (key: string, fallback = "") =>
      typeof p[key] === "string" ? (p[key] as string) : fallback;
    const disabled = p.disabled === true;
    const children = node.children.map((child) => (
      <Fragment key={child.id}>{render(child)}</Fragment>
    ));
    const className = cn(
      "min-w-0 max-w-full",
      sizes[text("size", "sm")],
      colors[text("color", "default")],
      p.width === "full" && "w-full",
      p.height === "full" && "h-full",
    );
    const layout = cn(
      className,
      spacing[text("spacing", "sm")],
      alignment[text("align", "stretch")],
    );
    const change = (value: string | boolean) => event(node, "change", value);
    switch (node.element) {
      case "cmx-stack":
        return (
          <div
            className={cn(
              layout,
              "flex",
              p.direction === "horizontal" ? "flex-row flex-wrap" : "flex-col",
            )}
          >
            {children}
          </div>
        );
      case "cmx-grid":
        return (
          <div className={cn(layout, "grid", columns[Number(p.columns) || 1])}>
            {children}
          </div>
        );
      case "cmx-card":
        return (
          <section
            className={cn(
              layout,
              "flex flex-col rounded-lg border bg-card p-3",
            )}
          >
            {text("title") && <h3 className="font-medium">{text("title")}</h3>}
            {children}
          </section>
        );
      case "cmx-text":
        return (
          <span className={cn(className, "whitespace-pre-wrap break-words")}>
            {children}
          </span>
        );
      case "cmx-heading":
        return (
          <div
            role="heading"
            aria-level={Number(p.level) || 2}
            className={cn(className, "font-semibold")}
          >
            {children}
          </div>
        );
      case "cmx-markdown":
        return (
          <div className={cn(className, "space-y-2 break-words")}>
            <Markdown
              skipHtml
              components={{
                img: () => null,
                a: ({ href, children }) =>
                  linksAllowed ? (
                    <button
                      type="button"
                      className="underline underline-offset-2"
                      disabled={!httpsHref(href)}
                      onClick={() => href && link(node, href)}
                    >
                      {children}
                    </button>
                  ) : (
                    <span>{children}</span>
                  ),
              }}
            >
              {plainText(node)}
            </Markdown>
          </div>
        );
      case "cmx-button": {
        const color = text("color", "default");
        return (
          <Button
            type="button"
            variant={colorVariants[color] ?? "outline"}
            size={p.size === "xs" ? "xs" : p.size === "lg" ? "default" : "sm"}
            className={cn(
              "min-w-0 max-w-full",
              !colorVariants[color] && colors[color],
              p.width === "full" && "w-full",
              p.height === "full" && "h-full",
            )}
            title={text("label") || undefined}
            disabled={disabled}
            onClick={() => event(node, "press", null)}
          >
            {/* One line, like every app button; a long label is cut off
                inside the panel instead of widening it. */}
            <span className="min-w-0 truncate">
              {children.length ? children : text("label")}
            </span>
          </Button>
        );
      }
      case "cmx-text-field":
      case "cmx-text-area":
        return (
          <AddonTextInput
            multiline={node.element === "cmx-text-area"}
            label={text("label")}
            placeholder={text("placeholder")}
            disabled={disabled}
            value={typeof p.value === "string" ? p.value : undefined}
            className={className}
            onValue={change}
          />
        );
      case "cmx-select":
        return (
          <label className={cn(className, "grid gap-1")}>
            {text("label")}
            <select
              className="rounded-md border bg-background p-2"
              value={text("value")}
              disabled={disabled}
              onChange={(e) => change(e.currentTarget.value)}
            >
              {((p.options as { label: string; value: string }[]) ?? []).map(
                (option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ),
              )}
            </select>
          </label>
        );
      case "cmx-checkbox":
        return (
          <label className={cn(className, "flex items-center gap-2")}>
            <input
              type="checkbox"
              checked={p.checked === true}
              disabled={disabled}
              onChange={(e) => change(e.currentTarget.checked)}
            />
            {text("label")}
            {children}
          </label>
        );
      case "cmx-switch":
        return (
          <label className={cn(className, "flex items-center gap-2")}>
            <Switch
              checked={p.checked === true}
              disabled={disabled}
              onCheckedChange={(checked) => change(checked)}
            />
            {text("label")}
            {children}
          </label>
        );
      case "cmx-tabs":
        return (
          <div className={className}>
            <div
              role="tablist"
              aria-label={text("label", "Add-on tabs")}
              className="flex gap-1 border-b"
            >
              {((p.options as { label: string; value: string }[]) ?? []).map(
                (option) => (
                  <button
                    type="button"
                    role="tab"
                    key={option.value}
                    aria-selected={p.value === option.value}
                    tabIndex={
                      p.value === option.value ||
                      (!p.value &&
                        option ===
                          (p.options as { label: string; value: string }[])[0])
                        ? 0
                        : -1
                    }
                    disabled={disabled}
                    onClick={() => change(option.value)}
                    onKeyDown={(e) => {
                      const options =
                        (p.options as { label: string; value: string }[]) ?? [];
                      const index = options.findIndex(
                        (item) => item.value === option.value,
                      );
                      const next =
                        e.key === "ArrowRight"
                          ? (index + 1) % options.length
                          : e.key === "ArrowLeft"
                            ? (index - 1 + options.length) % options.length
                            : e.key === "Home"
                              ? 0
                              : e.key === "End"
                                ? options.length - 1
                                : -1;
                      if (next < 0 || disabled) return;
                      e.preventDefault();
                      e.currentTarget.parentElement
                        ?.querySelectorAll<HTMLButtonElement>('[role="tab"]')
                        [next]?.focus();
                      change(options[next].value);
                    }}
                    className={cn(
                      "px-3 py-2",
                      p.value === option.value && "border-b-2 border-primary",
                    )}
                  >
                    {option.label}
                  </button>
                ),
              )}
            </div>
            <div role="tabpanel" className="pt-2">
              {children}
            </div>
          </div>
        );
      case "cmx-list":
        return (
          <VirtualRows
            rows={((p.items as string[]) ?? []).map((item) => [item])}
            label={text("label")}
          />
        );
      case "cmx-table":
        return (
          <VirtualRows
            rows={(p.rows as string[][]) ?? []}
            headers={(p.headers as string[]) ?? []}
            label={text("label")}
          />
        );
      case "cmx-badge": {
        const color = text("color", "default");
        return (
          <Badge
            variant={colorVariants[color] ?? "secondary"}
            className={cn(
              "max-w-full",
              !colorVariants[color] && color !== "default" && colors[color],
            )}
          >
            {children}
          </Badge>
        );
      }
      case "cmx-progress":
        return (
          <progress
            aria-label={text("label", "Progress")}
            max={typeof p.max === "number" ? p.max : 100}
            value={typeof p.value === "number" ? p.value : 0}
            className="h-2 w-full"
          />
        );
      case "cmx-icon": {
        const Icon = addonIcon(text("name"));
        return (
          <Icon
            aria-label={text("label") || undefined}
            aria-hidden={!text("label")}
            className={cn("size-4 shrink-0", colors[text("color", "default")])}
          />
        );
      }
      case "cmx-divider":
        return <Separator />;
      case "cmx-empty-state":
        return (
          <div
            className={cn(
              className,
              "rounded-lg border border-dashed p-6 text-center",
            )}
          >
            <h3 className="font-medium">{text("title")}</h3>
            <div className="mt-2 text-muted-foreground">{children}</div>
          </div>
        );
      default:
        return null;
    }
  }
  return (
    <>
      {nodes.map((node) => (
        <Fragment key={node.id}>{render(node)}</Fragment>
      ))}
    </>
  );
}
