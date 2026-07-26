// Settings → Appearance → "Scrolling" subsection.
//
// Linux-only surface: the toggle drives a WebKitGTK webview setting, so it
// renders nothing on the platforms where it would be inert (macOS WKWebView,
// Windows WebView2, the dev-mock Chromium). The preference is machine-local
// (`appearance.smooth_scrolling` in the settings store) rather than synced —
// it describes the webview on *this* machine, not a user's taste that should
// follow them to a Mac.

import { Switch } from "@/components/ui/switch";
import { applySmoothScrolling } from "@/hooks/use-smooth-scrolling";
import { isLinuxWebKitGtk } from "@/lib/webkit";
import { selectSmoothScrolling, useSettingsStore } from "@/stores/settings-store";

export function SmoothScrollingSection() {
  const enabled = useSettingsStore(selectSmoothScrolling);
  const setSetting = useSettingsStore((s) => s.set);

  const userAgent = typeof navigator !== "undefined" ? navigator.userAgent : "";
  if (!isLinuxWebKitGtk(userAgent)) return null;

  const handleToggle = (next: boolean) => {
    // Persist first so the choice survives even if the webview call fails,
    // then push it to the live webview (applies immediately, no restart).
    setSetting("appearance.smooth_scrolling", next ? "true" : "false");
    void applySmoothScrolling(next);
  };

  return (
    <section className="mt-10 first:mt-0">
      <div className="mb-3 flex items-end justify-between gap-4">
        <div className="min-w-0">
          <p className="font-mono text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground/55">
            Scrolling
          </p>
          <p className="text-[12px] text-muted-foreground/80 mt-1.5 leading-relaxed max-w-prose">
            How the mouse wheel moves content in this webview.
          </p>
        </div>
      </div>
      <div className="space-y-1">
        <div className="flex items-center justify-between gap-8 py-4">
          <div className="space-y-1 min-w-0">
            <p className="text-[13.5px] font-semibold leading-tight text-foreground">
              Smooth scrolling
            </p>
            <p className="text-[12px] text-muted-foreground/80 leading-relaxed">
              Animate mouse-wheel scrolling instead of jumping straight to the
              new position. Off by default: with a high-resolution or free-spin
              wheel the animation falls behind, so scrolling faster makes the
              page move slower. Applies immediately.
            </p>
          </div>
          <div className="shrink-0">
            <Switch
              checked={enabled}
              onCheckedChange={handleToggle}
              aria-label="Toggle smooth scrolling"
            />
          </div>
        </div>
      </div>
    </section>
  );
}
