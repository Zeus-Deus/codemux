import { useState } from "react";
import { cn } from "@/lib/utils";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  ArrowLeft,
  FolderPlus,
  GitBranch,
  FolderOpen,
  Loader2,
  X,
} from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import {
  pickFolderDialog,
  createEmptyRepo,
  gitCloneRepo,
  dbAddRecentProject,
  createEmptyWorkspace,
  activateWorkspace,
} from "@/tauri/commands";

type Mode = "empty" | "clone";

const MODE_OPTIONS: {
  mode: Mode;
  label: string;
  description: string;
  icon: typeof FolderPlus;
}[] = [
  {
    mode: "empty",
    label: "Empty",
    description: "New git repository from scratch",
    icon: FolderPlus,
  },
  {
    mode: "clone",
    label: "Clone",
    description: "Clone from a remote URL",
    icon: GitBranch,
  },
];

export function NewProjectScreen() {
  const setShowNewProjectScreen = useUIStore(
    (s) => s.setShowNewProjectScreen,
  );

  const [mode, setMode] = useState<Mode>("empty");
  const [parentDir, setParentDir] = useState("");
  const [repoName, setRepoName] = useState("");
  const [cloneUrl, setCloneUrl] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Auto-derive name from clone URL
  const derivedName =
    cloneUrl
      .replace(/\.git$/, "")
      .split("/")
      .pop()
      ?.replace(/[^a-zA-Z0-9._-]/g, "") || "";

  const effectiveName = mode === "clone" ? repoName || derivedName : repoName;

  const handleBrowse = async () => {
    const folder = await pickFolderDialog("Select project location");
    if (folder) setParentDir(folder);
  };

  const handleCreate = async () => {
    if (loading) return;

    if (!parentDir.trim()) {
      setError("Please select a project location");
      return;
    }

    setLoading(true);
    setError(null);

    try {
      let projectPath: string;

      if (mode === "empty") {
        const name = repoName.trim();
        if (!name) {
          setError("Please enter a repository name");
          setLoading(false);
          return;
        }
        projectPath = await createEmptyRepo(parentDir.trim(), name);
      } else {
        const url = cloneUrl.trim();
        if (!url) {
          setError("Please enter a repository URL");
          setLoading(false);
          return;
        }
        const targetDir = `${parentDir.trim()}/${effectiveName || derivedName}`;
        projectPath = await gitCloneRepo(url, targetDir);
      }

      const name =
        projectPath.split("/").filter(Boolean).pop() || projectPath;
      await dbAddRecentProject(projectPath, name);
      // Create a temporary workspace so the project appears in sidebar,
      // then show the onboarding wizard in the content area.
      const wsId = await createEmptyWorkspace(projectPath);
      await activateWorkspace(wsId);
      useUIStore.getState().setOnboardingProjectDir(projectPath);
      setShowNewProjectScreen(false);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-background">
      {/* Back button */}
      <div className="absolute top-4 left-4 z-10">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => setShowNewProjectScreen(false)}
        >
          <ArrowLeft className="h-4 w-4 mr-1" />
          Back
        </Button>
      </div>

      {/* Centered content */}
      <div className="flex flex-1 items-center justify-center">
        <div className="flex flex-col w-full max-w-xl px-6 gap-5">
          <h1 className="text-lg font-medium text-foreground">New Project</h1>

          {/* Location */}
          <div>
            <label className="block text-sm font-medium text-foreground mb-2">
              Location
            </label>
            <div className="flex gap-2">
              <Input
                value={parentDir}
                onChange={(e) => setParentDir(e.target.value)}
                placeholder="~/Projects"
                className="flex-1 font-mono text-xs"
                disabled={loading}
              />
              <Button
                variant="outline"
                size="icon"
                onClick={handleBrowse}
                disabled={loading}
                className="shrink-0"
              >
                <FolderOpen className="h-4 w-4" />
              </Button>
            </div>
          </div>

          {/* Mode selection cards */}
          <div className="grid grid-cols-2 gap-3">
            {MODE_OPTIONS.map((option) => {
              const selected = mode === option.mode;
              return (
                <button
                  key={option.mode}
                  type="button"
                  onClick={() => {
                    setMode(option.mode);
                    setError(null);
                  }}
                  className={cn(
                    "flex flex-col items-center gap-3 rounded-lg border p-4 pt-5 text-center transition-all",
                    selected
                      ? "border-foreground/50 bg-foreground/5"
                      : "border-border/50 hover:border-border hover:bg-accent/30",
                  )}
                >
                  <option.icon
                    className={cn(
                      "h-6 w-6",
                      selected ? "text-foreground" : "text-muted-foreground",
                    )}
                  />
                  <div>
                    <div className="text-sm font-medium text-foreground">
                      {option.label}
                    </div>
                    <div className="text-xs text-muted-foreground mt-0.5">
                      {option.description}
                    </div>
                  </div>
                </button>
              );
            })}
          </div>

          {/* Mode-specific fields */}
          {mode === "empty" && (
            <div>
              <label className="block text-sm font-medium text-foreground mb-2">
                Repository Name
              </label>
              <Input
                value={repoName}
                onChange={(e) => setRepoName(e.target.value)}
                placeholder="my-project"
                disabled={loading}
                autoFocus
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !loading) handleCreate();
                }}
              />
            </div>
          )}

          {mode === "clone" && (
            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium text-foreground mb-2">
                  Repository URL
                </label>
                <Input
                  value={cloneUrl}
                  onChange={(e) => setCloneUrl(e.target.value)}
                  placeholder="https:// or git@github.com:user/repo.git"
                  disabled={loading}
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !loading) handleCreate();
                  }}
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-foreground mb-2">
                  Repository Name
                </label>
                <Input
                  value={repoName}
                  onChange={(e) => setRepoName(e.target.value)}
                  placeholder={derivedName || "repo-name"}
                  disabled={loading}
                />
              </div>
            </div>
          )}

          {/* Error */}
          {error && (
            <div className="flex items-start gap-2 rounded-md px-4 py-3 bg-destructive/10 border border-destructive/20">
              <span className="flex-1 text-sm text-destructive">{error}</span>
              <button
                type="button"
                onClick={() => setError(null)}
                className="shrink-0 rounded p-0.5 text-destructive/70 hover:text-destructive transition-colors"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            </div>
          )}

          {/* Submit */}
          <div className="flex justify-end pt-2 border-t border-border/40">
            <Button onClick={handleCreate} disabled={loading} size="sm" className="bg-foreground text-background hover:bg-foreground/90">
              {loading && <Loader2 className="h-3 w-3 animate-spin mr-1" />}
              {loading
                ? mode === "clone"
                  ? "Cloning..."
                  : "Creating..."
                : "Create"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
