import { useMemo } from "react";
import { normalizeColor, type ThemeDefinition } from "@/lib/themes";

/**
 * A miniature Codemux wearing `theme` — sidebar, workspace header, a chat
 * exchange, a diff card and a terminal strip.
 *
 * This is the Theme Studio's whole preview story. The studio used to apply
 * the candidate to the real app and revert on close; it now paints it here
 * instead, so the page behind the modal keeps the theme you actually have and
 * nothing has to be put back if you cancel. It also means the preview shows
 * surfaces the settings page doesn't contain — a diff, a terminal — which is
 * where a palette usually falls apart.
 *
 * Every color is an inline style read off the theme object. That is the one
 * place in the app where that is correct: the whole point is to render a
 * palette that is *not* the active one, so semantic utilities (which resolve
 * to the applied theme) cannot be used.
 */
export function ThemePreviewShell({ theme }: { theme: ThemeDefinition }) {
  const c = useMemo(() => paletteOf(theme), [theme]);

  return (
    <div
      className="flex h-full min-h-0 overflow-hidden rounded-[11px] border"
      style={{ background: c.bg, borderColor: c.border }}
      aria-hidden="true"
    >
      {/* Sidebar */}
      <div
        className="flex w-[118px] flex-none flex-col gap-1.5 border-r p-2"
        style={{ background: c.sidebar, borderColor: c.border }}
      >
        <span className="h-[22px] rounded-md" style={{ background: c.card }} />
        <span
          className="flex h-6 items-center gap-1.5 rounded-md px-[7px]"
          style={{ background: c.card }}
        >
          <span
            className="size-[5px] flex-none rounded-full"
            style={{ background: c.accent }}
          />
          <span className="text-[10px] font-semibold" style={{ color: c.fg }}>
            auth-refactor
          </span>
        </span>
        {["web-scraper", "migrations"].map((name) => (
          <span
            key={name}
            className="flex h-6 items-center px-[7px] text-[10px]"
            style={{ color: c.fg2 }}
          >
            {name}
          </span>
        ))}
      </div>

      <div className="flex min-w-0 flex-1 flex-col">
        {/* Workspace header */}
        <div
          className="flex h-[30px] flex-none items-center gap-2.5 border-b px-3"
          style={{ background: c.bg2, borderColor: c.border }}
        >
          <span className="text-[11px] font-semibold" style={{ color: c.fg }}>
            auth-refactor
          </span>
          <span className="font-mono text-[10px]" style={{ color: c.fg3 }}>
            feat/oauth
          </span>
        </div>

        {/* Chat */}
        <div className="flex min-h-0 flex-1 flex-col gap-2.5 overflow-hidden p-4">
          <span
            className="max-w-[56%] self-end rounded-[10px] px-[11px] py-2 text-[11px] leading-[1.55]"
            style={{ background: c.card, color: c.fg }}
          >
            Rename the callback and fix the imports
          </span>
          <span
            className="max-w-[76%] text-[11px] leading-[1.65]"
            style={{ color: c.fg2 }}
          >
            Renamed it and left a deprecated re-export so nothing downstream
            breaks. One call site in{" "}
            <span className="font-mono" style={{ color: c.accent }}>
              session.ts
            </span>{" "}
            needed the update.
          </span>

          {/* Diff card — the tints are the reason a palette gets judged */}
          <span
            className="max-w-[76%] overflow-hidden rounded-[9px] border"
            style={{ borderColor: c.border }}
          >
            <span
              className="flex h-[25px] items-center gap-2.5 border-b px-2.5"
              style={{ background: c.bg2, borderColor: c.border }}
            >
              <span className="font-mono text-[10px]" style={{ color: c.fg2 }}>
                src/auth/callback.ts
              </span>
              <span className="flex-1" />
              <span className="font-mono text-[10px]" style={{ color: c.red }}>
                −1
              </span>
              <span className="font-mono text-[10px]" style={{ color: c.green }}>
                +2
              </span>
            </span>
            <span
              className="flex flex-col py-1.5 font-mono text-[10.5px] leading-[1.85]"
              style={{ color: c.fg3 }}
            >
              <span className="px-2.5">
                <span style={{ color: c.magenta }}>export function </span>
                <span style={{ color: c.blue }}>handleCallback</span>() {"{"}
              </span>
              <span
                className="border-l-2 px-2.5"
                style={{ background: c.delBg, borderColor: c.red }}
              >
                <span style={{ color: c.fg2 }}>{"  const s = "}</span>
                <span style={{ color: c.cyan }}>getSess</span>()
              </span>
              <span
                className="border-l-2 px-2.5"
                style={{ background: c.addBg, borderColor: c.green }}
              >
                <span style={{ color: c.fg2 }}>{"  const s = "}</span>
                <span style={{ color: c.cyan }}>getSession</span>()
              </span>
            </span>
          </span>
        </div>

        {/* Terminal — proves the ANSI slots, not just the chrome */}
        <div
          className="flex h-[104px] flex-none flex-col gap-[3px] border-t px-3.5 py-2.5 font-mono text-[10.5px] leading-[1.65]"
          style={{ background: c.terminal, borderColor: c.border }}
        >
          <span>
            <span style={{ color: c.green }}>➜</span>{" "}
            <span style={{ color: c.cyan }}>codemux</span>{" "}
            <span style={{ color: c.magenta }}>git:(</span>
            <span style={{ color: c.red }}>feat/oauth</span>
            <span style={{ color: c.magenta }}>)</span> npm run dev
          </span>
          <span style={{ color: c.fg3 }}>VITE v7.1.1 ready in 1.24s</span>
          <span style={{ color: c.fg2 }}>
            ➜ Local:{" "}
            <span style={{ color: c.blue }}>http://localhost:1425/</span>
          </span>
          <span style={{ color: c.yellow }}>⚠ 2 deprecation warnings</span>
        </div>
      </div>
    </div>
  );
}

/**
 * Flatten a theme to the concrete colors this preview paints with.
 *
 * Roles may be authored as `oklch(… / 10%)` (the built-ins' borders are), and
 * an alpha color composited over the *studio's* background rather than the
 * candidate's would show the wrong edge — so every role is normalized against
 * the candidate's own canvas first.
 */
function paletteOf(theme: ThemeDefinition) {
  const bg = normalizeColor(theme.roles.background) ?? "#000000";
  const on = (value: string, fallback: string) =>
    normalizeColor(value, bg) ?? fallback;
  return {
    bg,
    bg2: on(theme.roles.muted, bg),
    card: on(theme.roles.card, bg),
    sidebar: on(theme.roles.sidebar, bg),
    border: on(theme.roles.border, bg),
    fg: on(theme.roles.foreground, "#ffffff"),
    fg2: on(theme.roles.mutedForeground, "#aaaaaa"),
    fg3: on(theme.roles.ring, "#777777"),
    accent: on(theme.roles.brandAccent, "#e07850"),
    terminal: theme.ansi.black,
    red: theme.ansi.red,
    green: theme.ansi.green,
    yellow: theme.ansi.yellow,
    blue: theme.ansi.blue,
    magenta: theme.ansi.magenta,
    cyan: theme.ansi.cyan,
    // The diff tints the real viewer uses: the ANSI hue at low alpha over the
    // canvas, flattened here because the runtime stores opaque colors.
    addBg: mixOver(theme.ansi.green, bg, 0.12),
    delBg: mixOver(theme.ansi.red, bg, 0.13),
  };
}

function mixOver(color: string, backdrop: string, alpha: number): string {
  const top = hexChannels(normalizeColor(color) ?? "#000000");
  const base = hexChannels(normalizeColor(backdrop) ?? "#000000");
  const channel = (i: number) =>
    Math.round(top[i]! * alpha + base[i]! * (1 - alpha));
  return `rgb(${channel(0)}, ${channel(1)}, ${channel(2)})`;
}

function hexChannels(hex: string): [number, number, number] {
  const raw = hex.replace("#", "");
  return [
    Number.parseInt(raw.slice(0, 2), 16) || 0,
    Number.parseInt(raw.slice(2, 4), 16) || 0,
    Number.parseInt(raw.slice(4, 6), 16) || 0,
  ];
}
