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
  createOpenflowWorkspace,
  createOpenflowRun,
  spawnOpenflowAgents,
  activateWorkspace,
  pickFolderDialog,
} from "@/tauri/commands";
import {
  Dialog,
  DialogContent,
  DialogDescription,
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
    const folder = await pickFolderDialog("Choose folder");
    if (folder) setCwd(folder);
  };

  const handleSubmit = async () => {
    if (!goal.trim() || submitting) return;
    setSubmitting(true);

    try {
      // 1. Create the OpenFlow workspace (returns workspace_id)
      const workspaceId = await createOpenflowWorkspace(title, goal, cwd || null);

      // 2. Create the run record
      const agentRoles = agents.map((a) => a.role);
      const run = await createOpenflowRun({
        title,
        goal,
        agent_roles: agentRoles,
      });

      // 3. Build agent configs and spawn agents
      const configs: AgentConfig[] = agents.map((a, i) => ({
        agent_index: i,
        cli_tool: a.cliTool,
        model: a.model,
        provider: a.provider,
        thinking_mode: a.thinkingMode,
        role: a.role,
      }));

      await spawnOpenflowAgents(workspaceId, run.run_id, goal, cwd, configs);

      // 4. Activate the workspace and update state
      await activateWorkspace(workspaceId);
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
      <DialogContent className="max-w-4xl sm:max-w-4xl w-full max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle className="text-sm">New OpenFlow Run</DialogTitle>
          <DialogDescription className="sr-only">Configure and start a multi-agent orchestration run</DialogDescription>
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
          <table className="w-full border-collapse border rounded-md text-xs">
            <thead>
              <tr className="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider">
                <th className="w-10 px-2 py-1.5 text-left border-b">#</th>
                <th className="w-1/4 px-2 py-1.5 text-left border-b">Tool</th>
                <th className="w-1/4 px-2 py-1.5 text-left border-b">Model</th>
                <th className="w-1/4 px-2 py-1.5 text-left border-b">Role</th>
                <th className="px-2 py-1.5 text-left border-b">Thinking</th>
              </tr>
            </thead>
            <tbody>
              {agents.map((agent, i) => (
                <tr key={i} className="border-b border-border/50 last:border-b-0">
                  <td className="px-2 py-1 text-center text-muted-foreground">{i}</td>
                  <td className="px-1 py-1">
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
                  </td>
                  <td className="px-1 py-1">
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
                  </td>
                  <td className="px-1 py-1">
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
                  </td>
                  <td className="px-1 py-1">
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
                  </td>
                </tr>
              ))}
            </tbody>
          </table>

        </div>

        {/* Sticky submit footer */}
        <div className="sticky bottom-0 -mx-4 -mb-4 border-t bg-popover p-4">
          <Button
            className="w-full h-8 text-xs bg-foreground text-background hover:bg-foreground/90"
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
