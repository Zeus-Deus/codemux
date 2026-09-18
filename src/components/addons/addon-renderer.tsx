import { Fragment, useState, type ReactNode } from "react";
import Markdown from "react-markdown";
import {
  BookOpen,
  Check,
  FileText,
  GitBranch,
  Github,
  Info,
  Link,
  List,
  Plus,
  RefreshCw,
  Settings,
  type LucideIcon,
} from "lucide-react";
import type { AddonNode } from "@/lib/addons/types";
import { cn } from "@/lib/utils";
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
};
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
  success: "text-emerald-600 dark:text-emerald-400",
  warning: "text-amber-600 dark:text-amber-400",
  danger: "text-destructive",
  accent: "text-primary",
};
const sizes: Record<string, string> = {
  xs: "text-xs",
  sm: "text-sm",
  md: "text-base",
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
}
function VirtualRows({
  rows,
  headers,
}: {
  rows: string[][];
  headers?: string[];
}) {
  const [top, setTop] = useState(0);
  const rowHeight = 36;
  const start = Math.max(0, Math.floor(top / rowHeight) - 2);
  const end = Math.min(rows.length, start + 14);
  return (
    <div
      tabIndex={0}
      role={headers ? "table" : "list"}
      aria-label={headers ? "Add-on table" : "Add-on list"}
      aria-rowcount={headers ? rows.length : undefined}
      className="max-h-72 overflow-auto rounded-md border"
      onScroll={(e) => setTop(e.currentTarget.scrollTop)}
    >
      {headers && (
        <div role="row" className="sticky top-0 z-10 flex bg-muted font-medium">
          {headers.map((h, i) => (
            <span
              role="columnheader"
              className="min-w-0 flex-1 truncate px-2 py-2 text-xs"
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
          aria-rowindex={headers ? start + index + 1 : undefined}
          className="flex h-9 items-center border-b text-xs"
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
function plainText(node: AddonNode): string {
  return node.type === 3
    ? (node.data ?? "")
    : node.children.map(plainText).join("");
}
export function AddonRenderer({ nodes, event, link }: Props) {
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
                a: ({ href, children }) => (
                  <button
                    className="underline underline-offset-2"
                    disabled={!href?.startsWith("https://")}
                    onClick={() => href && link(node, href)}
                  >
                    {children}
                  </button>
                ),
              }}
            >
              {plainText(node)}
            </Markdown>
          </div>
        );
      case "cmx-button":
        return (
          <button
            type="button"
            className={cn(
              className,
              "inline-flex items-center justify-center gap-2 rounded-md border bg-secondary px-3 py-1.5 hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50",
            )}
            disabled={disabled}
            onClick={() => event(node, "press", null)}
          >
            {children.length ? children : text("label")}
          </button>
        );
      case "cmx-text-field":
      case "cmx-text-area": {
        const field = {
          className:
            "w-full rounded-md border bg-background px-2 py-1.5 text-sm focus-visible:outline-2 focus-visible:outline-ring",
          value: text("value"),
          placeholder: text("placeholder"),
          disabled,
          onChange: (
            e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>,
          ) => change(e.currentTarget.value),
        };
        return (
          <label className={cn(className, "grid gap-1")}>
            {text("label")}
            {node.element === "cmx-text-area" ? (
              <textarea {...field} rows={3} />
            ) : (
              <input {...field} />
            )}
          </label>
        );
      }
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
      case "cmx-switch":
        return (
          <label className={cn(className, "flex items-center gap-2")}>
            <input
              type="checkbox"
              role={node.element === "cmx-switch" ? "switch" : undefined}
              checked={p.checked === true}
              disabled={disabled}
              onChange={(e) => change(e.currentTarget.checked)}
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
          />
        );
      case "cmx-table":
        return (
          <VirtualRows
            rows={(p.rows as string[][]) ?? []}
            headers={(p.headers as string[]) ?? []}
          />
        );
      case "cmx-badge":
        return (
          <span
            className={cn(
              className,
              "inline-flex rounded-md bg-muted px-2 py-0.5 text-xs",
            )}
          >
            {children}
          </span>
        );
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
        return <hr className="border-border" />;
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
