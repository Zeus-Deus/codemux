import { useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { FolderOpen, Loader2 } from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useProjectActions } from "@/hooks/use-project-actions";
import { pickFolderDialog } from "@/tauri/commands";

export function CloneDialog() {
  const open = useUIStore((s) => s.showCloneDialog);
  const setOpen = useUIStore((s) => s.setShowCloneDialog);
  const { cloneProject } = useProjectActions();

  const [url, setUrl] = useState("");
  const [targetDir, setTargetDir] = useState("");
  const [cloning, setCloning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleOpenChange = (open: boolean) => {
    if (!open) {
      setUrl("");
      setTargetDir("");
      setError(null);
      setCloning(false);
    }
    setOpen(open);
  };

  const repoName =
    url
      .replace(/\.git$/, "")
      .split("/")
      .pop() || "";

  const handleClone = async () => {
    if (!url.trim() || cloning) return;
    const dir = targetDir.trim() || repoName;
    if (!dir) return;

    setCloning(true);
    setError(null);
    try {
      await cloneProject(url.trim(), dir);
      handleOpenChange(false);
    } catch (err) {
      setError(String(err));
    } finally {
      setCloning(false);
    }
  };

  const handlePickDir = async () => {
    const folder = await pickFolderDialog("Clone destination");
    if (folder) setTargetDir(folder);
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="sm:max-w-[460px] bg-popover p-0 gap-0 overflow-hidden"
      >
        <DialogHeader className="px-5 pt-4 pb-2">
          <DialogTitle className="text-sm">Clone Repository</DialogTitle>
          <DialogDescription className="sr-only">
            Clone a git repository
          </DialogDescription>
        </DialogHeader>

        <div className="px-5 pb-4 space-y-3">
          <div className="space-y-1.5">
            <label className="text-xs text-muted-foreground">
              Repository URL
            </label>
            <Input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://github.com/user/repo.git"
              className="h-8 text-sm"
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter") handleClone();
              }}
            />
          </div>

          <div className="space-y-1.5">
            <label className="text-xs text-muted-foreground">Directory</label>
            <div className="flex gap-1.5">
              <Input
                value={targetDir}
                onChange={(e) => setTargetDir(e.target.value)}
                placeholder={repoName || "~/project-name"}
                className="h-8 text-sm flex-1 font-mono"
              />
              <Button
                variant="outline"
                size="sm"
                className="h-8 shrink-0"
                onClick={handlePickDir}
              >
                <FolderOpen className="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>

          {error && (
            <div className="p-2 rounded-md bg-destructive/10 border border-destructive/20 text-xs text-destructive">
              {error}
            </div>
          )}

          <div className="flex justify-end gap-2 pt-1">
            <Button
              variant="outline"
              size="sm"
              onClick={() => handleOpenChange(false)}
              disabled={cloning}
            >
              Cancel
            </Button>
            <Button
              size="sm"
              onClick={handleClone}
              disabled={!url.trim() || cloning}
              className="bg-foreground text-background hover:bg-foreground/90"
            >
              {cloning && <Loader2 className="h-3 w-3 animate-spin mr-1" />}
              {cloning ? "Cloning..." : "Clone"}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
