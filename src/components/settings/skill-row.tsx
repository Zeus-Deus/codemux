import { BookOpen, ExternalLink, Eye, Link2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { Skill } from "@/tauri/commands";

import { CompatibilityBadge } from "./compatibility-badge";

interface Props {
  skill: Skill;
  /** Whether this skill is enabled (visible in slash popup +
   *  injected on send). Disabled skills stay rendered here so the
   *  user can flip them back on. */
  enabled: boolean;
  /** Toggle the enabled / disabled state. */
  onToggleEnabled: () => void;
  onView: () => void;
  onOpenFile: () => void;
}

/**
 * One row in the Settings → Skills list. Mirrors the row layout from
 * `permissions-section.tsx`: leading icon, name + description, hover-
 * reveal action buttons. Compatibility badges sit inline with the name
 * so users see at-a-glance which skills carry warnings without opening
 * the modal.
 */
export function SkillRow({
  skill,
  enabled,
  onToggleEnabled,
  onView,
  onOpenFile,
}: Props) {
  return (
    <div
      data-testid={`skill-row-${skill.id}`}
      data-enabled={enabled}
      className={cn(
        "group flex items-center gap-3 px-3 py-2 transition-colors",
        "hover:bg-accent/30",
        // Disabled skills render at half-opacity so users can see
        // they're still discovered but won't fire — the per-row
        // switch is the affordance to bring them back.
        !enabled && "opacity-50",
      )}
    >
      <BookOpen
        className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
        aria-hidden
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span
            className={cn(
              "truncate text-sm font-medium text-foreground",
              !enabled && "line-through decoration-muted-foreground/50",
            )}
          >
            {skill.name}
          </span>
          {!enabled && (
            <span
              data-testid="skill-row-disabled-badge"
              className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground"
            >
              Disabled
            </span>
          )}
          {skill.compatibility !== "compatible" && (
            <CompatibilityBadge level={skill.compatibility} />
          )}
          {skill.symlinked && (
            <Tooltip>
              <TooltipTrigger asChild>
                <span
                  data-testid="skill-row-symlink-icon"
                  className="inline-flex items-center text-muted-foreground/70 hover:text-muted-foreground"
                  aria-label="Symlinked skill"
                >
                  <Link2 className="h-3 w-3" aria-hidden />
                </span>
              </TooltipTrigger>
              <TooltipContent side="top">
                Resolved from{" "}
                <span className="font-mono text-[10px]">{skill.filePath}</span>
              </TooltipContent>
            </Tooltip>
          )}
        </div>
        {skill.description && (
          <p className="mt-0.5 truncate text-xs text-muted-foreground">
            {skill.description}
          </p>
        )}
      </div>
      <div className="flex items-center gap-2">
        <div
          className={cn(
            "flex items-center gap-1",
            "opacity-0 transition-opacity",
            "group-hover:opacity-100 focus-within:opacity-100",
          )}
        >
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs"
            onClick={onView}
          >
            <Eye className="mr-1 h-3 w-3" aria-hidden />
            View
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs"
            onClick={onOpenFile}
            aria-label={`Open ${skill.name} in editor`}
          >
            <ExternalLink className="h-3 w-3" aria-hidden />
          </Button>
        </div>
        {/* Switch sits outside the hover-reveal cluster — always
            visible so users know per-row enable/disable exists at
            a glance. Stops propagation so accidental clicks on the
            row don't fall through to the View action. */}
        <Switch
          checked={enabled}
          onCheckedChange={onToggleEnabled}
          aria-label={
            enabled ? `Disable ${skill.name}` : `Enable ${skill.name}`
          }
          data-testid={`skill-row-switch-${skill.id}`}
        />
      </div>
    </div>
  );
}
