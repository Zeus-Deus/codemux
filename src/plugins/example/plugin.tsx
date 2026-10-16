/**
 * The example plugin: one contribution in every area, so turning it on and
 * off exercises the whole plugin surface. It is deliberately plain. Off by
 * default; for development and the plugin gates only.
 */
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  APP_OVERLAY_AREA,
  APPEARANCE_EXTRA_AREA,
  Badge,
  Button,
  COMPOSER_ACCESSORY_AREA,
  COMPOSER_ACTIONS_AREA,
  COMPOSER_FOOTER_AREA,
  definePlugin,
  host,
  icons,
  KEYBINDS_AREA,
  LANDING_BACKDROP_AREA,
  LANDING_BELOW_COMPOSER_AREA,
  PAGES_AREA,
  PALETTE_AREA,
  RIGHT_PANEL_AREA,
  SIDEBAR_EMPTY_STATE_AREA,
  SIDEBAR_SECTION_AREA,
  Switch,
  THEMES_AREA,
  TITLEBAR_ACTIONS_AREA,
  TOOL_RENDERER_AREA,
  type LandingBackdropProps,
  type PluginContext,
  type ToolCallView,
} from "@codemux/sdk";

const tag = { "data-example-plugin": "" } as const;

function Chip({ children }: { children: ReactNode }) {
  return (
    <span
      {...tag}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "2px 8px",
        borderRadius: 6,
        border: `1px solid var(${host.theme.cssVar("border")})`,
        color: `var(${host.theme.cssVar("mutedForeground")})`,
        fontSize: 12,
      }}
    >
      {children}
    </span>
  );
}

function RightPane({ workspaceId }: { workspaceId: string }) {
  const [count, setCount] = useState(0);
  return (
    <div {...tag} style={{ padding: 16, display: "grid", gap: 12 }}>
      <strong>Example pane</strong>
      <span>Workspace: {workspaceId ? "active" : "none"}</span>
      <Button size="sm" variant="outline" onClick={() => setCount((n) => n + 1)}>
        Clicked {count} times
      </Button>
    </div>
  );
}

function Backdrop({ onPulse }: LandingBackdropProps) {
  const canvas = useRef<HTMLCanvasElement | null>(null);
  useEffect(() => {
    const el = canvas.current;
    const ctx = el?.getContext("2d");
    if (!el || !ctx) return;
    const pulses: { x: number; y: number; t: number }[] = [];
    let frame = 0;
    const fit = () => {
      el.width = el.clientWidth;
      el.height = el.clientHeight;
    };
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    fit();
    const off = onPulse((x, y) => {
      const r = el.getBoundingClientRect();
      pulses.push({ x: x - r.left, y: y - r.top, t: performance.now() });
    });
    const draw = (now: number) => {
      const accent =
        getComputedStyle(document.documentElement).getPropertyValue(host.theme.cssVar("primary")).trim() ||
        "#888";
      ctx.clearRect(0, 0, el.width, el.height);
      ctx.strokeStyle = accent;
      for (let i = pulses.length - 1; i >= 0; i -= 1) {
        const age = (now - pulses[i].t) / 900;
        if (age > 1) {
          pulses.splice(i, 1);
          continue;
        }
        ctx.globalAlpha = 1 - age;
        ctx.beginPath();
        ctx.arc(pulses[i].x, pulses[i].y, 10 + age * 120, 0, Math.PI * 2);
        ctx.stroke();
      }
      ctx.globalAlpha = 1;
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(frame);
      ro.disconnect();
      off();
    };
  }, [onPulse]);
  return (
    <canvas {...tag} ref={canvas} style={{ position: "absolute", inset: 0, width: "100%", height: "100%" }} />
  );
}

function ToolCard({ call }: { call: ToolCallView }) {
  return (
    <div {...tag} style={{ padding: "6px 10px", fontSize: 12, opacity: 0.8 }}>
      Example renderer for {call.tool_name} ({call.status})
    </div>
  );
}

function SettingsPage({ ctx }: { ctx: PluginContext }) {
  const [on, setOn] = useState(() => ctx.storage.get("flag", false));
  return (
    <label {...tag} style={{ display: "flex", alignItems: "center", gap: 12 }}>
      <Switch
        checked={on}
        onCheckedChange={(next) => {
          setOn(next);
          ctx.storage.set("flag", next);
        }}
      />
      A stored example flag
    </label>
  );
}

const chip = (text: string, padding = "4px 12px") => () => (
  <div style={{ padding }}>
    <Chip>{text}</Chip>
  </div>
);

export default definePlugin({
  id: "example",
  name: "Example",
  register(ctx) {
    ctx.registerMany([
      {
        id: "hello",
        area: PALETTE_AREA,
        data: {
          label: "Example plugin: say hello",
          icon: icons.Sparkles,
          run: () => host.notify({ message: "Hello from the bundled example plugin" }),
        },
      },
      {
        id: "page-cmd",
        area: PALETTE_AREA,
        data: { label: "Example plugin: open page", run: () => host.openPage("example:page") },
      },
      {
        id: "pane-cmd",
        area: PALETTE_AREA,
        data: {
          label: "Example plugin: open pane",
          requiresWorkspace: true,
          run: () => host.openRightPane("example:pane"),
        },
      },
      {
        id: "key",
        area: KEYBINDS_AREA,
        data: {
          label: "Example plugin shortcut",
          defaultKeys: "Ctrl+Alt+E",
          run: () => host.notify({ message: "Example shortcut pressed" }),
        },
      },
      { id: "pane", area: RIGHT_PANEL_AREA, title: "Example", data: { icon: icons.Sparkles }, render: RightPane },
      {
        id: "page",
        area: PAGES_AREA,
        title: "Example page",
        render: () => (
          <div {...tag} style={{ padding: 24 }}>
            A full plugin page.
          </div>
        ),
      },
      { id: "appearance", area: APPEARANCE_EXTRA_AREA, render: chip("Example appearance option", "24px 0 0") },
      {
        id: "insert",
        area: COMPOSER_ACTIONS_AREA,
        data: {
          label: "Insert example text",
          icon: icons.Sparkles,
          run: ({ composer }: { composer: { insertText(t: string): boolean } }) => {
            composer.insertText("Hello from the example plugin");
          },
        },
      },
      { id: "accessory", area: COMPOSER_ACCESSORY_AREA, render: chip("Example accessory") },
      { id: "footer", area: COMPOSER_FOOTER_AREA, render: chip("Example footer") },
      {
        id: "titlebar",
        area: TITLEBAR_ACTIONS_AREA,
        render: () => (
          <Button
            {...tag}
            size="icon-sm"
            variant="ghost"
            aria-label="Example title bar action"
            onPointerDown={(e) => e.stopPropagation()}
            onClick={() => host.notify({ message: "Example title bar action" })}
          >
            <icons.Sparkles />
          </Button>
        ),
      },
      { id: "sidebar", area: SIDEBAR_SECTION_AREA, render: chip("Example sidebar section", "8px 12px") },
      { id: "empty", area: SIDEBAR_EMPTY_STATE_AREA, render: () => <Chip>Example empty-sidebar art</Chip> },
      { id: "backdrop", area: LANDING_BACKDROP_AREA, render: Backdrop },
      { id: "below", area: LANDING_BELOW_COMPOSER_AREA, render: () => <Chip>Example below the composer</Chip> },
      { id: "bash", area: TOOL_RENDERER_AREA, data: { tool: "Bash", render: ToolCard } },
      {
        id: "theme",
        area: THEMES_AREA,
        data: {
          label: "Example Dusk",
          scheme: "dark",
          roles: { background: "#1b1a2e", foreground: "#e8e6f5", primary: "#b9a3ff", accent: "#2d2a4a" },
        },
      },
      {
        id: "overlay",
        area: APP_OVERLAY_AREA,
        render: () => (
          <div {...tag} style={{ position: "fixed", right: 12, bottom: 12, zIndex: 40, pointerEvents: "none" }}>
            <Badge variant="outline">Example overlay</Badge>
          </div>
        ),
      },
    ]);
    ctx.registerSettingsPage({ title: "Example", render: () => <SettingsPage ctx={ctx} /> });
  },
});
