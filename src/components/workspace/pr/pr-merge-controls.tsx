import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { mergePullRequest } from "@/tauri/commands";
import type { PullRequestInfo } from "@/tauri/types";

interface Props {
  pr: PullRequestInfo;
  cwd: string;
  onRefresh: () => void;
}

export function PrMergeControls({ pr, cwd, onRefresh }: Props) {
  const [mergeMethod, setMergeMethod] = useState("squash");
  const [confirmMerge, setConfirmMerge] = useState(false);
  const [merging, setMerging] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (pr.state !== "OPEN") return null;

  const handleMerge = async () => {
    if (!confirmMerge) {
      setConfirmMerge(true);
      setTimeout(() => setConfirmMerge(false), 5000);
      return;
    }
    setMerging(true);
    setError(null);
    try {
      await mergePullRequest(cwd, pr.number, mergeMethod);
      onRefresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setMerging(false);
      setConfirmMerge(false);
    }
  };

  return (
    <div className="space-y-1.5 px-1.5">
      <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground px-1">
        Merge
      </span>
      <div className="flex gap-1">
        <Select value={mergeMethod} onValueChange={setMergeMethod}>
          <SelectTrigger className="h-7 text-xs flex-1">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="squash" className="text-xs">
              Squash and merge
            </SelectItem>
            <SelectItem value="merge" className="text-xs">
              Create merge commit
            </SelectItem>
            <SelectItem value="rebase" className="text-xs">
              Rebase and merge
            </SelectItem>
          </SelectContent>
        </Select>
        <Button
          size="xs"
          className="text-xs h-7"
          variant={confirmMerge ? "destructive" : "default"}
          disabled={merging || pr.mergeable === "CONFLICTING"}
          onClick={handleMerge}
        >
          {merging
            ? "Merging..."
            : confirmMerge
              ? "Confirm"
              : "Merge"}
        </Button>
      </div>
      {pr.mergeable === "CONFLICTING" && (
        <p className="text-[10px] text-danger px-1">Has merge conflicts</p>
      )}
      {error && (
        <p className="text-xs text-danger break-words px-1">{error}</p>
      )}
    </div>
  );
}
