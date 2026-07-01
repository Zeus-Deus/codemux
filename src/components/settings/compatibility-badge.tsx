import { AlertCircle, AlertTriangle } from "lucide-react";

import { cn } from "@/lib/utils";
import type { SkillCompatibility } from "@/tauri/commands";

interface Props {
  level: Exclude<SkillCompatibility, "compatible">;
}

interface BadgeConfig {
  icon: typeof AlertTriangle;
  className: string;
  label: string;
}

const CONFIG: Record<Props["level"], BadgeConfig> = {
  "soft-warn": {
    icon: AlertTriangle,
    // Amber → "may reference external tools" (run-of-the-mill bash
    // blocks, CLI mentions). Distinct enough from foreground that it
    // reads as a status without grabbing the whole row's attention.
    className: "bg-status-working/15 text-status-working dark:text-status-working",
    label: "Tool refs",
  },
  "hard-warn": {
    icon: AlertCircle,
    // Destructive token → "may not work in current session" (allowed-
    // tools frontmatter, mcp__ refs, foreign provider paths). Treats
    // it like a real warning the user should consider before invoking.
    className: "bg-destructive/15 text-destructive",
    label: "May not work",
  },
};

/**
 * Compact compatibility chip rendered next to the skill name in the
 * Settings → Skills list and at the top of the View modal. The
 * `compatibility-signals` array on the skill explains the *why*; this
 * badge is the at-a-glance summary.
 */
export function CompatibilityBadge({ level }: Props) {
  const config = CONFIG[level];
  const Icon = config.icon;
  return (
    <span
      data-testid="compatibility-badge"
      data-level={level}
      className={cn(
        "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium",
        config.className,
      )}
    >
      <Icon className="size-2.5" aria-hidden />
      {config.label}
    </span>
  );
}
