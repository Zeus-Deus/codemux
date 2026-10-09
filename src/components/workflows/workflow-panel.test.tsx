/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { DEFAULT_WORKFLOW_LIMITS, type WorkflowRunSnapshot, type WorkflowTaskSnapshot } from "@/tauri/workflows";
import { useWorkflowUIStore } from "@/stores/workflow-ui-store";
import { useWorkflowInspectorStore } from "@/stores/workflow-inspector-store";

const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  list: vi.fn(), get: vi.fn(), capabilities: vi.fn(), scripts: vi.fn(), launch: vi.fn(), save: vi.fn(),
  pause: vi.fn(), resume: vi.fn(), cancel: vi.fn(), retry: vi.fn(), retire: vi.fn(), message: vi.fn(), cancelTask: vi.fn(), integrate: vi.fn(), reconcile: vi.fn(), preview: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@/tauri/workflows", async (original) => ({
  ...await original<typeof import("@/tauri/workflows")>(),
  workflowList: mocks.list, workflowGet: mocks.get, workflowCapabilities: mocks.capabilities, workflowScriptsList: mocks.scripts,
  workflowScriptRun: mocks.launch, workflowScriptSave: mocks.save, workflowPause: mocks.pause, workflowResume: mocks.resume,
  workflowCancel: mocks.cancel, workflowRetry: mocks.retry, workflowRetire: mocks.retire, workflowMessage: mocks.message,
  workflowReconcile: mocks.reconcile, workflowCancelTask: mocks.cancelTask, workflowIntegrateArtifact: mocks.integrate, workflowArtifactPreview: mocks.preview,
}));
import { WorkflowPanel, WORKFLOW_STARTER } from "./workflow-panel";

let changeListener: ((event: { payload: { run_id: string; revision: number; kind: string } }) => void) | undefined;
const zeroUsage = { input_tokens: 0, output_tokens: 0, total_tokens: 0, reserved_tokens: 0, estimated_tokens: 0, tokens_unknown: false, cost_usd: 0, cost_unknown: false };
function task(status: WorkflowTaskSnapshot["status"] = "queued"): WorkflowTaskSnapshot {
  return { retired: false, spec: { id: "inspect", title: "Inspect fixture", prompt: "Read the synthetic fixture", route_id: "codex", access: "read_only", dependencies: [], scope: [], required: true }, generation: 1, status, depth: 0, parent_task_id: null, current_attempt: null, attempts: [], result: null, error: null, waiting_for: [], messages: [] };
}
function run(tasks: WorkflowTaskSnapshot[] = [task()]): WorkflowRunSnapshot {
  return { id: "fixture-run", status: "running", cancel_requested: false, pause_requested: false, error: null, revision: 4, created_at_ms: 1, updated_at_ms: 2,
    spec: { workspace_id: "fixture-workspace", title: "Fixture review", goal: "Review fixtures", mode: "dry_run", allow_writes: false, routes: [{ id: "codex", provider: "codex", model: "fixture-model" }], limits: DEFAULT_WORKFLOW_LIMITS, tasks: [] },
    tasks, usage: zeroUsage, resolved_limits: { ...DEFAULT_WORKFLOW_LIMITS, concurrency: 4 }, script: { status: "running", phase: "Inspect", result: null, error: null } };
}
function retainedRun(): WorkflowRunSnapshot {
  const completed = task("succeeded");
  completed.spec.access = "write"; completed.spec.scope = ["src"];
  completed.attempts = [{ id: "attempt", generation: 1, operation_id: "operation", status: "succeeded", route_id: "codex", started_at_ms: 1, finished_at_ms: 2, cancel_requested: false, reserved_tokens: 0, external_ref: null, output: null, error: null, usage: zeroUsage, artifacts: [{ kind: "workspace_snapshot", run_id: "fixture-run", attempt_id: "attempt", digest: "sealed-digest", baseline_digest: "baseline", changed_paths: ["src/fixture.ts", "src/deleted.ts"] }] }];
  const snapshot = { ...run([completed]), status: "completed" as const };
  snapshot.spec.mode = "live"; snapshot.spec.allow_writes = true;
  return snapshot;
}
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={client}><WorkflowPanel workspaceId="fixture-workspace" /></QueryClientProvider>);
}
function openSection(testId: string) {
  const details = screen.getByTestId(testId);
  if (!details.hasAttribute("open")) fireEvent.click(within(details).getByText((_text, element) => element?.tagName === "SUMMARY" && element.parentElement === details));
}
async function selectTask(id = "inspect") {
  if (!screen.queryByTestId(`workflow-task-${id}`)) openSection("workflow-settled");
  fireEvent.click(await screen.findByTestId(`workflow-task-${id}`));
}
beforeEach(() => {
  vi.clearAllMocks();
  changeListener = undefined;
  mocks.listen.mockImplementation((_name: string, listener: typeof changeListener) => { changeListener = listener; return Promise.resolve(() => {}); });
  useWorkflowUIStore.setState({ drafts: {}, selectedRuns: {} });
  useWorkflowInspectorStore.setState({ runs: {} });
  mocks.list.mockResolvedValue([]); mocks.get.mockResolvedValue(run()); mocks.scripts.mockResolvedValue([]);
  mocks.capabilities.mockResolvedValue(["claude", "codex"].map((provider) => ({ provider, live: true, read_only: true, write: true, reason: null })));
  mocks.launch.mockResolvedValue(run()); mocks.pause.mockResolvedValue({ ...run(), status: "paused" });
  mocks.resume.mockResolvedValue(run()); mocks.cancel.mockResolvedValue({ ...run(), status: "cancelled" });
  mocks.save.mockResolvedValue({ id: "script", workspace_id: "fixture-workspace", title: "Workspace review", source: WORKFLOW_STARTER, updated_at_ms: 3 });
  mocks.preview.mockResolvedValue({ path: "src/fixture.ts", before: "old fixture", after: "new fixture", binary: false, truncated: false, before_sha256: null, after_sha256: null });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("dynamic workflow panel", () => {
  it("launches an executable mixed-provider script with auto concurrency and no write permission", async () => {
    mount();
    openSection("workflow-script-settings");
    openSection("workflow-limit-settings");
    openSection("workflow-route-settings");
    expect(screen.getByLabelText("Workflow JavaScript")).toHaveValue(WORKFLOW_STARTER);
    expect(screen.getByLabelText("Worker ceiling")).toHaveValue("0");
    fireEvent.change(screen.getByLabelText("Codex model"), { target: { value: "custom-model" } });
    fireEvent.click(screen.getByTestId("workflow-dry-run"));
    await waitFor(() => expect(mocks.launch).toHaveBeenCalledOnce());
    expect(mocks.launch.mock.calls[0][0]).toMatchObject({ mode: "dry_run", allow_writes: false, limits: { concurrency: 0 }, routes: [{ provider: "claude" }, { provider: "codex", model: "custom-model" }], tasks: [] });
    expect(mocks.launch.mock.calls[0][1]).toBe(WORKFLOW_STARTER);
    await screen.findByTestId("workflow-run-inspector");
  });

  it("exposes unsupported routes without enabling live execution", async () => {
    mocks.capabilities.mockResolvedValue([{ provider: "claude", live: true, read_only: true, write: true, reason: null }, { provider: "codex", live: false, read_only: false, write: false, reason: "Adapter unavailable" }]);
    mount();
    await screen.findByText("Adapter unavailable");
    expect(screen.getByTestId("workflow-live-run")).toBeDisabled();
    expect(screen.getByTestId("workflow-dry-run")).toBeEnabled();
  });

  it.each(["opencode", "review-route"])("requires a model on OpenCode route %s before live execution without restricting dry runs", async (routeId) => {
    mocks.capabilities.mockResolvedValue([{ provider: "opencode", live: true, read_only: true, write: true, requires_model: true, effort_supported: true, reason: null }]);
    useWorkflowUIStore.getState().setDraft("fixture-workspace", { title: "OpenCode review", goal: "Review the fixture", source: WORKFLOW_STARTER, allowWrites: false, limits: DEFAULT_WORKFLOW_LIMITS, routes: [{ id: routeId, provider: "opencode" }] });
    mount();
    await screen.findByText("OpenCode model required");
    expect(screen.getByTestId("workflow-route-settings")).not.toHaveAttribute("open");
    expect(screen.getByTestId("workflow-live-run")).toBeDisabled();
    expect(screen.getByTestId("workflow-dry-run")).toBeEnabled();
    fireEvent.click(screen.getByTestId("workflow-live-run"));
    expect(mocks.launch).not.toHaveBeenCalled();
    openSection("workflow-route-settings");
    const model = screen.getByTestId(`workflow-model-${routeId}`);
    expect(model).toHaveAttribute("placeholder", "provider/model");
    expect(model).toHaveAccessibleDescription("Model required for live execution. Use provider/model.");
    fireEvent.change(model, { target: { value: "   " } });
    expect(screen.getByTestId("workflow-live-run")).toBeDisabled();
    for (const value of ["model-only", "provider/", "bad provider/model", "provider/model name"]) {
      fireEvent.change(model, { target: { value } });
      expect(model).toHaveAttribute("aria-invalid", "true");
      expect(screen.getByTestId("workflow-live-run")).toBeDisabled();
    }
    fireEvent.change(model, { target: { value: "  fixture-provider/fixture-model  " } });
    expect(model).toHaveValue("  fixture-provider/fixture-model  ");
    expect(model).not.toHaveAttribute("aria-invalid");
    expect(screen.getByTestId("workflow-route-settings")).toHaveAttribute("open");
    expect(screen.getByTestId("workflow-live-run")).toBeEnabled();
    fireEvent.click(screen.getByTestId("workflow-live-run"));
    await waitFor(() => expect(mocks.launch).toHaveBeenCalledOnce());
    expect(mocks.launch.mock.calls[0][0]).toMatchObject({ mode: "live", routes: [{ id: routeId, provider: "opencode", model: "fixture-provider/fixture-model" }] });
  });

  it("allows clearing a restored unsupported effort before starting a Cursor route", async () => {
    mocks.capabilities.mockResolvedValue([{ provider: "cursor", live: true, read_only: true, write: true, requires_model: false, effort_supported: false, reason: null }]);
    useWorkflowUIStore.getState().setDraft("fixture-workspace", { title: "Cursor review", goal: "Review the fixture", source: WORKFLOW_STARTER, allowWrites: false, limits: DEFAULT_WORKFLOW_LIMITS, routes: [{ id: "cursor", provider: "cursor", effort: "high" }] });
    mount();
    await screen.findByText("Cursor uses its default effort. Leave effort empty.");
    expect(screen.getByTestId("workflow-live-run")).toBeDisabled();
    expect(screen.getByTestId("workflow-dry-run")).toBeEnabled();
    openSection("workflow-route-settings");
    const effort = screen.getByLabelText("Cursor effort");
    expect(effort).toBeEnabled();
    expect(effort).toHaveValue("high");
    fireEvent.change(effort, { target: { value: "   " } });
    expect(effort).toHaveValue("   ");
    expect(screen.getByTestId("workflow-route-settings")).toHaveAttribute("open");
    expect(screen.getByTestId("workflow-live-run")).toBeEnabled();
    fireEvent.click(screen.getByTestId("workflow-live-run"));
    await waitFor(() => expect(mocks.launch).toHaveBeenCalledOnce());
    expect(mocks.launch.mock.calls[0][0]).toMatchObject({ mode: "live", routes: [{ provider: "cursor", effort: null }] });
  });

  it("adds a second model route and persists the editable script draft", async () => {
    mount();
    openSection("workflow-route-settings");
    openSection("workflow-script-settings");
    fireEvent.click(screen.getByTestId("workflow-add-route"));
    fireEvent.change(screen.getByLabelText("Model for route-1"), { target: { value: "small-model" } });
    fireEvent.change(screen.getByLabelText("Workflow JavaScript"), { target: { value: "return await agent(goal);" } });
    expect(useWorkflowUIStore.getState().drafts["fixture-workspace"].routes).toHaveLength(3);
    expect(useWorkflowUIStore.getState().drafts["fixture-workspace"].routes[2].model).toBe("small-model");
    fireEvent.click(screen.getByTestId("workflow-save-script"));
    await waitFor(() => expect(mocks.save).toHaveBeenCalledWith("fixture-workspace", "Workspace review", "return await agent(goal);"));
  });

  it("pauses a run through IPC and exposes its concrete task result", async () => {
    const ready = task("succeeded"); ready.result = { findings: ["Synthetic finding"] };
    const snapshot = run([ready]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    mocks.pause.mockResolvedValue({ ...snapshot, status: "paused", revision: 5 });
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    fireEvent.click(await screen.findByTestId("workflow-pause"));
    await waitFor(() => expect(mocks.pause).toHaveBeenCalledWith(snapshot.id));
    await selectTask();
    await screen.findByTestId("workflow-task-detail");
    expect(screen.getByTestId("workflow-task-result")).toHaveTextContent("Synthetic finding");
  });

  it("does not offer retry for an unknown execution outcome", async () => {
    const unknown = task("unknown"); unknown.error = "Connection closed before execution was confirmed";
    const snapshot = { ...run([unknown]), status: "unknown" as const };
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    await selectTask();
    expect(screen.queryByTestId("workflow-retry")).not.toBeInTheDocument();
    expect(screen.getByTestId("workflow-cancel-task")).toBeEnabled();
    expect(screen.getByTestId("workflow-reconcile")).toBeEnabled();
    expect(screen.getByRole("alert")).toHaveTextContent("Connection closed");
  });

  it("separates observed tokens from estimates and flags incomplete accounting", async () => {
    const snapshot = { ...run([task("succeeded")]), status: "completed" as const, usage: { ...zeroUsage, total_tokens: 50, estimated_tokens: 4000, tokens_unknown: true } };
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    openSection("workflow-run-settings");
    expect(screen.getByText("Observed tokens")).toBeInTheDocument();
    expect(screen.getByTestId("workflow-usage-estimate")).toHaveTextContent("4,000 estimated tokens");
    expect(screen.getByTestId("workflow-usage-estimate")).toHaveTextContent("observed usage may be partial");
  });

  it("coalesces workflow events into one selected snapshot refresh", async () => {
    const snapshot = { ...run([task("succeeded")]), status: "completed" as const };
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    await waitFor(() => expect(changeListener).toBeDefined());
    mocks.get.mockResolvedValue({ ...snapshot, revision: 10 });
    for (let revision = 5; revision <= 10; revision++) changeListener!({ payload: { run_id: snapshot.id, revision, kind: "finished" } });
    await waitFor(() => expect(mocks.get).toHaveBeenCalledTimes(2));
  });

  it("lazily reviews retained before/after files before applying the sealed current artifact", async () => {
    const snapshot = retainedRun();
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]); mocks.integrate.mockResolvedValue({ attempt_id: "attempt", digest: "sealed-digest", changed_paths: ["src/fixture.ts", "src/deleted.ts"] });
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    await selectTask();
    expect(mocks.preview).not.toHaveBeenCalled();
    expect(screen.getByTestId("workflow-apply-artifact")).toBeDisabled();
    expect(screen.getByTestId("workflow-apply-artifact")).toHaveTextContent("Apply 2 file changes");
    fireEvent.click(screen.getByTestId("workflow-review-artifact"));
    await screen.findByText("old fixture");
    expect(screen.getByText("new fixture")).toBeInTheDocument();
    expect(mocks.preview).toHaveBeenCalledWith(snapshot.id, "inspect", "attempt", "sealed-digest", "src/fixture.ts");
    mocks.preview.mockResolvedValue({ path: "src/deleted.ts", before: "retired fixture", after: null, binary: false, truncated: true, before_sha256: "sealed-before-hash", after_sha256: null });
    fireEvent.change(screen.getByLabelText("Review changed file"), { target: { value: "src/deleted.ts" } });
    await screen.findByText("File deleted.");
    expect(screen.getByText(/64 KiB per side/)).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("workflow-apply-artifact"));
    await waitFor(() => expect(mocks.integrate).toHaveBeenCalledWith(snapshot.id, "inspect", "attempt", "sealed-digest"));
    await screen.findByText("Applied 2 file changes.");
  });

  it.each(["retired", "older generation"])("preserves %s artifact history without allowing application", async (state) => {
    const snapshot = retainedRun();
    if (state === "retired") snapshot.tasks[0].retired = true;
    else snapshot.tasks[0].generation = 2;
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    await selectTask();
    openSection("workflow-earlier-artifacts");
    fireEvent.click(await screen.findByTestId("workflow-review-artifact"));
    await screen.findByText("new fixture");
    expect(screen.queryByTestId("workflow-apply-artifact")).not.toBeInTheDocument();
    if (state === "older generation") expect(screen.getByText("Earlier attempt · review only. These changes cannot be applied.")).toBeInTheDocument();
    expect(mocks.integrate).not.toHaveBeenCalled();
  });

  it.each(["cancelled", "failed", "blocked"] as const)("keeps %s task and file evidence inspectable after run cancellation without offering retry or apply", async (status) => {
    const snapshot = retainedRun();
    snapshot.status = "cancelled"; snapshot.cancel_requested = true;
    const stopped = task(status);
    stopped.spec.id = "stopped"; stopped.spec.title = "Stopped task";
    snapshot.tasks.push(stopped);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    await selectTask("stopped");
    expect(screen.queryByTestId("workflow-retry")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back to checkpoints" }));
    await selectTask();
    fireEvent.click(screen.getByTestId("workflow-review-artifact"));
    await screen.findByText("new fixture");
    expect(screen.queryByTestId("workflow-apply-artifact")).not.toBeInTheDocument();
    expect(mocks.retry).not.toHaveBeenCalled();
    expect(mocks.integrate).not.toHaveBeenCalled();
  });

  it("keeps advanced setup and completed evidence folded until requested", async () => {
    mount();
    expect(screen.getByTestId("workflow-script-settings")).not.toHaveAttribute("open");
    expect(screen.getByTestId("workflow-limit-settings")).not.toHaveAttribute("open");
    expect(screen.getByTestId("workflow-route-settings")).not.toHaveAttribute("open");
    expect(screen.getByLabelText("Workflow goal")).toBeVisible();
    expect(screen.getByTestId("workflow-dry-run")).toBeVisible();
  });

  it("puts actionable tasks before ongoing work and keeps settled history collapsed", async () => {
    const snapshot = retainedRun();
    snapshot.status = "running";
    const failed = task("failed"); failed.spec.id = "repair"; failed.spec.title = "Repair worker";
    const waiting = task("waiting"); waiting.spec.id = "parent"; waiting.spec.title = "Coordinator";
    const settled = task("succeeded"); settled.spec.id = "reader"; settled.spec.title = "Read fixture";
    snapshot.tasks.push(failed, waiting, settled);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    expect(within(screen.getByTestId("workflow-needs-you")).getByTestId("workflow-task-repair")).toBeInTheDocument();
    expect(within(screen.getByTestId("workflow-ready-review")).getByTestId("workflow-task-inspect")).toBeInTheDocument();
    expect(within(screen.getByTestId("workflow-in-flight")).getByTestId("workflow-task-parent")).toBeInTheDocument();
    expect(screen.getByTestId("workflow-settled")).not.toHaveAttribute("open");
    expect(screen.queryByTestId("workflow-task-reader")).not.toBeInTheDocument();
    expect(screen.getByTestId("workflow-verification")).not.toHaveAttribute("open");
    expect(screen.getByTestId("workflow-run-settings")).not.toHaveAttribute("open");
    openSection("workflow-settled");
    await screen.findByTestId("workflow-task-reader");
  });

  it("retains the selected task when a refreshed snapshot moves it between checkpoints", async () => {
    const snapshot = run([task("running")]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    await selectTask();
    expect(screen.queryByTestId("workflow-checkpoints")).not.toBeInTheDocument();
    await waitFor(() => expect(changeListener).toBeDefined());
    const completed = task("succeeded"); completed.result = { answer: 42 };
    mocks.get.mockResolvedValue({ ...snapshot, revision: 5, tasks: [completed] });
    act(() => changeListener!({ payload: { run_id: snapshot.id, revision: 5, kind: "finished" } }));
    await waitFor(() => expect(screen.getByTestId("workflow-task-result")).toHaveTextContent("42"));
    expect(screen.getByTestId("workflow-task-detail")).toHaveTextContent("Completed");
    fireEvent.click(screen.getByTestId("workflow-back-checkpoints"));
    const row = await screen.findByTestId("workflow-task-inspect");
    expect(screen.getByTestId("workflow-settled")).toHaveAttribute("open");
    expect(row).toHaveFocus();
  });

  it("uses a split inspector only when a selected task has enough pane width", async () => {
    let resize: ResizeObserverCallback | undefined;
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: ResizeObserverCallback) { resize = callback; }
      observe() {}
      disconnect() {}
    });
    const snapshot = run([task("running")]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    const inspector = await screen.findByTestId("workflow-run-inspector");
    await waitFor(() => expect(resize).toBeDefined());
    const setWidth = (width: number) => act(() => resize!([{ contentRect: { width } } as ResizeObserverEntry], {} as ResizeObserver));
    setWidth(800);
    expect(inspector).toHaveAttribute("data-layout", "single");
    expect(screen.queryByTestId("workflow-task-detail")).not.toBeInTheDocument();
    await selectTask();
    expect(inspector).toHaveAttribute("data-layout", "split");
    expect(screen.getByRole("button", { name: "Close details" })).toBeInTheDocument();
    expect(screen.getByText("Inspect fixture", { selector: "h3" })).toHaveFocus();
    expect(screen.getByTestId("workflow-checkpoints")).toBeInTheDocument();
    expect(screen.getByTestId("workflow-task-detail")).toBeInTheDocument();
    expect(screen.getByTestId("workflow-task-inspect")).toHaveAttribute("aria-pressed", "true");
    const detail = screen.getByTestId("workflow-task-detail");
    const scrollIntoView = vi.fn();
    detail.scrollIntoView = scrollIntoView;
    detail.scrollTop = 120;
    setWidth(500);
    expect(inspector).toHaveAttribute("data-layout", "single");
    expect(screen.queryByTestId("workflow-checkpoints")).not.toBeInTheDocument();
    expect(screen.getByTestId("workflow-task-detail")).toBeInTheDocument();
    expect(detail.scrollTop).toBe(0);
    expect(scrollIntoView).toHaveBeenCalledWith({ block: "start" });
    fireEvent.click(screen.getByTestId("workflow-back-checkpoints"));
    expect(screen.getByTestId("workflow-task-inspect")).toHaveFocus();
  });

  it("preserves keyboard focus when a task settles into a different checkpoint group", async () => {
    const snapshot = run([task("running")]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    const row = await screen.findByTestId("workflow-task-inspect");
    row.focus();
    expect(row).toHaveFocus();
    await waitFor(() => expect(changeListener).toBeDefined());
    mocks.get.mockResolvedValue({ ...snapshot, revision: 5, tasks: [task("succeeded")] });
    act(() => changeListener!({ payload: { run_id: snapshot.id, revision: 5, kind: "finished" } }));
    await waitFor(() => expect(screen.getByTestId("workflow-settled")).toHaveAttribute("open"));
    expect(screen.getByTestId("workflow-task-inspect")).toHaveFocus();
  });

  it("resets split-to-single detail scroll without taking focus from the composer", async () => {
    let resize: ResizeObserverCallback | undefined;
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: ResizeObserverCallback) { resize = callback; }
      observe() {}
      disconnect() {}
    });
    const snapshot = run([task("running")]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    render(<input aria-label="Outside composer" />);
    await screen.findByTestId("workflow-run-inspector");
    await waitFor(() => expect(resize).toBeDefined());
    const setWidth = (width: number) => act(() => resize!([{ contentRect: { width } } as ResizeObserverEntry], {} as ResizeObserver));
    setWidth(800);
    await selectTask();
    const detail = screen.getByTestId("workflow-task-detail");
    detail.scrollIntoView = vi.fn();
    detail.scrollTop = 120;
    const composer = screen.getByLabelText("Outside composer");
    composer.focus();
    expect(composer).toHaveFocus();
    setWidth(500);
    expect(screen.getByTestId("workflow-run-inspector")).toHaveAttribute("data-layout", "single");
    expect(detail.scrollTop).toBe(0);
    expect(detail.scrollIntoView).toHaveBeenCalledWith({ block: "start" });
    expect(composer).toHaveFocus();
  });

  it("does not pull focus back from an outside pointer interaction when a task settles", async () => {
    const snapshot = run([task("running")]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    const row = await screen.findByTestId("workflow-task-inspect");
    row.focus();
    expect(row).toHaveFocus();
    fireEvent.pointerDown(document.body);
    row.blur();
    expect(document.body).toHaveFocus();
    await waitFor(() => expect(changeListener).toBeDefined());
    mocks.get.mockResolvedValue({ ...snapshot, revision: 5, tasks: [task("succeeded")] });
    act(() => changeListener!({ payload: { run_id: snapshot.id, revision: 5, kind: "finished" } }));
    await waitFor(() => expect(screen.queryByTestId("workflow-task-inspect")).not.toBeInTheDocument());
    expect(screen.getByTestId("workflow-settled")).not.toHaveAttribute("open");
    expect(document.body).toHaveFocus();
  });

  it("never presents missing worker accounting as available capacity", async () => {
    const snapshot = run([task("unknown")]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    const capacity = await screen.findByTestId("workflow-capacity");
    expect(capacity).toHaveTextContent("— / 4");
    expect(capacity).toHaveTextContent("Admission accounting incomplete");
    expect(capacity).toHaveAccessibleName("0 confirmed occupied worker slots, 1 workers awaiting admission accounting");
  });

  it("suppresses retry and application for cancelled status alone", async () => {
    const snapshot = retainedRun();
    snapshot.status = "cancelled"; snapshot.cancel_requested = false;
    const failed = task("failed"); failed.spec.id = "failed";
    snapshot.tasks.push(failed);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    expect(screen.queryByTestId("workflow-ready-review")).not.toBeInTheDocument();
    await selectTask("failed");
    expect(screen.queryByTestId("workflow-retry")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("workflow-back-checkpoints"));
    await selectTask();
    expect(screen.queryByTestId("workflow-apply-artifact")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("workflow-review-artifact"));
    await screen.findByText("new fixture");
    expect(mocks.integrate).not.toHaveBeenCalled();
  });

  it("pages only the requested group and focuses its first added task", async () => {
    const make = (id: string, status: WorkflowTaskSnapshot["status"]) => { const item = task(status); item.spec.id = id; item.spec.title = id; return item; };
    const snapshot = run([...Array.from({ length: 101 }, (_, i) => make(`failed-${i}`, "failed")), ...Array.from({ length: 101 }, (_, i) => make(`queued-${i}`, "queued"))]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    fireEvent.click(await screen.findByTestId("workflow-more-needs-you"));
    expect(await screen.findByTestId("workflow-task-failed-100")).toHaveFocus();
    expect(screen.queryByTestId("workflow-task-queued-100")).not.toBeInTheDocument();
    expect(screen.getByTestId("workflow-more-in-flight")).toBeInTheDocument();
  });

  it("clears a failed launch when starting a new setup without changing run selection", async () => {
    mocks.launch.mockRejectedValue(new Error("Old launch failure"));
    mount();
    fireEvent.click(screen.getByTestId("workflow-dry-run"));
    await screen.findByText("Old launch failure");
    fireEvent.click(screen.getByTestId("workflow-new"));
    await waitFor(() => expect(screen.queryByText("Old launch failure")).not.toBeInTheDocument());
  });

  it("returns to the focused checkpoint with Escape while preserving text-entry behavior", async () => {
    const snapshot = run([task("running")]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    await selectTask();
    fireEvent.keyDown(screen.getByTestId("workflow-run-inspector"), { key: "Escape" });
    expect(await screen.findByTestId("workflow-task-inspect")).toHaveFocus();
  });

  it("retains the selected task and settled disclosure when the pane remounts", async () => {
    const snapshot = run([task("succeeded")]);
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    const first = mount();
    await screen.findByTestId("workflow-run-inspector");
    await selectTask();
    first.unmount();
    mount();
    expect(await screen.findByTestId("workflow-task-detail")).toHaveTextContent("Inspect fixture");
    fireEvent.click(screen.getByTestId("workflow-back-checkpoints"));
    expect(await screen.findByTestId("workflow-task-inspect")).toHaveFocus();
    expect(screen.getByTestId("workflow-settled")).toHaveAttribute("open");
  });

  it("keeps current file review enabled while opening older artifact history", async () => {
    const snapshot = retainedRun();
    const current = snapshot.tasks[0].attempts[0];
    snapshot.tasks[0].attempts.unshift({ ...current, id: "historical", generation: 0, artifacts: [{ ...current.artifacts[0] as object, attempt_id: "historical", digest: "historical-digest" }] });
    mocks.get.mockResolvedValue(snapshot); mocks.list.mockResolvedValue([snapshot]);
    useWorkflowUIStore.getState().selectRun("fixture-workspace", snapshot.id);
    mount();
    await screen.findByTestId("workflow-run-inspector");
    await selectTask();
    fireEvent.click(screen.getByTestId("workflow-review-artifact"));
    await screen.findByText("new fixture");
    await waitFor(() => expect(screen.getByTestId("workflow-apply-artifact")).toBeEnabled());
    openSection("workflow-earlier-artifacts");
    const applies = screen.getAllByTestId("workflow-apply-artifact");
    expect(applies).toHaveLength(1);
    expect(applies[0]).toBeEnabled();
    await screen.findByText("Earlier attempt · review only. These changes cannot be applied.");
  });

});
