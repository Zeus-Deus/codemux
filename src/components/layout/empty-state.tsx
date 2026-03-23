import { Terminal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { createWorkspace } from "@/tauri/commands";

export function EmptyState() {
  const handleCreate = () => {
    createWorkspace(null).catch(console.error);
  };

  return (
    <div className="flex h-full items-center justify-center">
      <div className="text-center space-y-4">
        <Terminal className="h-12 w-12 mx-auto text-muted-foreground/30" />
        <h2 className="text-lg font-medium text-foreground">No workspace open</h2>
        <p className="text-sm text-muted-foreground">
          Create a workspace to get started.
        </p>
        <Button variant="outline" onClick={handleCreate}>
          Create workspace
        </Button>
      </div>
    </div>
  );
}
