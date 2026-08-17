import { useEffect, useState } from "react";

import { Loader2, Wrench } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  listMcpToolsForServer,
  type McpServerConfig,
  type McpServerRuntime,
  type McpTool,
} from "@/tauri/commands";

interface Props {
  /** Server whose tools we want to display. `null` closes the modal. */
  server: McpServerConfig | null;
  /** Live runtime row for the server. Used to render an empty state
   *  when the server isn't running yet. */
  runtime: McpServerRuntime | null;
  onClose: () => void;
}

/**
 * Stage 4 — settings tool-list modal. Click a server row → see every
 * tool it registered with descriptions.
 *
 * Mirrors `SkillViewModal` in shape: shadcn Dialog with a header
 * describing the server, an overflow-scrolled body listing each
 * tool, and a small empty state when nothing is available.
 */
export function McpToolModal({ server, runtime, onClose }: Props) {
  const [tools, setTools] = useState<McpTool[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!server) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    listMcpToolsForServer(server.id)
      .then((result) => {
        if (cancelled) return;
        setTools(result);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(String(err));
      })
      .finally(() => {
        if (cancelled) return;
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [server]);

  const isRunning = runtime?.status.kind === "running";

  return (
    <Dialog open={!!server} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-3xl max-h-[80vh] overflow-hidden flex flex-col">
        {server && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <span>{server.name}</span>
                {isRunning && (
                  <span className="text-xs text-muted-foreground font-normal">
                    {runtime?.status.kind === "running"
                      ? `${runtime.status.toolCount} tool${
                          runtime.status.toolCount === 1 ? "" : "s"
                        }`
                      : ""}
                  </span>
                )}
              </DialogTitle>
              <DialogDescription className="font-mono text-xs">
                {server.command}{" "}
                {server.args.length > 0 ? server.args.join(" ") : ""}
              </DialogDescription>
            </DialogHeader>

            {loading ? (
              <div
                data-testid="mcp-tool-modal-loading"
                className="flex items-center gap-2 py-6 text-sm text-muted-foreground"
              >
                <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
                Loading tools…
              </div>
            ) : error ? (
              <p
                data-testid="mcp-tool-modal-error"
                className="rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive"
              >
                Failed to load tools: {error}
              </p>
            ) : tools.length === 0 ? (
              <p
                data-testid="mcp-tool-modal-empty"
                className="py-6 text-center text-sm text-muted-foreground"
              >
                {isRunning
                  ? "No tools exposed by this server."
                  : "Server not running — start it to see tools."}
              </p>
            ) : (
              <ul
                className="space-y-2 overflow-y-auto pr-1"
                data-testid="mcp-tool-modal-list"
              >
                {tools.map((tool) => (
                  <li key={tool.prefixedName}>
                    <ToolRow tool={tool} />
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function ToolRow({ tool }: { tool: McpTool }) {
  return (
    <div
      className="rounded-md border border-border/50 px-3 py-2"
      data-testid={`mcp-tool-${tool.prefixedName}`}
    >
      <div className="flex items-center gap-2">
        <Wrench className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        <code className="text-xs font-mono">{tool.prefixedName}</code>
      </div>
      {tool.description && (
        <p className="ml-5 mt-1 text-xs text-muted-foreground">
          {tool.description}
        </p>
      )}
    </div>
  );
}
