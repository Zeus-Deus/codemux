/* Shared mockup kit: synthetic scenario, app shell, shared renderers.
 * Everything is in-memory and illustrative. No network, storage or backend. */
(function () {
  "use strict";
  const P = {
    check: '<path d="M20 6 9 17l-5-5"/>', x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>', plus: '<path d="M5 12h14"/><path d="M12 5v14"/>',
    right: '<path d="m9 18 6-6-6-6"/>', down: '<path d="m6 9 6 6 6-6"/>', left: '<path d="m15 18-6-6 6-6"/>',
    loader: '<path d="M21 12a9 9 0 1 1-6.219-8.56"/>', pause: '<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>',
    play: '<polygon points="6 3 20 12 6 21 6 3"/>', stop: '<rect width="16" height="16" x="4" y="4" rx="2"/>',
    rotate: '<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>',
    alert: '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
    help: '<circle cx="12" cy="12" r="10"/><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/><path d="M12 17h.01"/>',
    hourglass: '<path d="M5 22h14"/><path d="M5 2h14"/><path d="M17 22v-4.172a2 2 0 0 0-.586-1.414L12 12l-4.414 4.414A2 2 0 0 0 7 17.828V22"/><path d="M7 2v4.172a2 2 0 0 0 .586 1.414L12 12l4.414-4.414A2 2 0 0 0 17 6.172V2"/>',
    circle: '<circle cx="12" cy="12" r="4"/>', ban: '<circle cx="12" cy="12" r="10"/><path d="m4.9 4.9 14.2 14.2"/>',
    network: '<rect x="16" y="16" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/><rect x="9" y="2" width="6" height="6" rx="1"/><path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"/><path d="M12 12V8"/>',
    branch: '<line x1="6" x2="6" y1="3" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>',
    file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>',
    max: '<polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" x2="14" y1="3" y2="10"/><line x1="3" x2="10" y1="21" y2="14"/>',
    min: '<polyline points="4 14 10 14 10 20"/><polyline points="20 10 14 10 14 4"/><line x1="14" x2="21" y1="10" y2="3"/><line x1="3" x2="10" y1="21" y2="14"/>',
    panelL: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>', panelR: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M15 3v18"/>',
    folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
    search: '<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>', msg: '<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>',
    up: '<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>', code: '<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>',
    shield: '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>',
    eye: '<path d="M2.06 12.35a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 19.88 0 1 1 0 0 1 0 .7 10.75 10.75 0 0 1-19.88 0"/><circle cx="12" cy="12" r="3"/>',
    archive: '<rect width="20" height="5" x="2" y="3" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/><path d="M10 12h4"/>',
    settings: '<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2 12h3M19 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1"/>',
    pen: '<path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.4 2.6a1 1 0 0 1 3 3L12.4 14.6l-4 1 1-4z"/>',
    minus: '<path d="M5 12h14"/>', sq: '<rect width="14" height="14" x="5" y="5" rx="1"/>', diff: '<path d="M12 3v14"/><path d="M5 10h14"/><path d="M5 21h14"/>',
    layers: '<path d="m12 2 10 5-10 5L2 7z"/><path d="m2 17 10 5 10-5"/><path d="m2 12 10 5 10-5"/>', history: '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>'
  };
  const ic = (n, s = 14, c = "") => `<svg class="ic ${c}" width="${s}" height="${s}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${P[n]}</svg>`;
  const esc = (v) => String(v).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const t = (s) => `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
  const k = (n) => (n >= 1000 ? (n / 1000).toFixed(n >= 100000 ? 0 : 1).replace(/\.0$/, "") + "k" : String(n));

  /* ── Scenario ─────────────────────────────────────────────── */
  const ROUTES = [
    { id: "claude/coordinator", provider: "Claude", role: "coordinator", effort: "high" },
    { id: "claude/writer", provider: "Claude", role: "writer", effort: "medium" },
    { id: "codex/writer", provider: "Codex", role: "writer", effort: "medium" },
    { id: "codex/reviewer", provider: "Codex", role: "reviewer", effort: "high" }
  ];
  const DRY_ONLY = ["Hermes", "OpenCode", "Cursor", "Grok"];
  const TASKS = {
    coordinator: { title: "Coordinate session recovery", parent: null, depth: 0, access: "write", scope: ["src/session", "src/ui/recovery"], deps: [], route: "claude/coordinator",
      prompt: "Refactor session recovery so a crashed session restores its newest readable snapshot. Split the work, wait for typed results, repair failures, then add a synthesis task." },
    arch: { title: "Map recovery architecture", parent: "coordinator", depth: 1, access: "read_only", scope: ["src/session", "src/ui/recovery"], deps: [], route: "codex/reviewer",
      prompt: "Map how sessions are snapshotted and restored. Return modules, hazards and a proposal as JSON." },
    storage: { title: "Rework snapshot storage", parent: "coordinator", depth: 1, access: "write", scope: ["src/session"], deps: ["arch"], route: "claude/writer",
      prompt: "Make restoreLatest fall back to the newest readable snapshot. Keep v1 snapshots readable. Return changed paths and migration_notes[]." },
    ui: { title: "Recovery banner UI", parent: "coordinator", depth: 1, access: "write", scope: ["src/ui/recovery"], deps: ["arch"], route: "codex/writer",
      prompt: "Show restored, restored-older and unrecoverable states in RecoveryBanner." },
    tests: { title: "Audit recovery test coverage", parent: "coordinator", depth: 1, access: "read_only", scope: ["src/session"], deps: ["storage"], route: "codex/reviewer",
      prompt: "Audit the recovery tests in src/session against the new storage result and return a test plan. You cannot run commands; do not claim results." },
    synthesis: { title: "Synthesize and verify results", parent: "coordinator", depth: 1, access: "read_only", scope: ["src/session", "src/ui/recovery"], deps: ["storage", "ui", "tests"], route: "claude/coordinator",
      prompt: "Review typed child results, schema conformance, scope and artifact manifests. Summarize what is verified and what is not." }
  };
  const ORDER = ["coordinator", "arch", "storage", "ui", "tests", "synthesis"];
  const A0 = {
    c1: { task: "coordinator", gen: 1, n: 1, route: "claude/coordinator", slot: 1, start: 2, end: 42, status: "waiting", tok: { obs: 18420 }, note: "Added arch, storage and ui; yielded while waiting" },
    a1: { task: "arch", gen: 1, n: 1, route: "codex/reviewer", slot: 1, start: 43, end: 130, status: "succeeded", tok: { obs: 41380 } },
    s1: { task: "storage", gen: 1, n: 1, route: "codex/writer", slot: 1, start: 131, end: 295, status: "failed", tok: { obs: 36900 }, error: "Result rejected by output_schema: migration_notes must be an array (got string). No artifact was sealed." },
    u1: { task: "ui", gen: 1, n: 1, route: "codex/writer", slot: 2, start: 131, end: 422, status: "succeeded", tok: { obs: 38240 } },
    c2: { task: "coordinator", gen: 1, n: 2, route: "claude/coordinator", slot: 1, start: 296, end: 320, status: "waiting", tok: { obs: 9860 }, note: "Replaced storage as generation 2 on claude/writer; added tests" },
    s2: { task: "storage", gen: 2, n: 2, route: "claude/writer", slot: 1, start: 321, end: 475, status: "succeeded", tok: { obs: 44510 } },
    c3: { task: "coordinator", gen: 1, n: 3, route: "claude/coordinator", slot: 1, start: 476, end: 498, status: "waiting", tok: { est: 7100, unknown: true }, note: "Added synthesis; final usage counter missing, so an estimate counts toward the budget" },
    t1: { task: "tests", gen: 1, n: 1, route: "codex/reviewer", slot: 2, start: 477, end: 630, status: "succeeded", tok: { obs: 29770 } },
    y1: { task: "synthesis", gen: 1, n: 1, route: "claude/coordinator", slot: 1, start: 631, end: 845, status: "succeeded", tok: { obs: 33050 } },
    c4: { task: "coordinator", gen: 1, n: 4, route: "claude/coordinator", slot: 1, start: 846, end: 880, status: "succeeded", tok: { obs: 6240 } }
  };
  const RUNNING = { status: "running", end: null, tok: { res: 12000 } };
  const RESULTS = {
    arch: { modules: ["session/snapshot-store", "session/legacy-restore", "ui/recovery/RecoveryBanner"], hazards: ["restoreLatest trusts the newest file even if it is corrupt", "v1 snapshots have no checksum"], proposal: "Move decoding into snapshot-codec; fall back to older readable snapshots" },
    storage: { changed: ["src/session/snapshot-store.ts", "src/session/snapshot-codec.ts", "src/session/legacy-restore.ts", "src/session/fixtures/journal-large.json"], migration_notes: ["v1 snapshots decode through readLegacy()", "legacy-restore.ts folded into snapshot-codec.ts"] },
    ui: { changed: ["src/ui/recovery/RecoveryBanner.tsx", "src/ui/recovery/restore-illustration.png"], states: ["restored", "restored-older", "unrecoverable"] },
    tests: { kind: "test_plan", executed: false, reviewed: ["src/session/snapshot-store.test.ts"], proposed_file: "src/session/snapshot-codec.test.ts", cases: ["newest snapshot corrupt → restores previous", "all snapshots corrupt → unrecoverable banner", "v1 snapshot without checksum decodes", "empty snapshot dir → null", "truncated journal is skipped", "banner shows source timestamp", "codec rejects unknown version"] },
    synthesis: { verdict: "ready_for_review", host_checked: ["5 typed results matched schema", "changed paths inside scope", "2 artifacts sealed; manifest hashes match"], agent_reviewed: ["storage and ui agree on the three recovery states", "test plan covers corrupt, missing and v1 snapshots"], not_run: ["tests", "build", "type check"] },
    coordinator: { summary: "Restore now falls back to the newest readable snapshot; the banner explains which snapshot was used.", accepted: ["storage@g2", "ui@g1"], follow_up: "Add the 7 planned test cases, then run them yourself." }
  };
  const SNIP = {
    storeBefore: 'export async function restoreLatest(dir: string) {\n  const files = await listSnapshots(dir);\n  const latest = files.sort().at(-1);\n  if (!latest) return null;\n  return JSON.parse(await readFile(latest, "utf8"));\n}',
    storeAfter: 'export async function restoreLatest(dir: string): Promise<Restored | null> {\n  for (const file of await listSnapshots(dir, { newestFirst: true })) {\n    const decoded = decodeSnapshot(await readFile(file));\n    if (decoded.ok) return { snapshot: decoded.value, source: file };\n    log.warn("skipping unreadable snapshot", { file, reason: decoded.reason });\n  }\n  return null;\n}',
    codecAfter: 'export function decodeSnapshot(bytes: Uint8Array): Decoded {\n  const header = readHeader(bytes);\n  if (header.version === 1) return readLegacy(bytes);\n  if (!verifyChecksum(bytes, header)) return fail("checksum");\n  return ok(parseBody(bytes, header));\n}',
    legacyBefore: 'export function restoreLegacy(raw: string) {\n  // v1 snapshots: plain JSON, no checksum\n  return JSON.parse(raw);\n}',
    bannerBefore: 'export function RecoveryBanner({ restored }: Props) {\n  if (!restored) return null;\n  return <Banner tone="info">Session restored</Banner>;\n}',
    bannerAfter: 'export function RecoveryBanner({ result }: Props) {\n  if (result.kind === "unrecoverable")\n    return <Banner tone="attention">No readable snapshot. Started fresh.</Banner>;\n  const older = result.kind === "restored-older";\n  return <Banner tone="info">Restored from {formatTime(result.at)}{older ? " (newest was unreadable)" : ""}</Banner>;\n}'
  };
  const ARTIFACTS = {
    "art-storage": { task: "storage", attempt: "s2", label: "storage · gen 2", digest: "sha256:7c1e0b94f2…a83d", files: [
      { path: "src/session/snapshot-store.ts", change: "modified", before: SNIP.storeBefore, after: SNIP.storeAfter, b: "41d2…9e07", a: "c8a0…31f4" },
      { path: "src/session/snapshot-codec.ts", change: "added", before: null, after: SNIP.codecAfter, b: null, a: "0f6b…d2c9" },
      { path: "src/session/legacy-restore.ts", change: "deleted", before: SNIP.legacyBefore, after: null, b: "9a17…5b20", a: null },
      { path: "src/session/fixtures/journal-large.json", change: "modified", truncated: true, before: '{"entries":[{"id":1,"kind":"open"},…', after: '{"version":2,"entries":[{"id":1,"kind":"open"},…', b: "e3b4…77a1", a: "5d90…c4e8" }] },
    "art-ui": { task: "ui", attempt: "u1", label: "ui · gen 1", digest: "sha256:4be1c07d9a…10fe", conflict: "src/ui/recovery/RecoveryBanner.tsx", files: [
      { path: "src/ui/recovery/RecoveryBanner.tsx", change: "modified", before: SNIP.bannerBefore, after: SNIP.bannerAfter, b: "2c6e…8f13", a: "b7d5…0a92" },
      { path: "src/ui/recovery/restore-illustration.png", change: "added", binary: true, b: null, a: "6e21…f0b8" }] }
  };
  const STATES = { setup: "Setup", running: "Running", paused: "Paused", unknown: "Unknown", review: "Review", cancelled: "Cancelled" };

  function snapshot(S) {
    const st = S.state === "setup" ? "running" : S.state;
    const att = {};
    const add = (id, patch) => { att[id] = Object.assign({ id }, A0[id], patch || {}); if (patch && patch.tok) att[id].tok = patch.tok; };
    ["c1", "a1", "s1", "c2"].forEach((id) => add(id));
    let now, status = "running", phase, tasks = {}, events, extra = {};
    const T = (id, s, more) => (tasks[id] = Object.assign({ id, status: s, gen: 1 }, TASKS[id], more || {}));
    if (st === "running") {
      now = 390; phase = "Build"; add("u1", RUNNING); add("s2", RUNNING);
      T("coordinator", "waiting", { waitingFor: ["storage", "ui"] }); T("arch", "succeeded"); T("storage", "running", { gen: 2 }); T("ui", "running"); T("tests", "queued", { waitingFor: ["storage"] });
    } else if (st === "paused") {
      now = 430; phase = "Build"; status = "paused"; add("u1"); add("s2", Object.assign({}, RUNNING, { draining: true }));
      T("coordinator", "waiting", { waitingFor: ["storage"] }); T("arch", "succeeded"); T("storage", "running", { gen: 2, draining: true }); T("ui", "succeeded"); T("tests", "queued", { waitingFor: ["storage"], held: "Admission paused" });
    } else if (st === "unknown") {
      now = 495; phase = "Audit"; add("s2"); add("t1", { slot: 1, start: 476, status: "running", end: null, tok: { res: 12000 } });
      if (!S.reconciled) {
        status = "unknown";
        add("u1", { status: "unknown", end: null, cancelAt: 450, tok: { est: 14800, res: 12000, unknown: true }, error: "Cancelled by you at 07:30. The provider process group has not confirmed it stopped, so the slot and its 12k reservation stay held." });
        T("ui", "unknown");
      } else {
        add("u1", { status: "cancelled", end: 495, cancelAt: 450, tok: { est: 14800, unknown: true }, error: "Stop verified by Reconcile (illustrative). Slot and reservation released." });
        if (S.retried) { att.u2 = { id: "u2", task: "ui", gen: 1, n: 2, route: "codex/writer", slot: 2, start: 495, end: null, status: "running", tok: { res: 12000 } }; T("ui", "running"); }
        else T("ui", "cancelled", { needsRepair: true });
      }
      T("coordinator", "waiting", { waitingFor: ["ui"] }); T("arch", "succeeded"); T("storage", "succeeded", { gen: 2 }); T("tests", "running");
    } else if (st === "review") {
      now = 881; phase = "Complete"; status = "completed"; ["u1", "s2", "c3", "t1", "y1", "c4"].forEach((id) => add(id));
      ORDER.forEach((id) => T(id, "succeeded", id === "storage" ? { gen: 2 } : {}));
    } else {
      now = 445; phase = "Build"; status = "cancelled"; add("u1"); add("s2", { status: "cancelled", end: 436, tok: { est: 21300, unknown: true }, error: "Run cancelled by you at 07:10. Stop verified at 07:16; final usage counter missing, so the figure is an estimate." });
      T("coordinator", "cancelled"); T("arch", "succeeded"); T("storage", "cancelled", { gen: 2 }); T("ui", "succeeded"); T("tests", "cancelled", { note: "Never admitted" });
    }
    Object.values(tasks).forEach((tk) => {
      tk.attempts = Object.values(att).filter((a) => a.task === tk.id).sort((x, y) => y.n - x.n);
      tk.current = tk.attempts[0] || null;
      tk.result = (tk.status === "succeeded" && RESULTS[tk.id]) || null;
      tk.dependents = Object.keys(tasks).filter((o) => tasks[o].deps.includes(tk.id));
    });
    if (tasks.storage) tasks.storage.messages = ["coordinator → storage (gen 2): Keep the v1 snapshot reader. Return migration_notes as an array."];
    const all = Object.values(att);
    const usage = { obs: 0, est: 0, res: 0, unknown: false };
    all.forEach((a) => { usage.obs += a.tok.obs || 0; usage.est += a.tok.est || 0; usage.res += (a.end === null || a.status === "unknown") ? (a.tok.res || 0) : 0; usage.unknown = usage.unknown || !!a.tok.unknown; });
    const slots = [1, 2].map((n) => { const a = all.find((x) => x.slot === n && (x.end === null)); return { n, attempt: a ? a.id : null, held: !!a && a.status === "unknown", draining: !!a && !!a.draining }; });
    const busy = slots.filter((s) => s.attempt).length;
    const arts = Object.entries(ARTIFACTS).filter(([, v]) => tasks[v.task] && att[v.attempt] && att[v.attempt].status === "succeeded").map(([id, v]) => {
      const a = Object.assign({ id }, v);
      a.canApply = status !== "cancelled"; a.reason = status === "cancelled" ? "Apply unavailable: cancelling the run revoked application authority for every artifact in it." : "";
      a.sealed = att[v.attempt].end; return a;
    });
    const needs = [];
    if (status === "paused") needs.push({ kind: "paused", text: "Paused by you at 06:48. No new tasks are admitted; storage (gen 2) is still finishing in slot 1.", act: "resume" });
    if (tasks.ui && tasks.ui.status === "unknown") needs.push({ kind: "unknown", task: "ui", text: "ui: stop not confirmed after you cancelled it. It holds slot 2 and a 12k token reservation until Reconcile proves it stopped. The coordinator is waiting on it.", act: "reconcile" });
    if (tasks.ui && tasks.ui.needsRepair) needs.push({ kind: "repair", task: "ui", text: "ui is a required task and was cancelled. Retry it, or message the coordinator to replace it.", act: "retry" });
    if (status === "cancelled") needs.push({ kind: "cancelled", text: "Run cancelled by you at 07:10. Retry and Apply are revoked for every task. Evidence stays available to inspect.", act: null });
    events = buildEvents(st, S, now);
    return { state: st, now, status, phase, tasks, order: ORDER.filter((id) => tasks[id]), att, attempts: all.sort((x, y) => x.start - y.start), usage, slots, busy, artifacts: arts, needs, events, routes: ROUTES, ceiling: 2 };
  }

  function buildEvents(st, S, now) {
    const E = [
      [0, "run", "Run started", "live routes · 2-worker ceiling · file changes allowed in src/session, src/ui/recovery"],
      [2, "admit", "coordinator turn 1 admitted", "slot 1 · claude/coordinator", "coordinator", 1],
      [40, "decision", "Coordinator added arch, storage, ui", "storage ← arch · ui ← arch", "coordinator", 1],
      [42, "yield", "Coordinator yielded slot 1", "waiting for storage, ui · not counted against the ceiling", "coordinator", 1],
      [43, "admit", "arch admitted", "slot 1 · codex/reviewer", "arch"],
      [130, "result", "arch result accepted", "matched output_schema", "arch"],
      [131, "admit", "storage and ui admitted", "slot 1 storage · codex/writer — slot 2 ui · codex/writer", "storage"],
      [295, "fail", "storage gen 1 failed", "result rejected: migration_notes must be an array", "storage"],
      [296, "admit", "coordinator turn 2 resumed", "slot 1 · received storage's failed outcome", "coordinator", 2],
      [318, "decision", "Coordinator replaced storage → gen 2 on claude/writer", "guidance: keep the v1 reader; return migration_notes as an array · added tests ← storage", "coordinator", 2],
      [320, "yield", "Coordinator yielded slot 1", "waiting for storage, ui", "coordinator", 2],
      [321, "admit", "storage gen 2 admitted", "slot 1 · claude/writer", "storage"]
    ];
    const add = (rows) => rows.forEach((r) => E.push(r));
    if (st === "paused" || st === "review") add([[408, "user", "You paused the run", "admitted work drains; nothing new is admitted"], [422, "result", "ui result accepted · 2 files sealed", "matched output_schema · artifact hash-verified", "ui"]]);
    if (st === "review") add([[440, "user", "You resumed the run", ""], [475, "result", "storage gen 2 accepted · 4 files sealed", "matched output_schema · artifact hash-verified", "storage"],
      [476, "admit", "coordinator turn 3 resumed", "slot 1", "coordinator", 3], [477, "admit", "tests admitted", "slot 2 · codex/reviewer", "tests"],
      [496, "decision", "Coordinator added synthesis", "synthesis ← storage, ui, tests", "coordinator", 3], [498, "yield", "Coordinator yielded slot 1", "waiting for synthesis", "coordinator", 3],
      [630, "result", "tests result accepted · test plan, not executed", "7 proposed cases · managed workers cannot run commands", "tests"], [631, "admit", "synthesis admitted", "slot 1 · claude/coordinator", "synthesis"],
      [845, "result", "synthesis result accepted", "host checks + agent review summarized", "synthesis"], [846, "admit", "coordinator turn 4 resumed", "slot 1", "coordinator", 4],
      [880, "result", "Coordinator submitted the final result", "matched output_schema", "coordinator", 4], [881, "run", "Run completed", "2 change sets ready for review"]]);
    if (st === "unknown") {
      add([[450, "user", "You cancelled ui", "to narrow its scope"], [475, "result", "storage gen 2 accepted · 4 files sealed", "matched output_schema · artifact hash-verified", "storage"],
        [476, "admit", "tests admitted", "slot 1 · codex/reviewer", "tests"], [480, "unknown", "ui stop not confirmed → Unknown", "slot 2 and 12k reservation held until Reconcile", "ui"]]);
      if (S.reconciled) E.push([495, "user", "You reconciled ui (illustrative)", "stop verified · slot 2 released · ui needs repair", "ui"]);
      if (S.retried) E.push([495, "admit", "ui attempt 2 admitted", "slot 2 · codex/writer · same generation", "ui"]);
    }
    if (st === "cancelled") add([[422, "result", "ui result accepted · 2 files sealed", "matched output_schema · artifact hash-verified", "ui"], [430, "user", "You cancelled the run", "authority revoked immediately"], [436, "cancel", "storage stop verified", "slot 1 released · usage estimated", "storage"], [437, "run", "Run cancelled", "Retry and Apply are unavailable"]]);
    return E.filter((e) => e[0] <= now).sort((a, b) => a[0] - b[0]).map(([at, kind, text, sub, task, turn]) => ({ at, kind, text, sub, task, turn }));
  }

  /* ── Status vocabulary ─────────────────────────────────────── */
  const ST = {
    running: ["work", "loader", "running"], waiting: ["mon", "hourglass", "waiting"], queued: ["muted", "circle", "queued"], succeeded: ["ok", "check", "done"], completed: ["ok", "check", "completed"],
    failed: ["bad", "x", "failed"], unknown: ["bad", "help", "unknown"], cancelled: ["muted", "ban", "cancelled"], stopping: ["work", "stop", "stopping"], paused: ["work", "pause", "paused"], blocked: ["bad", "alert", "blocked"]
  };
  // label "" renders icon-only (callers then supply the status word nearby or in an aria-label).
  function stat(s, label) { const [c, i, w] = ST[s] || ST.queued; return `<span class="st ${c}">${ic(i, 13, s === "running" ? "spin" : "")}${label === "" ? "" : esc(label || w)}</span>`; }
  const route = (id) => ROUTES.find((r) => r.id === id);
  function tokLine(a) {
    const tk = a.tok || {}, out = [];
    if (tk.obs) out.push(`<b>${k(tk.obs)}</b> observed`);
    if (tk.est) out.push(`${k(tk.est)} estimated`);
    if (tk.res && (a.end === null || a.status === "unknown")) out.push(`${k(tk.res)} reserved`);
    if (tk.unknown) out.push("final count unknown");
    if (!out.length) out.push("usage pending");
    return `<span class="tok">${out.join(" · ")}</span>`;
  }
  function usageLine(u) {
    return `<span class="tok"><b>${k(u.obs)}</b> observed${u.est ? ` · ${k(u.est)} estimated` : ""}${u.res ? ` · ${k(u.res)} reserved` : ""}${u.unknown ? " · some accounting unknown" : ""} · budget 400k</span>`;
  }

  /* ── Shared renderers ──────────────────────────────────────── */
  const mockTag = '<span class="mock" title="Illustrative control. Nothing is sent anywhere.">MOCK</span>';
  function runControls(s) {
    const live = ["running", "paused", "unknown"].includes(s.status);
    let h = "";
    if (s.status === "running" || s.status === "unknown") h += `<button class="btn sm" data-act="pause" data-fk="ctl-pause">${ic("pause", 12)}Pause</button>`;
    if (s.status === "paused") h += `<button class="btn sm" data-act="resume" data-fk="ctl-resume">${ic("play", 12)}Resume</button>`;
    if (live) h += `<button class="btn sm" data-act="cancel-run" data-fk="ctl-cancel">${ic("stop", 12)}Cancel run</button>`;
    return h ? `<div class="row-acts" role="group" aria-label="Run controls (illustrative)">${h}${mockTag}</div>` : "";
  }
  function runMeta(s) {
    const statusWord = { running: "running", paused: "paused · draining", unknown: "needs attention", completed: "completed", cancelled: "cancelled" }[s.status];
    return { statusWord, html: `${stat(s.status === "unknown" ? "unknown" : s.status, statusWord)}` };
  }
  function taskActions(tk, s) {
    const b = [], cancelled = s.status === "cancelled";
    const btn = (act, icon, label, dis, why) => b.push(`<button class="btn sm" data-act="${act}" data-arg="${tk.id}" data-fk="ta-${act}-${tk.id}" ${dis ? `disabled aria-describedby="why-${act}-${tk.id}"` : ""}>${ic(icon, 12)}${label}</button>${dis ? `<span class="why" id="why-${act}-${tk.id}">${esc(why)}</span>` : ""}`);
    if (tk.status === "unknown") { btn("reconcile", "shield", "Reconcile task"); btn("retry", "rotate", "Retry task", true, "Available after Reconcile settles the outcome."); }
    else if (["running", "waiting", "queued"].includes(tk.status) && !cancelled) { btn("message", "msg", "Message task"); btn("cancel-task", "stop", "Cancel task"); if (tk.status === "queued") btn("retire", "archive", "Retire task"); }
    else if (["failed", "cancelled"].includes(tk.status)) btn("retry", "rotate", "Retry task", cancelled, "Unavailable: the run was cancelled.");
    return b.length ? `<div class="row-acts" role="group" aria-label="Task controls (illustrative)">${b.join("")}${mockTag}</div>` : "";
  }
  function taskLink(id, s) { const tk = s.tasks[id]; return tk ? `<button class="chip" data-act="select" data-arg="${id}" data-fk="dep-${id}" aria-label="Open ${esc(tk.title)}, ${ST[tk.status][2]}">${ic(ST[tk.status][1], 11, tk.status === "running" ? "spin" : "")}${id}</button>` : `<span class="chip">${id} · not created</span>`; }

  function taskDetail(id, s, opts = {}) {
    const tk = s.tasks[id]; if (!tk) return `<div class="td"><p class="muted">This task no longer exists in this state.</p></div>`;
    const r = route(tk.current ? tk.current.route : tk.route);
    const art = s.artifacts.find((a) => a.task === id);
    const res = tk.result ? JSON.stringify(tk.result, null, 2) : null;
    const total = tk.attempts.length;
    return `<div class="td" data-task="${id}">
      <div class="td-head">${opts.back ? `<button class="btn ghost sm" data-act="back" data-fk="back" style="margin:-4px 0 6px -8px">${ic("left", 12)}${esc(opts.back)}</button>` : ""}
        <div style="display:flex;gap:10px;align-items:baseline"><h3 tabindex="-1" id="td-title" style="flex:1">${esc(tk.title)}</h3>${stat(tk.status)}</div>
        <div class="line"><span>${id}</span><span>gen ${tk.gen}</span><span>attempt ${total || 0}${total ? ` of ${total}` : ""}</span><span>depth ${tk.depth}</span><span>${tk.parent ? "parent " + tk.parent : "root task"}</span>${tk.required !== false ? "<span>required</span>" : ""}</div></div>
      ${tk.status === "unknown" ? `<div class="banner bad" role="note"><b>${ic("help", 13)} Execution not confirmed stopped</b><span class="sm">${esc(tk.current.error)}</span><span class="why">Reconcile checks trusted runtime identity (boot ID, process start, process group). It cannot clear the hold just because you clicked it; if evidence is missing the hold stays.</span></div>` : ""}
      ${tk.needsRepair ? `<div class="banner work" role="note"><b>Required work was cancelled</b><span class="sm">Its dependents still need a valid result. Retry admits a new attempt in the same generation; a coordinator replace would start generation 2.</span></div>` : ""}
      ${taskActions(tk, s)}
      <dl class="kv">
        <dt>Route</dt><dd><span class="mono lbl">${esc(r.id)}</span> <span class="muted lbl">· ${r.provider} · provider default model · ${r.effort} effort</span></dd>
        <dt>Access</dt><dd class="lbl">${tk.access === "write" ? "write" : "read only"} · <span class="mono">${tk.scope.join(", ")}</span></dd>
        <dt>Needs</dt><dd>${tk.deps.length ? tk.deps.map((d) => taskLink(d, s)).join(" ") : '<span class="muted lbl">nothing — starts when admitted</span>'}</dd>
        <dt>Needed by</dt><dd>${tk.dependents.length ? tk.dependents.map((d) => taskLink(d, s)).join(" ") : `<span class="muted lbl">${id === "coordinator" ? "the run result" : "nothing yet"}</span>`}</dd>
        ${tk.waitingFor ? `<dt>Waiting for</dt><dd>${tk.waitingFor.map((d) => taskLink(d, s)).join(" ")} ${tk.status === "waiting" ? '<span class="muted lbl">· yielded, holds no slot</span>' : ""}${tk.held ? ` <span class="work lbl">· ${esc(tk.held)}</span>` : ""}</dd>` : ""}
      </dl>
      <div class="sec"><div class="eyebrow">Attempts</div>${total ? `<ul class="att">${tk.attempts.map((a) => `<li><div class="a1">${stat(a.status === "waiting" ? "waiting" : a.status, a.status === "waiting" ? "yielded" : null)}<span>#${a.n}</span><span class="mono lbl">${a.route}</span><span class="muted lbl">gen ${a.gen}${a.slot ? ` · slot ${a.slot}` : ""}</span></div>
          <div class="a2">${t(a.start)}–${a.end === null ? "now" : t(a.end)} · ${tokLine(a)}</div>${a.note ? `<div class="lbl muted">${esc(a.note)}</div>` : ""}${a.error ? `<div class="${a.status === "failed" || a.status === "unknown" ? "err" : "lbl muted"}">${esc(a.error)}</div>` : ""}</li>`).join("")}</ul>` : `<p class="lbl muted" style="margin:0">${tk.note || "Not admitted yet."}</p>`}</div>
      <div class="sec"><div class="eyebrow">Result</div>${res ? `<p class="lbl muted" style="margin:0">Typed result, checked against the task's output_schema by CodeMux. Agent prose is not accepted as a result.</p><pre class="out" tabindex="0" aria-label="Result JSON">${esc(res)}</pre>` : `<p class="lbl muted" style="margin:0">No accepted result${tk.status === "failed" ? " — the last attempt's result was rejected" : " yet"}.</p>`}
        ${id === "tests" && res ? `<p class="notice work">This is a plan. Managed workers have no shell, build or test commands, so none of these cases were run.</p>` : ""}</div>
      ${art ? `<div class="sec"><div class="eyebrow">Retained changes</div><div class="row-acts"><button class="btn sm" data-act="files" data-arg="${art.id}" data-fk="files-${art.id}">${ic("diff", 12)}Review ${art.files.length} changed files</button><span class="mono cap muted">${esc(art.digest)}</span></div></div>` : ""}
      ${tk.messages ? `<div class="sec"><div class="eyebrow">Guidance</div>${tk.messages.map((m) => `<p class="sm" style="margin:0">${esc(m)}</p>`).join("")}</div>` : ""}
      ${disclosure("prompt-" + id, "Task prompt", `<p class="sm muted" style="margin:4px 0 0;user-select:text">${esc(tk.prompt)}</p>`, s)}
    </div>`;
  }
  function disclosure(key, label, body) {
    const open = K.S.open.has(key);
    return `<div><button class="disc" aria-expanded="${open}" aria-controls="dc-${key}" data-act="toggle" data-arg="${key}" data-fk="dc-${key}">${ic("right", 12, "chev")}${label}</button><div id="dc-${key}" ${open ? "" : "hidden"}>${body}</div></div>`;
  }

  function ledger(s) {
    const done = (id) => s.tasks[id] && s.tasks[id].status === "succeeded";
    const typed = ["arch", "storage", "ui", "tests", "synthesis", "coordinator"].filter(done);
    const host = [`${typed.length} typed result${typed.length === 1 ? "" : "s"} matched output_schema${typed.length ? ` <span class="src">(${typed.join(", ")})</span>` : ""}`];
    host.push(`1 result rejected <span class="src">(storage gen 1 — repaired as gen 2)</span>`);
    if (s.artifacts.length) host.push(`${s.artifacts.length} artifact${s.artifacts.length > 1 ? "s" : ""} sealed after verified stop; manifest hashes match retained files`, "every changed path is inside its task's write scope");
    const agent = done("synthesis") ? ["storage and ui agree on the three recovery states", "test plan covers corrupt, missing and v1 snapshots"] : [];
    const not = ["No tests, builds or type checks ran: managed workers have no shell/build/test commands.", "Source workspace is only rechecked when you Apply."];
    if (s.usage.unknown) not.push("Token accounting is incomplete for at least one attempt; totals include estimates.");
    return `<div class="ledger">
      <div><h4>${ic("shield", 13, "ok")}Checked by CodeMux <span class="src">deterministic</span></h4><ul>${host.map((x) => `<li>${x}</li>`).join("")}</ul></div>
      <div><h4>${ic("eye", 13, "mon")}Reviewed by an agent <span class="src">judgment, synthesis task</span></h4>${agent.length ? `<ul>${agent.map((x) => `<li>${x}</li>`).join("")}</ul>` : `<p class="lbl muted" style="margin:0 0 0 20px">Not yet. The coordinator adds a synthesis task after the children finish.</p>`}</div>
      <div><h4>${ic("alert", 13, "work")}Not verified</h4><ul>${not.map((x) => `<li>${x}</li>`).join("")}</ul></div></div>`;
  }

  function filesView(artId, s, opts = {}) {
    const art = s.artifacts.find((a) => a.id === artId) || Object.assign({ id: artId, canApply: false, reason: "This artifact is not current in this state." }, ARTIFACTS[artId]);
    const sel = K.S.file[artId] || art.files[0].path, f = art.files.find((x) => x.path === sel) || art.files[0];
    const kind = { modified: ["work", "M"], added: ["ok", "A"], deleted: ["bad", "D"] };
    const pane = (lbl, txt, empty) => `<div><div class="eyebrow" style="margin-bottom:4px">${lbl}</div>${txt === null ? `<p class="notice">${empty}</p>` : `<pre class="out" tabindex="0" aria-label="${lbl}">${esc(txt)}</pre>`}</div>`;
    const outcome = K.S.applied[artId];
    return `<div class="fr">
      ${opts.back ? `<button class="btn ghost sm" data-act="back" data-fk="back" style="justify-self:start;margin:-4px 0 0 -8px">${ic("left", 12)}${esc(opts.back)}</button>` : ""}
      <div><h3 tabindex="-1" id="td-title" style="margin:0;font-size:var(--t-body-lg);font-weight:600">Review file changes</h3>
        <div class="mono cap muted" style="margin-top:3px">${esc(art.label)} · ${art.files.length} files · ${esc(art.digest)}</div></div>
      <p class="lbl muted" style="margin:0">Retained snapshot from a successful attempt. Nothing touches your workspace until you apply it.</p>
      <div class="flist" role="listbox" aria-label="Changed files" data-fk-group="files-${artId}">
        ${art.files.map((x) => `<div class="fitem" role="option" aria-selected="${x.path === f.path}" tabindex="${x.path === f.path ? 0 : -1}" data-act="file" data-arg="${artId}|${x.path}" data-fk="f-${x.path}">
          <span class="k mono ${kind[x.change][0]}" aria-hidden="true">${kind[x.change][1]}</span><span class="p">${esc(x.path)}</span><span class="k muted">${x.change}${x.binary ? " · binary" : ""}${x.truncated ? " · truncated" : ""}</span></div>`).join("")}
      </div>
      ${f.binary ? `<p class="notice">Binary file — text preview unavailable. Added ${esc(f.path.split("/").pop())}.</p>` : `<div class="ba ${K.S.width === "expanded" ? "side" : ""}">${pane("Before · workspace baseline", f.before, "File did not exist.")}${pane("After · retained change", f.after, "File deleted.")}</div>`}
      ${f.truncated ? `<p class="notice work">Preview is limited to 64 KiB per side. The complete file remains in the retained artifact.</p>` : ""}
      ${disclosure("hash-" + f.path, "File hashes", `<pre class="out">before_sha256  ${f.b || "— (did not exist)"}\nafter_sha256   ${f.a || "— (deleted)"}</pre>`)}
      <div class="apply">
        ${art.canApply ? `<p class="lbl muted" style="margin:0">Apply rechecks the task generation, accepted digest, scope and each source file right before writing. Multi-file apply is not atomic; a conflict stops it.</p>` : `<p class="notice bad">${esc(art.reason)}</p>`}
        <div class="row-acts"><button class="btn ${art.canApply ? "solid" : ""}" data-act="apply" data-arg="${artId}" data-fk="apply-${artId}" ${art.canApply && !outcome ? "" : "disabled"} ${art.canApply ? "" : 'aria-describedby="apply-why"'}>${ic("check", 12)}Apply changes</button>${mockTag}${art.canApply ? "" : '<span class="sr-only" id="apply-why">Apply is disabled. See the reason above.</span>'}</div>
        ${outcome ? `<p role="status" class="notice ${outcome.ok ? "" : "bad"}">${outcome.text}</p>` : ""}
      </div></div>`;
  }

  const SCRIPT = `await workflow.phase("Plan");
// The coordinator adds children with graph tools,
// yields its slot while they run, and resumes
// with their typed outcomes. Children may only
// narrow this scope, never widen it.
return await agent(goal, {
  id: "coordinator",
  title: "Coordinate session recovery",
  route_id: "claude/coordinator",
  access: "write",
  scope: ["src/session", "src/ui/recovery"],
  output_schema: { type: "object",
    required: ["summary", "accepted", "follow_up"] }
});`;
  const SAVED = ["session-recovery.js · saved 2 days ago", "review-fanout.js · starter", "dependency-audit.js"];
  const CEILINGS = ["Auto (up to 4)", "2 workers", "4 workers", "8 workers"];
  // Draft inputs live here so re-renders (route/write toggles, resize) never reset them.
  const DRAFT = { routes: [], writes: false, notice: false, name: "Refactor session recovery",
    goal: "Refactor session recovery so a crashed session restores its newest readable snapshot and explains what it restored.",
    saved: SAVED[0], js: SCRIPT, cc: "2 workers", tb: "24", ab: "48", nd: "2", tk: "400000", ob: "262144", tl: "45" };
  // Escaped, one-line echo of the draft for illustrative feedback.
  function draftSummary() {
    const D = K.S.draft, name = D.name.trim() || "Untitled run", goal = D.goal.trim().replace(/\s+/g, " ");
    return `“${esc(name)}” · goal: ${goal ? esc(goal.length > 90 ? goal.slice(0, 90) + "…" : goal) : "(empty)"} · limits: ${esc(D.cc)}, ${esc(D.tb)} tasks, ${esc(D.ab)} attempts, depth ${esc(D.nd)}, ${esc(D.tk)} tokens, ${esc(D.ob)} output bytes, ${esc(D.tl)} min · file changes ${D.writes ? "allowed" : "off"}`;
  }

  function setupView(s, variant = {}) {
    const D = K.S.draft;
    const liveOk = D.routes.every((r) => !DRY_ONLY.includes(r));
    const block = (title, body) => `<section class="sec" aria-label="${title}"><div class="eyebrow">${title}</div>${body}</section>`;
    const num = (label, key, fk, extra) => `<label class="field"><span>${label}</span><input class="inp" type="number" min="1" value="${esc(D[key])}" data-draft="${key}" data-fk="${fk}"${extra || ""}></label>`;
    const sections = {
      goal: block("Run", `<label class="field"><span>Run name</span><input class="inp" id="su-name" value="${esc(D.name)}" data-draft="name" data-fk="su-name"></label>
        <label class="field"><span>Goal</span><textarea class="inp" rows="3" id="su-goal" data-draft="goal" data-fk="su-goal">${esc(D.goal)}</textarea></label>`),
      script: block("Script", `<label class="field"><span>Saved script</span><select class="inp" id="su-saved" data-draft="saved" data-fk="su-saved">${SAVED.map((o) => `<option${o === D.saved ? " selected" : ""}>${esc(o)}</option>`).join("")}</select></label>
        <label class="field"><span>Workflow JavaScript</span><textarea class="inp code-in" rows="${variant.tallScript ? 14 : 9}" spellcheck="false" id="su-js" data-draft="js" data-fk="su-js">${esc(D.js)}</textarea></label>
        <div class="row-acts"><button class="btn sm" data-act="save-script" data-fk="su-save">Save script</button>${mockTag}<span class="why">Scripts run in a sandbox with no network, files or timers.</span></div>`),
      routes: block("Provider routes", `<div class="routes" role="group" aria-label="Routes">${ROUTES.map((r) => `<label class="route"><input type="checkbox" checked disabled aria-describedby="rt-live"><span><span class="mono lbl">${r.id}</span></span><span class="cap ok">live + dry run</span><span class="sub">${r.provider} · provider default model · ${r.effort} effort</span></label>`).join("")}
        ${DRY_ONLY.map((p) => `<label class="route"><input type="checkbox" data-act="dryroute" data-arg="${p}" data-fk="dr-${p}" ${D.routes.includes(p) ? "checked" : ""}><span><span class="mono lbl">${p.toLowerCase()}/reviewer</span></span><span class="cap muted">dry run only</span><span class="sub">${p} adapter lacks managed isolation and verified stop; never used for live runs</span></label>`).join("")}</div>
        <p class="why" id="rt-live" style="margin:0">Live support: Claude and Codex on Linux in local Git workspaces. Labels here are illustrative and do not prove a provider works.</p>`),
      limits: block("Limits", `<div class="limits">
        <label class="field"><span>Worker ceiling</span><select class="inp" data-draft="cc" data-fk="su-cc">${CEILINGS.map((o) => `<option${o === D.cc ? " selected" : ""}>${o}</option>`).join("")}</select></label>
        ${num("Task budget", "tb", "su-tb")}${num("Attempt budget", "ab", "su-ab")}${num("Nesting depth", "nd", "su-nd")}${num("Token budget", "tk", "su-tk")}
        ${num("Output size limit (bytes)", "ob", "su-ob", ' aria-describedby="ob-why"')}${num("Time limit (min)", "tl", "su-tl")}</div>
        <p class="why" style="margin:0">Host ceiling: 8 workers globally. Workers that are waiting yield their slot. Admitted running, stopping and unknown attempts count toward the ceiling.</p>
        <p class="why" id="ob-why" style="margin:0">Output size limit is the runtime's <span class="mono">max_output_bytes</span>: the most output CodeMux keeps from one attempt. The value shown is illustrative.</p>`),
      writes: `<label class="optin"><input type="checkbox" data-act="writes" data-fk="su-w" ${D.writes ? "checked" : ""}><span><b style="font-weight:600">Allow file changes</b><span class="why" style="display:block;margin-top:2px">Off until you check it. Writer tasks edit isolated copies inside their scope (<span class="mono">src/session</span>, <span class="mono">src/ui/recovery</span>). Changes stay retained until you review and apply them. A script can't turn this on.</span></span></label>`,
      launch: `<div class="sec" style="border-top:1px solid var(--hairline);padding-top:12px"><div class="row-acts"><button class="btn" data-act="launch" data-arg="dry" data-fk="su-dry">${ic("play", 12)}Dry run</button><button class="btn solid" data-act="launch" data-arg="live" data-fk="su-live" ${liveOk ? "" : 'disabled aria-describedby="live-why"'}>${ic("play", 12)}Run with agents</button>${mockTag}</div>
        ${liveOk ? `<p class="why" style="margin:0">A dry run exercises the scheduler with zero inference usage. Run with agents uses the live routes above.</p>` : `<p class="why bad" id="live-why" style="margin:0">Run with agents is unavailable: ${D.routes.join(", ")} ${D.routes.length > 1 ? "are" : "is"} dry run only. Remove ${D.routes.length > 1 ? "them" : "it"} or choose Dry run.</p>`}
        ${D.writes ? "" : `<p class="why" style="margin:0">File changes are off, so this draft can only run read-only tasks. The example script gives the coordinator write access and plans writer children (storage, ui), so it won't start until you check Allow file changes. The running states in this mockup show a run that already opted in.</p>`}
        ${D.notice && !D.writes ? `<p class="notice work" role="status" style="margin:0">Not started. ${D.notice === "dry" ? "Dry run" : "Run with agents"} was blocked because Allow file changes is off and this fictional example needs writers. Check Allow file changes to start it, or change the script's <span class="mono">access</span> to <span class="mono">"read_only"</span> for an audit-only run. Your draft is kept: ${draftSummary()}.</p>` : ""}</div>`
    };
    const order = variant.order || ["goal", "script", "routes", "limits", "writes", "launch"];
    return `<div class="setup">${variant.head || `<h3 tabindex="-1" id="td-title">New workflow</h3>`}${order.map((x) => sections[x]).join("")}</div>`;
  }

  /* ── Shell ─────────────────────────────────────────────────── */
  function shell(meta) {
    const embed = /embed=1/.test(location.hash);
    document.body.classList.add("mk");
    if (embed) document.body.classList.add("embed");
    document.body.insertAdjacentHTML("afterbegin", `
    <div class="rs" role="region" aria-label="Review controls (not part of the app)">
      <a href="index.html">${ic("left", 12)}All directions</a>
      <span class="rs-title"><b>Mockup ${meta.letter}</b> · ${meta.name} <span class="muted">— ${meta.tagline}</span></span>
      <span class="rs-tag">Illustrative · no backend · not integrated</span>
      <div class="seg" role="group" aria-label="Scenario state"><span>State</span>${Object.entries(STATES).map(([id, l]) => `<button data-act="state" data-arg="${id}" aria-pressed="false">${l}</button>`).join("")}</div>
      <div class="seg" role="group" aria-label="Pane width"><span>Pane</span><button data-act="width" data-arg="narrow" aria-pressed="false">Narrow 360</button><button data-act="width" data-arg="normal" aria-pressed="false">Normal 500</button><button data-act="width" data-arg="expanded" aria-pressed="false">Expanded</button></div>
    </div>
    <div class="app" id="app">
      <aside class="sb" aria-label="Projects (static)">
        <div class="sb-top">${ic("panelL", 15)}</div>
        <div class="sb-row">${ic("folder", 14, "muted")}<span class="grow">All projects</span>${ic("search", 14, "muted")}${ic("pen", 14, "muted")}</div>
        <div class="sb-list">
          <div class="ws on"><div class="ws-p"><i>H</i>harbor-notes<span class="ws-s work" id="sb-st"></span></div><div class="ws-n">session-recovery</div><div class="ws-b">refactor/session-recovery</div></div>
          <div class="ws"><div class="ws-p"><i>H</i>harbor-notes</div><div class="ws-n muted">main</div></div>
          <div class="ws"><div class="ws-p"><i>T</i>tidepool-cli</div><div class="ws-n muted">main</div></div>
          <div class="ws"><div class="ws-p"><i>Q</i>quill-site</div><div class="ws-n muted">draft/pricing-copy</div><div class="ws-b">draft/pricing-copy</div></div>
        </div>
        <div class="sb-foot">${ic("settings", 15)}${ic("history", 15)}${ic("branch", 15)}</div>
      </aside>
      <main class="chat" aria-label="Agent chat (static)">
        <div class="chat-band"><span class="ctab on">${ic("msg", 12)}Agent Chat <span class="dot ok"></span></span><span class="ctab">${ic("plus", 12)}</span></div>
        <div class="chat-log">
          <div class="msg-u sm">Refactor session recovery so a crash restores the newest readable snapshot. Use a workflow — Claude and Codex, two workers max.</div>
          <div class="msg-a"><p class="sm">Started <b>Refactor session recovery</b> as a workflow. A coordinator will split the work and repair failures; file changes stay retained until you review them.</p>
            <div class="wf-card">${ic("network", 15, "muted")}<span class="t"><b>Refactor session recovery</b><span class="mono cap muted" id="chat-st"></span></span><button class="btn sm" data-act="focus-pane" data-fk="chat-open">Open in Workflows</button></div></div>
          <div class="msg-meta">${ic("check", 11)} illustrative transcript · this mockup calls no models</div>
        </div>
        <div class="composer"><label for="cmp" class="sr-only">Message (disabled in mockup)</label><input id="cmp" class="sm" disabled placeholder="Reply or steer the agent…" style="flex:1;border:0;background:transparent;min-width:0"><button class="ib" data-act="focus-pane" aria-label="Open Workflows pane" data-fk="cmp-wf">${ic("network", 14)}</button><button class="ib" disabled aria-label="Send (disabled in mockup)">${ic("up", 14)}</button></div>
        <div class="composer-meta"><span>harbor-notes</span><span>·</span><span>refactor/session-recovery</span><span>·</span><span>local Git</span></div>
      </main>
      <section class="deck" aria-label="Right panel">
        <div class="deck-head">
          <div class="deck-tabs"><div role="tablist" aria-label="Panes" style="display:flex;gap:2px">
            <button class="dtab" role="tab" id="tab-changes" aria-selected="false" tabindex="-1" data-act="tab" data-arg="changes">${ic("diff", 13)}Changes <span class="badge">0</span></button>
            <button class="dtab" role="tab" id="tab-review" aria-selected="false" tabindex="-1" data-act="tab" data-arg="review">${ic("branch", 13)}Review</button>
            <button class="dtab" role="tab" id="tab-workflows" aria-selected="true" tabindex="0" data-act="tab" data-arg="workflows">${ic("network", 13)}Workflows <span class="badge acc" id="tab-badge"></span></button></div>
            <div class="acts"><button class="ib" data-act="setup" aria-label="New workflow" data-fk="tab-new">${ic("plus", 13)}</button></div></div>
          <div class="deck-ctl"><button class="ib" data-act="expand" aria-pressed="false" aria-label="Expand panel" data-fk="band-expand">${ic("max", 13)}</button><span class="ib" aria-hidden="true">${ic("panelR", 14)}</span><span class="win" aria-hidden="true">${ic("minus", 13)}${ic("sq", 12)}${ic("x", 13)}</span></div>
        </div>
        <div class="deck-body" id="pane" tabindex="-1" role="tabpanel" aria-labelledby="tab-workflows" data-dir="${meta.letter.toLowerCase()}"></div>
        <footer class="deck-foot" id="foot"></footer>
      </section>
    </div>
    <div class="toast" id="toast" role="status" aria-live="polite"></div>`);
  }

  /* ── State, render, events ─────────────────────────────────── */
  const K = {
    S: { state: "running", width: "normal", tab: "workflows", view: "run", sel: null, artifact: null, open: new Set(), file: {}, applied: {}, reconciled: false, retried: false, draft: DRAFT, extra: {} },
    ic, esc, t, k, stat, route, tokLine, usageLine, taskDetail, filesView, setupView, ledger, runControls, runMeta, disclosure, taskLink, snapshot, ST, mockTag, TASKS, ROUTES,
    mount(meta) { this.meta = meta; shell(meta); readHash(); bind(); this.render(); },
    snap() { return snapshot(this.S); },
    render() {
      const S = this.S, s = this.snap(), pane = document.getElementById("pane"), ae = document.activeElement, fk = ae && ae.dataset ? ae.dataset.fk : null;
      const deck = document.querySelector(".deck"), app = document.getElementById("app");
      let caret = null; try { if (ae && ae.dataset && ae.dataset.draft && ae.selectionStart != null) caret = [ae.selectionStart, ae.selectionEnd]; } catch (_) { caret = null; }
      const row = app.clientWidth - 256, maxW = Math.max(360, Math.min(row * 0.75, row - 240));
      app.style.setProperty("--pane-w", (S.width === "narrow" ? 360 : S.width === "expanded" ? Math.round(maxW) : 500) + "px");
      deck.dataset.width = S.width;
      document.querySelectorAll('.rs [data-act="state"]').forEach((b) => b.setAttribute("aria-pressed", b.dataset.arg === S.state));
      document.querySelectorAll('.rs [data-act="width"]').forEach((b) => b.setAttribute("aria-pressed", b.dataset.arg === S.width));
      const ex = document.querySelector('[data-act="expand"]'); ex.setAttribute("aria-pressed", S.width === "expanded"); ex.setAttribute("aria-label", S.width === "expanded" ? "Restore panel width" : "Expand panel"); ex.innerHTML = ic(S.width === "expanded" ? "min" : "max", 13);
      document.querySelectorAll(".dtab").forEach((b) => { const on = b.dataset.arg === S.tab; b.setAttribute("aria-selected", on); b.tabIndex = on ? 0 : -1; });
      pane.setAttribute("aria-labelledby", "tab-" + S.tab);
      const word = S.state === "setup" ? "draft" : runMeta(s).statusWord;
      document.getElementById("chat-st").textContent = S.state === "setup" ? "not started · draft" : `${word} · ${s.busy}/2 workers · ${s.phase} · illustrative run, file changes already opted in`;
      document.getElementById("sb-st").innerHTML = S.state === "setup" ? "" : stat(s.status === "unknown" ? "unknown" : s.status, s.status === "unknown" ? "Needs you" : s.status === "completed" ? "Review" : word.split(" ")[0]);
      document.getElementById("tab-badge").textContent = S.state === "setup" ? "" : s.needs.length ? "!" + s.needs.length : `${s.busy}/2`;
      document.getElementById("foot").innerHTML = `<span>${S.state === "setup" ? "workflows · draft" : `workflows · ${word} · ${s.busy}/2 workers`}</span><span class="mocktag">· mock data${S.state === "setup" ? "" : '<span class="opt"> · illustrative run, writes opted in</span>'}</span><span class="r">${S.state === "setup" ? "no usage yet" : `Σ ${k(s.usage.obs)} obs${s.usage.est ? " + " + k(s.usage.est) + " est" : ""}`}</span>`;
      if (S.tab !== "workflows") pane.innerHTML = `<div class="ph">The ${S.tab === "changes" ? "Changes" : "Review"} pane is unchanged by this proposal. <button class="btn sm" data-act="tab" data-arg="workflows" data-fk="ph-back">Back to Workflows</button></div>`;
      else if (S.state === "setup") pane.innerHTML = this.meta.setup ? this.meta.setup(s) : setupView(s);
      else pane.innerHTML = this.meta.render(s);
      if (this.meta.after) this.meta.after(s);
      if (fk) { const el = document.querySelector(`[data-fk="${CSS.escape(fk)}"]`); if (el && !el.disabled) { el.focus({ preventScroll: true }); if (caret) try { el.setSelectionRange(caret[0], caret[1]); } catch (_) { /* number inputs have no caret */ } } else if (this._focusTitle !== false) focusTitle(); }
      if (this._pendingFocus) { const el = document.querySelector(this._pendingFocus); this._pendingFocus = null; if (el) el.focus(); }
    },
    focusAfter(sel) { this._pendingFocus = sel; },
    toast(text) { const el = document.getElementById("toast"); el.innerHTML = `${mockTag}<span>${text}</span>`; el.classList.add("on"); clearTimeout(this._tt); this._tt = setTimeout(() => el.classList.remove("on"), 5200); }
  };
  function focusTitle() { const h = document.getElementById("td-title"); if (h) h.focus({ preventScroll: false }); }
  function readHash() {
    const h = new URLSearchParams(location.hash.slice(1));
    if (STATES[h.get("state")]) { K.S.state = h.get("state"); }
    if (["narrow", "normal", "expanded"].includes(h.get("width"))) K.S.width = h.get("width");
  }
  function resetRun() { Object.assign(K.S, { view: "run", sel: null, artifact: null, applied: {}, reconciled: false, retried: false }); }

  const ACT = {
    state(a) { K.S.state = a; resetRun(); },
    width(a) { K.S.width = a; },
    expand() { K.S.width = K.S.width === "expanded" ? "normal" : "expanded"; },
    tab(a) { K.S.tab = a; },
    "focus-pane"() { K.S.tab = "workflows"; K.focusAfter("#tab-workflows"); },
    setup() { K.S.state = "setup"; resetRun(); K.focusAfter("#td-title"); },
    select(a) { K.S.sel = a; K.S.view = "task"; K.focusAfter(K.meta.focusOnSelect || "#td-title"); },
    back() { const from = K.S.view; K.S.view = from === "files" && K.S.sel ? "task" : "run"; if (K.S.view === "run") K.focusAfter(K.S.sel ? `[data-fk="row-${K.S.sel}"]` : "#pane"); else K.focusAfter("#td-title"); },
    files(a) { K.S.artifact = a; K.S.view = "files"; K.focusAfter("#td-title"); },
    file(a) { const [art, p] = a.split("|"); K.S.file[art] = p; },
    toggle(a) { K.S.open.has(a) ? K.S.open.delete(a) : K.S.open.add(a); },
    pause() { K.S.state = "paused"; resetRun(); K.toast("Pause would stop new admissions and script operations; admitted work drains. Showing the paused state."); },
    resume() { K.S.state = "running"; resetRun(); K.toast("Resume would re-open admission. Showing the running state."); },
    "cancel-run"() { K.S.state = "cancelled"; resetRun(); K.toast("Cancel run would revoke authority immediately and hold capacity until stops are verified. Retry and Apply become unavailable."); },
    reconcile() { K.S.reconciled = true; K.toast("Reconcile would ask the host to prove the attempt stopped. Illustrative outcome shown: stop verified, slot 2 and its reservation released."); },
    retry(a) { if (a === "ui" && K.S.reconciled) { K.S.retried = true; K.toast("Retry would admit ui attempt 2 in the same generation. Shown as running in slot 2."); } else K.toast("Retry would admit a new attempt for this task."); },
    "cancel-task"(a) { K.toast(`Cancel task would revoke ${a}'s authority and stop its subtree. Required work then needs repair. Not simulated here — see the Unknown state.`); },
    retire(a) { K.toast(`Retire would withdraw ${a} while keeping its evidence. Its dependents would still need valid results.`); },
    message(a) { K.toast(`Message would queue guidance for ${a}'s next admission or continuation.`); },
    mock(a) { K.toast(a); },
    launch(a) {
      if (!K.S.draft.writes) { K.S.draft.notice = a; K.toast("Not started: Allow file changes is off, and this example needs writer tasks. See the note under the launch buttons."); return; }
      K.S.draft.notice = false; K.S.state = "running"; resetRun();
      K.toast(`${a === "dry" ? "No run was started; a dry run would use zero inference." : "No run was started and no provider was called."} Your draft: ${draftSummary()}. Showing the illustrative running example, which already opted in to file changes.`);
    },
    "save-script"() { const D = K.S.draft; K.toast(`Save script would store ${esc(D.saved.split(" · ")[0])} (${D.js.split("\n").length} lines) for this workspace with draft ${draftSummary()}.`); },
    dryroute(a) { const r = K.S.draft.routes; K.S.draft.routes = r.includes(a) ? r.filter((x) => x !== a) : r.concat(a); },
    writes() { K.S.draft.writes = !K.S.draft.writes; K.S.draft.notice = false; },
    apply(a) {
      const art = ARTIFACTS[a];
      K.S.applied[a] = art.conflict ? { ok: false, text: `Illustrative conflict: ${art.conflict} changed in your workspace after the run pinned it, so its source hash no longer matches. Apply stopped before writing anything. Review the current file, or ask the coordinator to redo ui.` }
        : { ok: true, text: `Mock only — no files were written. In the app, CodeMux would recheck generation, digest, scope and source content, then write ${art.files.length} paths.` };
    }
  };
  function bind() {
    document.addEventListener("click", (e) => {
      const el = e.target.closest("[data-act]"); if (!el || el.disabled) return;
      const a = el.dataset.act; if (el.tagName === "INPUT" && el.type === "checkbox") { ACT[a] && ACT[a](el.dataset.arg); K.render(); return; }
      if (K.meta.act && K.meta.act(a, el.dataset.arg, el) === true) { K.render(); return; }
      if (ACT[a]) { e.preventDefault(); ACT[a](el.dataset.arg); K.render(); }
    });
    document.addEventListener("keydown", (e) => {
      const el = e.target;
      const lb = el.closest && el.closest('[role="listbox"]');
      if (lb && el.getAttribute("role") === "option") {
        const opts = [...lb.querySelectorAll('[role="option"]')], i = opts.indexOf(el);
        const next = { ArrowDown: i + 1, ArrowRight: i + 1, ArrowUp: i - 1, ArrowLeft: i - 1, Home: 0, End: opts.length - 1 }[e.key];
        if (next !== undefined) { e.preventDefault(); const n = opts[Math.max(0, Math.min(opts.length - 1, next))]; opts.forEach((o) => (o.tabIndex = -1)); n.tabIndex = 0; n.focus(); return; }
        if (e.key === "Enter" || e.key === " ") { e.preventDefault(); el.click(); return; }
      }
      const tl = el.closest && el.closest('[role="tablist"]');
      if (tl && el.getAttribute("role") === "tab" && ["ArrowLeft", "ArrowRight"].includes(e.key)) {
        const tabs = [...tl.querySelectorAll('[role="tab"]')], i = tabs.indexOf(el), n = tabs[(i + (e.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length];
        e.preventDefault(); n.focus(); n.click(); return;
      }
      if (e.key === "Escape" && K.S.view !== "run" && document.getElementById("pane").contains(el)) { if (!(K.meta.act && K.meta.act("back") === true)) ACT.back(); K.render(); }
    });
    // Keep typed draft values in memory only (no storage), so any re-render restores them.
    const keep = (e) => { const key = e.target.dataset && e.target.dataset.draft; if (key && key in K.S.draft) K.S.draft[key] = e.target.value; };
    document.addEventListener("input", keep);
    document.addEventListener("change", keep);
    window.addEventListener("hashchange", () => { readHash(); resetRun(); K.render(); });
    window.addEventListener("resize", () => K.render());
  }
  window.K = K;
})();
