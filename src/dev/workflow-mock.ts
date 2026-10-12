import { randomUUID } from "@/lib/uuid";
/** Token-free browser-preview workflow runtime. State survives page reloads. */
import type {
  WorkflowChangeEvent,
  WorkflowAttempt,
  WorkflowCapability,
  WorkflowRunSnapshot,
  WorkflowRunSpec,
  WorkflowScript,
  WorkflowTaskSnapshot,
  WorkflowTaskSpec,
  WorkflowUsage,
} from "@/tauri/workflows";
import { DEFAULT_WORKFLOW_LIMITS } from "@/tauri/workflows";

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;
const STORAGE_KEY = "codemux:dev-workflows:v1";
const MAX_RUNS = 30;
const waiters = new Map<string, Array<(value: unknown) => void>>();
const workers = new Map<string, Worker>();
const timers = new Map<string, ReturnType<typeof setInterval>>();
const terminalTasks = ["succeeded", "failed", "cancelled", "blocked"];
const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
const usage = (): WorkflowUsage => ({ input_tokens: 0, output_tokens: 0, total_tokens: 0, reserved_tokens: 0, estimated_tokens: 0, tokens_unknown: false, cost_usd: 0, cost_unknown: false });
interface State { runs: WorkflowRunSnapshot[]; scripts: WorkflowScript[]; keys: Record<string, string> }
function load(): State {
  try {
    const state = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as State | null;
    if (state && Array.isArray(state.runs) && Array.isArray(state.scripts)) {
      for (const run of state.runs) {
        if (["running", "stopping"].includes(run.status)) {
          run.status = "paused";
          run.pause_requested = true;
          if (run.script && run.script.status === "running") run.script.status = "paused";
          for (const task of run.tasks) if (task.status === "running") {
            task.status = "queued";
            if (task.current_attempt) { task.current_attempt.status = "cancelled"; task.current_attempt.finished_at_ms = Date.now(); }
            task.current_attempt = null;
          }
        }
      }
      return { ...state, keys: state.keys ?? {} };
    }
  } catch { /* A corrupt preview fixture can be replaced without touching app data. */ }
  return { runs: [], scripts: [], keys: {} };
}
let state = load();
function persist() { localStorage.setItem(STORAGE_KEY, JSON.stringify(state)); }
let onChange: ((event: WorkflowChangeEvent) => void) | undefined;
function touch(run: WorkflowRunSnapshot, kind = "changed") { run.updated_at_ms = Date.now(); run.revision += 1; persist(); onChange?.({ run_id: run.id, revision: run.revision, kind, timestamp_ms: run.updated_at_ms, sequence: 0 }); }
function getRun(id: unknown) {
  const run = state.runs.find((item) => item.id === id);
  if (!run) throw new Error("Workflow run not found.");
  return run;
}
function getTask(run: WorkflowRunSnapshot, id: unknown) {
  const task = run.tasks.find((item) => item.spec.id === id);
  if (!task) throw new Error("Workflow task not found.");
  return task;
}
function taskResult(task: WorkflowTaskSnapshot) {
  return { id: task.spec.id, status: task.status, output: task.result, error: task.error, artifacts: task.current_attempt?.artifacts ?? [], generation: task.generation };
}
function settleWaiters(run: WorkflowRunSnapshot, task: WorkflowTaskSnapshot) {
  if (!terminalTasks.includes(task.status)) return;
  const key = `${run.id}:${task.spec.id}`;
  for (const resolve of waiters.get(key) ?? []) resolve(taskResult(task));
  waiters.delete(key);
}
function validate(spec: WorkflowRunSpec) {
  if (!spec.workspace_id || !spec.title.trim() || !spec.goal.trim()) throw new Error("Workspace, name and goal are required.");
  if (!spec.routes.length) throw new Error("Choose at least one provider route.");
  if (new Set(spec.routes.map((route) => route.id)).size !== spec.routes.length) throw new Error("Route IDs must be unique.");
  if (spec.tasks.length > spec.limits.max_tasks) throw new Error("Task budget exceeded.");
}
function taskSnapshot(spec: WorkflowTaskSpec): WorkflowTaskSnapshot {
  return { retired: false, spec: clone(spec), generation: 1, status: "queued", depth: 0, parent_task_id: null, current_attempt: null, attempts: [], result: null, error: null, waiting_for: [], messages: [] };
}
function addTask(run: WorkflowRunSnapshot, raw: WorkflowTaskSpec) {
  const spec: WorkflowTaskSpec = { ...raw, id: raw.id || `task-${run.tasks.length + 1}`, title: raw.title || raw.id || "Task", dependencies: raw.dependencies ?? [], route_id: raw.route_id ?? run.spec.routes[0].id, access: raw.access ?? "read_only", scope: raw.scope ?? [], required: raw.required ?? true };
  const existing = run.tasks.find((task) => task.spec.id === spec.id);
  if (existing) return existing.spec.id;
  if (!["running", "paused"].includes(run.status)) throw new Error("This run no longer accepts tasks.");
  if (run.tasks.length >= run.resolved_limits.max_tasks) throw new Error("Task budget exceeded.");
  if (spec.access === "write" && !run.spec.allow_writes) throw new Error("Enable Allow file changes before submitting a writer task.");
  if (!run.spec.routes.some((route) => route.id === spec.route_id)) throw new Error(`Unknown route: ${spec.route_id}`);
  if (spec.dependencies.some((id) => !run.tasks.some((task) => task.spec.id === id))) throw new Error("Task dependencies must exist before they are referenced.");
  run.tasks.push(taskSnapshot(spec));
  touch(run);
  return spec.id;
}
function create(spec: WorkflowRunSpec, key: string) {
  validate(spec);
  if (state.keys[key]) return getRun(state.keys[key]);
  const id = randomUUID();
  const run: WorkflowRunSnapshot = { id, status: "running", cancel_requested: false, pause_requested: false, error: null, revision: 1, created_at_ms: Date.now(), updated_at_ms: Date.now(), spec: clone(spec), tasks: spec.tasks.map(taskSnapshot), usage: usage(), resolved_limits: { ...DEFAULT_WORKFLOW_LIMITS, ...spec.limits, concurrency: Math.min(spec.limits.concurrency || 4, 8) }, script: null };
  state.runs.unshift(run);
  if (state.runs.length > MAX_RUNS) state.runs = state.runs.slice(0, MAX_RUNS);
  state.keys[key] = id;
  persist();
  onChange?.({ run_id: run.id, revision: run.revision, kind: "created", timestamp_ms: run.updated_at_ms, sequence: 0 });
  schedule(run);
  return run;
}
function finishTask(run: WorkflowRunSnapshot, task: WorkflowTaskSnapshot) {
  const fail = task.spec.prompt.includes("[mock:fail]");
  task.status = fail ? "failed" : "succeeded";
  task.error = fail ? "Synthetic dry-run failure. Edit the prompt or retry this task." : null;
  task.result = fail ? null : { dry_run: true, task_id: task.spec.id, route_id: task.current_attempt?.route_id, summary: `Completed ${task.spec.title}. No provider was called.` };
  if (task.current_attempt) {
    task.current_attempt.status = fail ? "failed" : "succeeded";
    task.current_attempt.finished_at_ms = Date.now();
    task.current_attempt.output = task.result;
    task.current_attempt.error = task.error;
    task.current_attempt.artifacts = [{ kind: "dry_run_report", task_id: task.spec.id, note: "Synthetic report; no workspace files changed." }];
  }
  settleWaiters(run, task);
}
function schedule(run: WorkflowRunSnapshot) {
  if (timers.has(run.id)) return;
  const timer = setInterval(() => {
    if (["completed", "failed", "cancelled"].includes(run.status)) { clearInterval(timer); timers.delete(run.id); return; }
    let changed = false;
    if (Date.now() - run.created_at_ms > run.resolved_limits.wall_time_ms) { run.status = "failed"; run.error = "Workflow time limit exceeded."; workers.get(run.id)?.terminate(); workers.delete(run.id); if (run.script) { run.script.status = "failed"; run.script.error = run.error; } for (const task of run.tasks) if (!terminalTasks.includes(task.status)) cancelTask(run, task); touch(run); return; }
    const delay = Number(new URLSearchParams(location.search).get("workflowDelay")) || 1500;
    for (const task of run.tasks) if (task.status === "running" && task.current_attempt && Date.now() - task.current_attempt.started_at_ms >= delay) { finishTask(run, task); changed = true; }
    if (run.status === "running") {
      let running = run.tasks.filter((task) => task.status === "running").length;
      for (const task of run.tasks) {
        if (task.status !== "queued") continue;
        const dependencies = task.spec.dependencies.map((id) => getTask(run, id));
        if (dependencies.some((dependency) => ["failed", "blocked", "cancelled"].includes(dependency.status))) { task.status = "blocked"; task.error = "A required dependency did not succeed."; settleWaiters(run, task); changed = true; continue; }
        if (dependencies.some((dependency) => dependency.status !== "succeeded") || running >= run.resolved_limits.concurrency) continue;
        if (run.tasks.reduce((count, item) => count + item.attempts.length, 0) >= run.resolved_limits.max_attempts) { task.status = "failed"; task.error = "Attempt budget exceeded."; settleWaiters(run, task); changed = true; continue; }
        const attempt: WorkflowAttempt = { id: randomUUID(), generation: task.generation, operation_id: randomUUID(), status: "running", route_id: task.spec.route_id ?? run.spec.routes[0].id, started_at_ms: Date.now(), finished_at_ms: null, cancel_requested: false, reserved_tokens: 0, external_ref: null, output: null, error: null, usage: usage(), artifacts: [] };
        task.current_attempt = attempt;
        task.attempts.push(attempt);
        task.status = "running";
        running += 1;
        changed = true;
      }
      if (run.tasks.length > 0 && run.tasks.every((task) => terminalTasks.includes(task.status)) && (!run.script || ["completed", "failed"].includes(run.script.status))) {
        run.status = run.tasks.some((task) => task.spec.required && !task.retired && task.status !== "succeeded") || run.script?.status === "failed" ? "failed" : "completed";
        changed = true;
      }
    }
    if (changed) touch(run);
  }, 100);
  timers.set(run.id, timer);
}

/** Runs scripts off the UI thread; no provider runtime exists in this mock. */
function executeScript(run: WorkflowRunSnapshot) {
  if (!run.spec.script || workers.has(run.id)) return;
  const script = run.spec.script;
  if (/\b(import|fetch|XMLHttpRequest|WebSocket|EventSource|Worker|SharedWorker|importScripts|setTimeout|setInterval)\b/.test(script.source)) { run.script = { status: "failed", result: null, error: "Imports, network, and timers are unavailable in workflow scripts.", phase: null }; run.status = "failed"; touch(run); return; }
  run.script = { status: "running", result: null, error: null, phase: null };
  const workerSource = `
    const waiting = new Map(); let sequence = 0, taskSequence = 0;
    function rpc(method, params) { const id = ++sequence; return new Promise((resolve, reject) => { waiting.set(id, {resolve,reject}); postMessage({id,method,params}); }); }
    onmessage = async (event) => {
      const data = event.data;
      if (data.reply) { const pending=waiting.get(data.id); if (pending) {waiting.delete(data.id);data.error?pending.reject(new Error(data.error)):pending.resolve(data.value);} return; }
      if (data.start) {
        const normalize = spec=>({...spec,id:spec.id||'agent-'+(++taskSequence),title:spec.title||'Agent task',dependencies:spec.dependencies||[],route_id:spec.route_id||null,access:spec.access||'read_only',scope:spec.scope||[],output_schema:spec.output_schema||spec.schema||null,required:spec.required!==false});
        const workflow = {addTask: spec=>rpc('addTask',normalize(spec)),addTasks:specs=>rpc('addTasks',specs.map(normalize)),wait:id=>rpc('wait',id),cancelTask:id=>rpc('cancelTask',id),retire:id=>rpc('retire',id),list:()=>rpc('list'),message:(id,text)=>rpc('message',{id,text}),replace:(id,spec)=>rpc('replace',{id,spec:normalize({...spec,id})}),phase:title=>rpc('phase',title),log:text=>rpc('log',text)};
        const agent = async(prompt, options={})=>{ const id=await workflow.addTask({prompt,...options, output_schema: options.output_schema ?? options.schema});return workflow.wait(id); };
        const parallel = async(items, fn)=>{if(!Array.isArray(items)||items.length>4096)throw new Error('Fan-out requires at most 4096 items');return Promise.all(items.map(fn||((item)=>typeof item==='function'?item():item)));};
        const pipeline = parallel;
        self.fetch=undefined;self.XMLHttpRequest=undefined;self.WebSocket=undefined;self.importScripts=undefined;self.EventSource=undefined;self.Worker=undefined;self.SharedWorker=undefined;self.indexedDB=undefined;self.caches=undefined;self.setTimeout=undefined;self.setInterval=undefined;self.Date=undefined;Math.random=undefined;
        try { const AsyncFunction=Object.getPrototypeOf(async function(){}).constructor;const result=await new AsyncFunction('workflow','agent','parallel','pipeline','args','goal','routes','allow_writes',data.source)(workflow,agent,parallel,pipeline,data.args,data.goal,data.routes,data.allow_writes);postMessage({finished:true,result}); }
        catch(error){postMessage({failed:true,error:String(error?.message??error)});}
      }
    };
  `;
  const url = URL.createObjectURL(new Blob([workerSource], { type: "text/javascript" }));
  const worker = new Worker(url);
  URL.revokeObjectURL(url);
  workers.set(run.id, worker);
  worker.onmessage = async (event: MessageEvent<Record<string, unknown>>) => {
    const data = event.data;
    if (data.finished || data.failed) {
      if (run.script) { run.script.status = data.failed ? "failed" : "completed"; run.script.result = data.result ?? null; run.script.error = data.failed ? String(data.error) : null; }
      if (data.failed && run.tasks.length === 0) run.status = "failed";
      touch(run);
      worker.terminate();
      workers.delete(run.id);
      return;
    }
    try {
      let value: unknown;
      switch (data.method) {
        case "addTask": value = addTask(run, data.params as WorkflowTaskSpec); break;
        case "addTasks": value = (data.params as WorkflowTaskSpec[]).map((spec) => addTask(run, spec)); break;
        case "wait": {
          const task = getTask(run, data.params);
          value = terminalTasks.includes(task.status) ? taskResult(task) : await new Promise((resolve) => { const key = `${run.id}:${task.spec.id}`; waiters.set(key, [...(waiters.get(key) ?? []), resolve]); });
          break;
        }
        case "list": value = clone(run.tasks.map((task) => ({ id: task.spec.id, title: task.spec.title, status: task.status, generation: task.generation, route_id: task.current_attempt?.route_id ?? task.spec.route_id, dependencies: task.spec.dependencies }))); break;
        case "retire": { const task = getTask(run, data.params); cancelTask(run, task, true); value = taskResult(task); break; }
        case "cancelTask": { const task = getTask(run, data.params); cancelTask(run, task); value = taskResult(task); break; }
        case "message": { const params = data.params as { id: string; text: string }; getTask(run, params.id).messages.push(params.text); touch(run); break; }
        case "replace": { const params = data.params as { id: string; spec: WorkflowTaskSpec }; replaceTask(run, params.id, params.spec); break; }
        case "phase": if (run.script) { run.script.phase = String(data.params); touch(run); } break;
        case "log": break;
        default: throw new Error("Unknown workflow method.");
      }
      if (workers.has(run.id)) worker.postMessage({ reply: true, id: data.id, value });
    } catch (error) { if (workers.has(run.id)) worker.postMessage({ reply: true, id: data.id, error: error instanceof Error ? error.message : String(error) }); }
  };
  worker.onerror = (event) => { if (run.script) { run.script.status = "failed"; run.script.error = event.message; } run.status = "failed"; touch(run); worker.terminate(); workers.delete(run.id); };
  worker.postMessage({ start: true, source: script.source, args: script.args, goal: run.spec.goal, routes: run.spec.routes, allow_writes: run.spec.allow_writes });
  touch(run);
}
function cancelTask(run: WorkflowRunSnapshot, task: WorkflowTaskSnapshot, retire = false) {
  const retirementChanged = retire && !task.retired;
  if (retire) task.retired = true;
  if (["succeeded", "failed", "cancelled"].includes(task.status)) { if (retirementChanged) touch(run); return; }
  task.status = "cancelled";
  if (task.current_attempt) { task.current_attempt.status = "cancelled"; task.current_attempt.cancel_requested = true; task.current_attempt.finished_at_ms = Date.now(); }
  settleWaiters(run, task);
  touch(run);
}
function replaceTask(run: WorkflowRunSnapshot, id: string, spec: WorkflowTaskSpec) {
  const task = getTask(run, id);
  if (["running", "unknown", "stopping"].includes(task.status)) throw new Error("Cancel and settle the current attempt before replacing it.");
  task.spec = { ...spec, id };
  task.retired = false;
  task.generation += 1;
  task.status = "queued";
  task.result = null;
  task.error = null;
  task.current_attempt = null;
  if (["completed", "failed"].includes(run.status)) run.status = "running";
  touch(run);
  schedule(run);
}

const capabilities: WorkflowCapability[] = ["claude", "codex", "hermes", "opencode", "cursor", "grok"].map((provider) => ({ provider: provider as WorkflowCapability["provider"], live: false, read_only: true, write: false, requires_model: provider === "opencode", effort_supported: provider !== "cursor", reason: "Browser preview runs only synthetic tasks. No provider is called." }));

export function workflowMockHandlers(listener?: (event: WorkflowChangeEvent) => void): Record<string, Handler> {
  onChange = listener;
  return {
    workflow_capabilities: () => clone(capabilities),
    workflow_list: (args) => state.runs.filter((run) => run.spec.workspace_id === args.workspaceId).map((run) => ({ id: run.id, status: run.status, revision: run.revision, created_at_ms: run.created_at_ms, updated_at_ms: run.updated_at_ms, spec: { workspace_id: run.spec.workspace_id, title: run.spec.title, mode: run.spec.mode } })),
    workflow_get: (args) => clone(getRun(args.runId)),
    workflow_create: (args) => clone(create(args.spec as WorkflowRunSpec, String(args.idempotencyKey))),
    workflow_script_execute: (args) => {
      const input = args.input as { spec: WorkflowRunSpec; source: string; args: unknown; idempotency_key: string };
      const run = create({ ...input.spec, script: { source: input.source, args: input.args, api_version: 1 } }, input.idempotency_key);
      executeScript(run);
      return clone(run);
    },
    workflow_pause: (args) => { const run = getRun(args.runId); run.status = "paused"; run.pause_requested = true; if (run.script?.status === "running") run.script.status = "paused"; touch(run); return clone(run); },
    workflow_resume: (args) => { const run = getRun(args.runId); run.status = "running"; run.pause_requested = false; if (run.script?.status === "paused") { if (workers.has(run.id)) run.script.status = "running"; else executeScript(run); } schedule(run); touch(run); return clone(run); },
    workflow_cancel: (args) => { const run = getRun(args.runId); run.status = "cancelled"; run.cancel_requested = true; workers.get(run.id)?.terminate(); workers.delete(run.id); if (run.script && ["running", "paused"].includes(run.script.status)) run.script.status = "failed"; for (const task of run.tasks) if (!terminalTasks.includes(task.status)) cancelTask(run, task); touch(run); return clone(run); },
    workflow_cancel_task: (args) => { const run = getRun(args.runId); cancelTask(run, getTask(run, args.taskId)); return clone(run); },
    workflow_integrate_artifact: () => { throw new Error("Browser preview retains synthetic results only; apply file changes from the desktop host."); },
    workflow_artifact_preview: (args) => {
      const run = getRun(args.runId);
      const task = getTask(run, args.taskId);
      const attempt = task.attempts.find((item) => item.id === args.attemptId);
      const retained = attempt?.artifacts.find((value) => value && typeof value === "object" && "kind" in value && value.kind === "workspace_snapshot" && "digest" in value && value.digest === args.digest && "changed_paths" in value && Array.isArray(value.changed_paths) && value.changed_paths.includes(args.path));
      if (!retained) throw new Error("No retained file matches this preview request.");
      return { path: String(args.path), before: "// Synthetic browser-preview baseline\n", after: "// Synthetic browser-preview retained change\n", binary: false, truncated: false, before_sha256: null, after_sha256: null };
    },
    workflow_reconcile: () => { throw new Error("Browser preview has no managed process to reconcile. Unknown executions must be reconciled by the host."); },
    workflow_retry: (args) => { const run = getRun(args.runId); const task = getTask(run, args.taskId); if (run.cancel_requested) throw new Error("Cancelled runs cannot retry."); if (!["failed", "cancelled", "blocked"].includes(task.status)) throw new Error("Only settled tasks can be retried."); task.status = "queued"; task.retired = false; task.error = null; task.result = null; task.current_attempt = null; run.status = "running"; run.error = null; touch(run); schedule(run); return clone(run); },
    workflow_replace: (args) => { const run = getRun(args.runId); replaceTask(run, String(args.taskId), args.spec as WorkflowTaskSpec); return clone(run); },
    workflow_retire: (args) => { const run = getRun(args.runId); cancelTask(run, getTask(run, args.taskId), true); return clone(run); },
    workflow_message: (args) => { const run = getRun(args.runId); getTask(run, args.taskId).messages.push(String(args.text)); touch(run); return clone(run); },
    workflow_script_list: (args) => clone(state.scripts.filter((script) => script.workspace_id === args.workspaceId)),
    workflow_script_save: (args) => {
      const existing = state.scripts.find((script) => script.workspace_id === args.workspaceId && script.title === args.name);
      const script: WorkflowScript = { id: existing?.id ?? randomUUID(), workspace_id: String(args.workspaceId), title: String(args.name), source: String(args.source), updated_at_ms: Date.now() };
      state.scripts = [script, ...state.scripts.filter((item) => item.id !== script.id)];
      persist();
      return clone(script);
    },
  };
}

/** Isolated fixture state for focused tests. Never invokes a provider. */
export function resetWorkflowMock() {
  for (const timer of timers.values()) clearInterval(timer);
  for (const worker of workers.values()) worker.terminate();
  timers.clear(); workers.clear(); waiters.clear();
  state = { runs: [], scripts: [], keys: {} };
  persist();
}
