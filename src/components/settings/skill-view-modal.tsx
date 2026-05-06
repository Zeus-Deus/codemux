import { BookOpen, Link2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ChatMarkdown } from "@/components/chat/ChatMarkdown";
import { cn } from "@/lib/utils";
import type { Skill } from "@/tauri/commands";

import { CompatibilityBadge } from "./compatibility-badge";

interface Props {
  /** Skill to display, or `null` to keep the modal closed. */
  skill: Skill | null;
  onClose: () => void;
}

/**
 * Modal showing a skill's full SKILL.md body, parsed scope/provider
 * metadata, and any compatibility warnings. The body renders through
 * `ChatMarkdown` so headings/lists/code blocks look the same here as
 * they do in chat plan output.
 *
 * Rendering is gated on `skill !== null` rather than a boolean `open`
 * prop so the parent can drop the modal entirely when nothing is
 * selected — keeps the dialog tree out of the DOM when idle.
 */
export function SkillViewModal({ skill, onClose }: Props) {
  if (!skill) return null;

  const frontmatter = skill.rawFrontmatter as Record<string, unknown> | null;
  const hasFrontmatter =
    frontmatter !== null &&
    typeof frontmatter === "object" &&
    Object.keys(frontmatter).length > 0;

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        data-testid="skill-view-modal"
        // ~80% of viewport, capped at 1152px (72rem). Long pre blocks
        // (directory trees, bash tables) need real width — `max-w-3xl`
        // (768px) clipped them visibly.
        className="flex max-h-[85vh] w-[min(80vw,72rem)] max-w-none flex-col overflow-hidden sm:max-w-none"
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <BookOpen className="size-4" aria-hidden />
            {skill.name}
          </DialogTitle>
          <DialogDescription className="space-y-1">
            <span className="block">
              {skill.provider} · {skill.scope}
              {skill.pluginSlug ? ` · ${skill.pluginSlug}` : ""}
            </span>
            <span
              className="block break-all font-mono text-xs"
              data-testid="skill-modal-filepath"
            >
              {skill.filePath}
            </span>
            {skill.symlinked && (
              <span
                className="inline-flex items-center gap-1 text-xs text-muted-foreground/80"
                data-testid="skill-modal-symlink"
              >
                <Link2 className="h-3 w-3" aria-hidden />
                Resolved from a symlink — the path above is the link
                target, not where the entry lives in your skills folder.
              </span>
            )}
          </DialogDescription>
        </DialogHeader>

        {skill.compatibility !== "compatible" && (
          <div
            data-testid="skill-modal-compat-warning"
            className={cn(
              "rounded-md border p-3 text-xs",
              skill.compatibility === "soft-warn" &&
                "border-amber-500/30 bg-amber-500/5",
              skill.compatibility === "hard-warn" &&
                "border-destructive/30 bg-destructive/10",
            )}
          >
            <div className="mb-1 flex items-center gap-2">
              <CompatibilityBadge level={skill.compatibility} />
              <span className="font-medium text-foreground">
                {skill.compatibility === "soft-warn"
                  ? "May reference external tools"
                  : "May not work in current session"}
              </span>
            </div>
            {skill.compatibilitySignals.length > 0 && (
              <ul className="ml-1 list-inside list-disc text-muted-foreground">
                {skill.compatibilitySignals.map((sig) => (
                  <li key={sig}>{sig}</li>
                ))}
              </ul>
            )}
          </div>
        )}

        <div
          data-testid="skill-modal-body"
          className="flex-1 overflow-y-auto rounded-md border border-border/50 bg-muted/30 p-4"
        >
          {skill.body.trim().length === 0 ? (
            <p className="text-sm italic text-muted-foreground">
              This skill has no body content.
            </p>
          ) : (
            <ChatMarkdown>{skill.body}</ChatMarkdown>
          )}
        </div>

        {hasFrontmatter && (
          <details className="text-xs">
            <summary className="cursor-pointer text-muted-foreground hover:text-foreground">
              Advanced metadata
            </summary>
            <pre
              data-testid="skill-modal-frontmatter"
              className="mt-2 overflow-x-auto rounded-md bg-muted p-3 font-mono"
            >
              {JSON.stringify(frontmatter, null, 2)}
            </pre>
          </details>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
