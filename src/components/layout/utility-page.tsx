import { createContext, useContext, type ReactNode } from "react";
import { ArrowLeft } from "lucide-react";

import { Button } from "@/components/ui/button";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useTitlebarOverlay } from "@/hooks/use-gui-chrome";
import { WINDOW_CONTROLS_RESERVE } from "@/lib/titlebar-geometry";
import { WindowChrome } from "./window-chrome";

/**
 * True when a utility page has the whole window to itself: on mobile, and
 * when there is no workspace for the sidebar to show. The page then draws
 * its own window chrome and back arrow, since there is no sidebar footer
 * to hold the Back button.
 */
export const UtilityPageStandaloneContext = createContext(false);

interface UtilityPageProps {
  title: ReactNode;
  /** Accessible name of the standalone back arrow, e.g. "Close devices". */
  backLabel: string;
  onBack: () => void;
  /** Right-aligned header content. */
  actions?: ReactNode;
  children: ReactNode;
}

/**
 * Chrome shared by the utility pages (Automations, Devices, Pull requests).
 *
 * On desktop these open in the main area with the sidebar still beside
 * them, the way a workspace does; the sidebar footer's Back button leads
 * out. The header is the titlebar band's height so its title sits on the
 * same line as the window controls.
 */
export function UtilityPage({ title, backLabel, onBack, actions, children }: UtilityPageProps) {
  const standalone = useContext(UtilityPageStandaloneContext);
  const overlay = useTitlebarOverlay();

  if (standalone) {
    return (
      <div className="relative flex h-screen flex-col bg-background">
        <WindowChrome />
        {/* `pt-7` (= the 28px WindowChrome drag strip) keeps the back
            button's hit area entirely below the drag region. */}
        <div className="flex shrink-0 items-center gap-2 px-3 pb-2 pt-7">
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={backLabel}
            className="text-muted-foreground hover:bg-surface-2 hover:text-foreground"
            onClick={onBack}
          >
            <ArrowLeft className="size-4" />
          </Button>
          <h1 className="min-w-0 truncate text-body font-medium text-foreground">{title}</h1>
          <div className="min-w-0 flex-1" />
          {actions}
        </div>
        <div className="flex min-h-0 flex-1 flex-col">{children}</div>
      </div>
    );
  }

  // The floating titlebar keeps the native window buttons in the top-right
  // corner over this header, so the header stops short of them.
  const rightReserve = overlay && !isRemoteClient() ? WINDOW_CONTROLS_RESERVE + 6 : 12;

  return (
    <div className="flex h-full min-h-0 flex-col bg-background" data-testid="utility-page">
      <header
        data-tauri-drag-region
        className="flex h-10 shrink-0 items-center gap-3 pl-5"
        style={{ paddingRight: rightReserve }}
      >
        <h1
          data-tauri-drag-region
          className="min-w-0 truncate text-body font-medium text-foreground"
        >
          {title}
        </h1>
        <div data-tauri-drag-region className="min-w-0 flex-1 self-stretch" />
        {actions}
      </header>
      <div className="flex min-h-0 flex-1 flex-col">{children}</div>
    </div>
  );
}
