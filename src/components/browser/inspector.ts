import type { PaneNodeSnapshot, SurfaceSnapshot } from "@/tauri/types";

export interface ElementInfo {
  tag: string;
  id: string;
  classes: string[];
  text: string;
  selector: string;
  rect: { x: number; y: number; width: number; height: number };
}

// Injected JS that adds mousemove hover highlighting.
// Uses only double quotes and backticks — no single quotes (shell_quote wraps in single quotes).
export const INSPECTOR_INJECT_SCRIPT = `(() => {
  if (window.__codemux_inspector) return "already_injected";
  const style = document.createElement("style");
  style.id = "__codemux_inspector_style";
  style.textContent = ".__codemux_highlight { outline: 2px solid #3b82f6 !important; outline-offset: -1px; }";
  document.head.appendChild(style);
  let lastEl = null;
  function onMove(e) {
    const el = document.elementFromPoint(e.clientX, e.clientY);
    if (el === lastEl) return;
    if (lastEl) lastEl.classList.remove("__codemux_highlight");
    if (el) el.classList.add("__codemux_highlight");
    lastEl = el;
  }
  document.addEventListener("mousemove", onMove);
  window.__codemux_inspector = { style: style, listener: onMove, lastEl: null };
  return "inspector_enabled";
})()`;

// Cleanup script to remove all injected inspector artifacts.
export const INSPECTOR_CLEANUP_SCRIPT = `(() => {
  const ins = window.__codemux_inspector;
  if (!ins) return "not_injected";
  document.removeEventListener("mousemove", ins.listener);
  if (ins.lastEl) ins.lastEl.classList.remove("__codemux_highlight");
  const style = document.getElementById("__codemux_inspector_style");
  if (style) style.remove();
  document.querySelectorAll(".__codemux_highlight").forEach(function(el) {
    el.classList.remove("__codemux_highlight");
  });
  delete window.__codemux_inspector;
  return "inspector_cleaned";
})()`;

// Returns JS that queries the element at viewport coordinates (x, y) and returns JSON info.
export function buildElementQueryScript(x: number, y: number): string {
  return `(() => {
  const el = document.elementFromPoint(${x}, ${y});
  if (!el) return JSON.stringify(null);
  function buildSelector(node) {
    if (node.id) return "#" + node.id;
    const parts = [];
    let cur = node;
    while (cur && cur !== document.body && cur !== document.documentElement) {
      let seg = cur.tagName.toLowerCase();
      if (cur.id) { parts.unshift("#" + cur.id); break; }
      const parent = cur.parentElement;
      if (parent) {
        const siblings = Array.from(parent.children).filter(function(c) { return c.tagName === cur.tagName; });
        if (siblings.length > 1) {
          seg += ":nth-of-type(" + (siblings.indexOf(cur) + 1) + ")";
        }
      }
      parts.unshift(seg);
      cur = parent;
    }
    return parts.join(" > ");
  }
  const r = el.getBoundingClientRect();
  return JSON.stringify({
    tag: el.tagName.toLowerCase(),
    id: el.id || "",
    classes: Array.from(el.classList).filter(function(c) { return c !== "__codemux_highlight"; }),
    text: (el.textContent || "").substring(0, 120).trim(),
    selector: buildSelector(el),
    rect: { x: r.x, y: r.y, width: r.width, height: r.height }
  });
})()`;
}

// Parse the result from agentBrowserRun eval into ElementInfo.
export function parseEvalResult(result: unknown): ElementInfo | null {
  try {
    const data = (result as { data?: unknown })?.data;
    if (!data) return null;
    let raw: string;
    if (typeof data === "string") {
      raw = data;
    } else {
      const obj = data as Record<string, unknown>;
      raw = (obj.raw ?? obj.result ?? JSON.stringify(data)) as string;
    }
    // agent-browser eval may wrap the result — try to extract JSON
    const jsonMatch = raw.match(/\{[\s\S]*\}/);
    if (!jsonMatch) return null;
    const parsed = JSON.parse(jsonMatch[0]);
    if (!parsed || !parsed.tag) return null;
    return parsed as ElementInfo;
  } catch {
    return null;
  }
}

/** Where "Send to agent" delivers an inspected element. */
export type AgentTarget =
  | { kind: "agent_chat"; paneId: string; threadId: string; title: string }
  | { kind: "terminal"; paneId: string; sessionId: string; title: string };

function leafPanes(node: PaneNodeSnapshot, out: PaneNodeSnapshot[] = []): PaneNodeSnapshot[] {
  if (node.kind === "split") {
    for (const child of node.children) leafPanes(child, out);
  } else {
    out.push(node);
  }
  return out;
}

function isChatPane(node: PaneNodeSnapshot): boolean {
  return node.kind === "agent_chat" && !!node.thread_id;
}

function toTarget(node: PaneNodeSnapshot | undefined): AgentTarget | null {
  if (node?.kind === "agent_chat" && node.thread_id) {
    return {
      kind: "agent_chat",
      paneId: node.pane_id,
      threadId: node.thread_id,
      title: node.title || "Agent chat",
    };
  }
  if (node?.kind === "terminal") {
    return {
      kind: "terminal",
      paneId: node.pane_id,
      sessionId: node.session_id,
      title: node.title || "Terminal",
    };
  }
  return null;
}

/**
 * The pane an inspected element should go to. An agent chat beats a
 * terminal (a terminal may be a plain shell); within each kind the
 * surface's active pane wins over tree order.
 */
export function findAgentTarget(surface: SurfaceSnapshot): AgentTarget | null {
  const leaves = leafPanes(surface.root);
  const active = leaves.find((n) => n.pane_id === surface.active_pane_id);
  if (active && isChatPane(active)) return toTarget(active);
  const chat = leaves.find(isChatPane);
  if (chat) return toTarget(chat);
  if (active?.kind === "terminal") return toTarget(active);
  return toTarget(leaves.find((n) => n.kind === "terminal"));
}

// C0 controls, DEL and C1 controls. A terminal acts on these (CR/LF
// submit the line, ^C/^D signal, ESC starts a sequence), so none of them
// may reach a PTY.
const CONTROL_CHARS = /[\u0000-\u001f\u007f-\u009f]+/g;

// A whole surrogate pair, or a surrogate without its other half. The page
// can supply a lone one (its own text cut splits pairs), and a lone
// surrogate fails JSON deserialization on the Rust side.
const SURROGATES = /[\ud800-\udbff][\udc00-\udfff]|[\ud800-\udfff]/g;

/**
 * A complete reference to the element, left open for the user's
 * instruction. The tag, text and selector come from the page (an element
 * id can hold a newline), so every control character is replaced with a
 * space: the result is one inert line that a terminal never submits. Lone
 * surrogates become U+FFFD so the prompt always serializes.
 */
export function buildTellAgentPrompt(element: ElementInfo, pageUrl: string): string {
  // Truncate by code point so the cut never splits a surrogate pair.
  const text = Array.from(element.text.replace(/\s+/g, " ").trim()).slice(0, 60).join("");
  const desc = `<${element.tag}>${text ? ` "${text}"` : ""}`;
  const where = pageUrl && pageUrl !== "about:blank" ? ` at ${pageUrl}` : "";
  const prompt = `In the browser${where}, the element \`${element.selector}\` (${desc}): `;
  return prompt.replace(CONTROL_CHARS, " ").replace(SURROGATES, (m) => (m.length === 2 ? m : "\ufffd"));
}
