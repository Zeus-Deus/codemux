import { BookOpen, ExternalLink, Eye } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { Skill } from "@/tauri/commands";

import { CompatibilityBadge } from "./compatibility-badge";

interface Props {
  skill: Skill;
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
export function SkillRow({ skill, onView, onOpenFile }: Props) {
  return (
    <div
      data-testid={`skill-row-${skill.id}`}
      className={cn(
        "group flex items-center gap-3 px-3 py-2 transition-colors",
        "hover:bg-accent/30",
      )}
    >
      <BookOpen
        className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
        aria-hidden
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-medium text-foreground">
            {skill.name}
          </span>
          {skill.compatibility !== "compatible" && (
            <CompatibilityBadge level={skill.compatibility} />
          )}
          {skill.symlinked && (
            <span
              className="text-[10px] text-muted-foreground/70"
              title={`Symlinked from ${skill.filePath}`}
            >
              symlinked
            </span>
          )}
        </div>
        {skill.description && (
          <p className="mt-0.5 truncate text-xs text-muted-foreground">
            {skill.description}
          </p>
        )}
      </div>
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
    </div>
  );
}
