import { useState, useEffect, useCallback } from "react";
import { useOpenFlowStore } from "@/stores/openflow-store";
import type {
  AgentConfig,
  CliToolInfo,
  ModelInfo,
  OpenFlowRole,
  ThinkingModeInfo,
} from "@/tauri/types";
import {
  listAvailableCliTools,
  listModelsForTool,
  listThinkingModesForTool,
  createOpenflowRun,
  spawnOpenflowAgents,
  activateWorkspace,
  pickFolderDialog,
} from "@/tauri/commands";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { FolderOpen, Play } from "lucide-react";

const ROLES: OpenFlowRole[] = [
  "orchestrator",
  "planner",
  "builder",
  "reviewer",
  "tester",
  "debugger",
  "researcher",
];

interface AgentRow {
  cliTool: string;
  model: string;
  provider: string;
  role: OpenFlowRole;
  thinkingMode: string;
}

function defaultAgentRows(count: number): AgentRow[] {
  return Array.from({ length: count }, (_, i) => ({
    cliTool: "",
    model: "",
    provider: "",
    role: i === 0 ? "orchestrator" : "builder",
    thinkingMode: "auto",
  }));
}

interface NewRunDialogProps {
  defaultCwd?: string;
}

export function NewRunDialog({ defaultCwd }: NewRunDialogProps) {
  const open = useOpenFlowStore((s) => s.newRunDialogOpen);
  const setOpen = useOpenFlowStore((s) => s.setNewRunDialogOpen);
  const syncRuntime = useOpenFlowStore((s) => s.syncRuntime);
  const setActiveRun = useOpenFlowStore((s) => s.setActiveRun);

  const [title, setTitle] = useState("OpenFlow Run");
  const [goal, setGoal] = useState("");
  const [cwd, setCwd] = useState(defaultCwd ?? "");
  const [agentCount, setAgentCount] = useState(3);
  const [agents, setAgents] = useState<AgentRow[]>(defaultAgentRows(3));
  const [submitting, setSubmitting] = useState(false);

  // Discovery state
  const [cliTools, setCliTools] = useState<CliToolInfo[]>([]);
  const [models, setModels] = useState<Record<string, ModelInfo[]>>({});
  const [thinkingModes, setThinkingModes] = useState<
    Record<string, ThinkingModeInfo[]>
  >({});

  // Discover available tools on mount
  useEffect(() => {
    if (!open) return;
    listAvailableCliTools().then((tools) => {
      setCliTools(tools);
      // Pre-fetch models and thinking modes for each available tool
      for (const tool of tools.filter((t) => t.available)) {
        listModelsForTool(tool.id).then((m) =>
          setModels((prev) => ({ ...prev, [tool.id]: m })),
        );
        listThinkingModesForTool(tool.id).then((tm) =>
          setThinkingModes((prev) => ({ ...prev, [tool.id]: tm })),
        );
      }

      // Default first available tool for all agents
      const firstTool = tools.find((t) => t.available);
      if (firstTool) {
        setAgents((prev) =>
          prev.map((a) => (a.cliTool ? a : { ...a, cliTool: firstTool.id })),
        );
      }
    });
  }, [open]);

  // Set default model when tool models load
  useEffect(() => {
    setAgents((prev) =>
      prev.map((a) => {
        if (a.model || !a.cliTool) return a;
        const toolModels = models[a.cliTool];
        if (!toolModels || toolModels.length === 0) return a;
        return {
          ...a,
          model: toolModels[0].id,
          provider: toolModels[0].provider ?? "",
        };
      }),
    );
  }, [models]);

  // Sync agent count
  useEffect(() => {
    setAgents((prev) => {
      if (prev.length === agentCount) return prev;
      if (prev.length < agentCount) {
        const firstTool = cliTools.find((t) => t.available);
        const extra = Array.from(
          { length: agentCount - prev.length },
          () =>
            ({
              cliTool: firstTool?.id ?? "",
              model: "",
              provider: "",
              role: "builder" as OpenFlowRole,
              thinkingMode: "auto",
            }),
        );
        return [...prev, ...extra];
      }
      return prev.slice(0, agentCount);
    });
  }, [agentCount, cliTools]);

  const updateAgent = useCallback(
    (index: number, patch: Partial<AgentRow>) => {
      setAgents((prev) => {
        const next = [...prev];
        next[index] = { ...next[index], ...patch };
        return next;
      });
    },
    [],
  );

  const handlePickFolder = async () => {
    const folder = await pickFolderDialog("Select project directory");
    if (folder) setCwd(folder);
  };

  const handleSubmit = async () => {
    if (!goal.trim() || submitting) return;
    setSubmitting(true);

    try {
      const agentRoles = agents.map((a) => a.role);
      const run = await createOpenflowRun({
        title,
        goal,
        agent_roles: agentRoles,
      });

      const configs: AgentConfig[] = agents.map((a, i) => ({
        agent_index: i,
        cli_tool: a.cliTool,
        model: a.model,
        provider: a.provider,
        thinking_mode: a.thinkingMode,
        role: a.role,
      }));

      // The workspace is created by createOpenflowRun on the backend
      // We need the workspace_id — sync runtime to discover it
      await syncRuntime();

      // Use workspace from app state since the run was just created
      const { useAppStore } = await import("@/stores/app-store");
      const appState = useAppStore.getState().appState;
      const openflowWs = appState?.workspaces.find(
        (w) => w.workspace_type === "open_flow",
      );

      if (openflowWs) {
        await spawnOpenflowAgents(
          openflowWs.workspace_id,
          run.run_id,
          goal,
          cwd,
          configs,
        );
        await activateWorkspace(openflowWs.workspace_id);
      }

      setActiveRun(run.run_id);
      await syncRuntime();
      setOpen(false);
    } catch (err) {
      console.error("Failed to start OpenFlow run:", err);
    } finally {
      setSubmitting(false);
    }
  };

  const availableTools = cliTools.filter((t) => t.available);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="max-w-2xl max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle className="text-sm">New OpenFlow Run</DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          {/* Title */}
          <div className="space-y-1.5">
            <Label className="text-xs">Title</Label>
            <Input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              className="h-8 text-xs"
              placeholder="Run title"
            />
          </div>

          {/* Goal */}
          <div className="space-y-1.5">
            <Label className="text-xs">Goal</Label>
            <textarea
              value={goal}
              onChange={(e) => setGoal(e.target.value)}
              className="flex min-h-[80px] w-full rounded-md border border-input bg-transparent px-3 py-2 text-xs placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring resize-y"
              placeholder="Describe what the agents should accomplish..."
            />
          </div>

          {/* Directory */}
          <div className="space-y-1.5">
            <Label className="text-xs">Project Directory</Label>
            <div className="flex gap-1.5">
              <Input
                value={cwd}
                onChange={(e) => setCwd(e.target.value)}
                className="h-8 text-xs flex-1"
                placeholder="/path/to/project"
              />
              <Button
                variant="outline"
                size="sm"
                className="h-8 shrink-0"
                onClick={handlePickFolder}
              >
                <FolderOpen className="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>

          {/* Agent count */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <Label className="text-xs">Agents</Label>
              <span className="text-xs text-muted-foreground tabular-nums">
                {agentCount}
              </span>
            </div>
            <Slider
              value={[agentCount]}
              onValueChange={([v]) => setAgentCount(v)}
              min={2}
              max={20}
              step={1}
              className="w-full"
            />
          </div>

          {/* Agent config table */}
          <div className="border rounded-md overflow-hidden">
            <div className="grid grid-cols-[36px_1fr_1fr_1fr_1fr] gap-px bg-border text-[10px] font-semibold text-muted-foreground uppercase tracking-wider">
              <div className="bg-card px-2 py-1.5">#</div>
              <div className="bg-card px-2 py-1.5">Tool</div>
              <div className="bg-card px-2 py-1.5">Model</div>
              <div className="bg-card px-2 py-1.5">Role</div>
              <div className="bg-card px-2 py-1.5">Thinking</div>
            </div>
            {agents.map((agent, i) => (
              <div
                key={i}
                className="grid grid-cols-[36px_1fr_1fr_1fr_1fr] gap-px bg-border"
              >
                <div className="bg-card flex items-center justify-center text-xs text-muted-foreground">
                  {i}
                </div>
                <div className="bg-card p-1">
                  <Select
                    value={agent.cliTool}
                    onValueChange={(v) => {
                      updateAgent(i, { cliTool: v, model: "", provider: "" });
                    }}
                  >
                    <SelectTrigger className="h-7 text-[11px]">
                      <SelectValue placeholder="Tool" />
                    </SelectTrigger>
                    <SelectContent>
                      {availableTools.map((t) => (
                        <SelectItem key={t.id} value={t.id} className="text-xs">
                          {t.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="bg-card p-1">
                  <Select
                    value={agent.model}
                    onValueChange={(v) => {
                      const m = models[agent.cliTool]?.find((m) => m.id === v);
                      updateAgent(i, {
                        model: v,
                        provider: m?.provider ?? "",
                      });
                    }}
                  >
                    <SelectTrigger className="h-7 text-[11px]">
                      <SelectValue placeholder="Model" />
                    </SelectTrigger>
                    <SelectContent>
                      {(models[agent.cliTool] ?? []).map((m) => (
                        <SelectItem key={m.id} value={m.id} className="text-xs">
                          {m.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="bg-card p-1">
                  <Select
                    value={agent.role}
                    onValueChange={(v) =>
                      updateAgent(i, { role: v as OpenFlowRole })
                    }
                  >
                    <SelectTrigger className="h-7 text-[11px]">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {ROLES.map((r) => (
                        <SelectItem key={r} value={r} className="text-xs capitalize">
                          {r}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="bg-card p-1">
                  <Select
                    value={agent.thinkingMode}
                    onValueChange={(v) => updateAgent(i, { thinkingMode: v })}
                  >
                    <SelectTrigger className="h-7 text-[11px]">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {(thinkingModes[agent.cliTool] ?? [{ id: "auto", name: "Auto", description: "" }]).map(
                        (tm) => (
                          <SelectItem
                            key={tm.id}
                            value={tm.id}
                            className="text-xs"
                          >
                            {tm.name}
                          </SelectItem>
                        ),
                      )}
                    </SelectContent>
                  </Select>
                </div>
              </div>
            ))}
          </div>

          {/* Submit */}
          <Button
            className="w-full h-8 text-xs"
            onClick={handleSubmit}
            disabled={!goal.trim() || !cwd.trim() || submitting}
          >
            <Play className="h-3.5 w-3.5 mr-1.5" />
            {submitting ? "Starting..." : "Start Orchestration"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
