import { toast } from "@/lib/toast";
import { copyMissingNestedWorktreeIncludes } from "@/tauri/commands";

export function formatIncludeDetails(items: string[]): string {
  const examples = items.slice(0, 3).join(", ");
  const remaining = items.length > 3 ? ` and ${items.length - 3} more` : "";
  return `${examples}${remaining}`;
}

export function reportWorktreeIncludeErrors(errors: string[]): void {
  if (errors.length === 0) return;
  toast.warning("Some worktree includes could not be copied", {
    description: formatIncludeDetails(errors),
    classNames: { description: "line-clamp-3 break-words" },
    duration: 8000,
  });
}

export async function copyMissingNestedFilesWithFeedback(workspaceId: string): Promise<void> {
  try {
    const result = await copyMissingNestedWorktreeIncludes(workspaceId);
    reportWorktreeIncludeErrors(result.errors);
    if (result.copied.length > 0) {
      toast.success("Copied missing nested worktree files", {
        description: formatIncludeDetails(result.copied),
        classNames: { description: "line-clamp-3 break-words" },
      });
    } else if (result.errors.length === 0) {
      toast.info("No missing nested worktree files to copy");
    }
  } catch (error) {
    toast.failure("Couldn't copy missing nested worktree files", error);
  }
}
