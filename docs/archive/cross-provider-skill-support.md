# Cross-Provider Skill Discovery And Explicit Invocation

- Purpose: Plan the provider-correct discovery and explicit invocation of skills in Agent Chat.
- Audience: Anyone changing the Codemux skills registry, provider adapters, slash popup, or Skills settings.
- Authority: Completed implementation record; current behavior lives in the feature docs.
- Update when: Provider contracts, rollout gates, or likely touch points change.
- Read next: `docs/features/agent-chat.md`, `docs/features/skills-sync.md`, `docs/features/multi-provider-chat.md`
- Status: COMPLETED (2026-08-03)

## Review Follow-up (2026-08-05)

The merge-base refresh and review hardening are included in the completed
implementation: absent provider binaries are empty catalogs; turn resolution
uses the caller's plugin scope and reuses cold forced discovery; invalid legacy
names remain visible but unavailable; the dead Codex `$name` route is removed;
wrapped Claude turns fall back to portable invocation rather than treating
prefixes as `$ARGUMENTS`; old project-skill disabled ids migrate on discovery;
preset launches refresh exact selections; and inherited project roots are
watched as well as scanned.

## Goal

Make the Agent Chat skills surface reflect what Claude Code, Codex, and OpenCode
actually expose for the current working directory, while preserving Codemux's
useful cross-provider `/skill` flow. Discovery should be automatic. Explicit
invocation should resolve one exact skill and retain its supporting-file context.
Provider-native automatic invocation should continue unchanged; Codemux must not
silently make a foreign skill model-invocable in this milestone.

## Locked Product Decisions

1. **No new global discovery toggle.** Finding installed skills is passive,
   expected behavior. Settings keeps the existing per-skill switch and the
   narrower “Include plugin-bundled skills” preference.
2. **The per-skill switch controls Codemux surfaces.** Disabled skills remain
   visible in Settings but disappear from the Codemux slash popup and cannot be
   explicitly injected by Codemux. The row must state that an active provider
   may still discover the same skill natively; Codemux does not rewrite provider
   config behind the user's back.
3. **Native auto-invocation stays native.** Claude, Codex, and OpenCode may
   automatically use skills they natively advertise according to their own
   policies and permissions.
4. **Foreign skills stay explicit.** A Claude skill selected in a Codex or
   OpenCode pane works through the Codemux `/` surface, but Codemux does not add
   its description to the target provider's model-visible automatic catalog.
5. **No permission translation.** `allowed-tools`, Claude hooks/model/effort,
   Codex dependencies, OpenCode permissions, and similar source-specific fields
   never become grants in another provider. The target session's sandbox and
   approval rules remain authoritative.
6. **Catalog metadata and portable content are different capabilities.** A
   provider may report a built-in, managed, remote, or plugin skill whose files
   Codemux cannot read. Such an entry can be shown as native-only but cannot be
   promised as cross-provider invocable.
7. **One selection means one definition.** Name-only, first-wins resolution is
   not acceptable for collisions. The selected source identity must survive
   from popup row to provider turn.

## Current Baseline

Codemux already has a provider-neutral manual path:

1. `list_skills` walks a hand-maintained set of filesystem roots.
2. The frontend exposes every active result in the `/` popup.
3. `/skill-name` is matched by name in `skill-tokens.ts`.
4. The full Markdown body is prefixed to the outgoing text by
   `applyAllPrefixes`, regardless of the active provider.

That makes foreign skills usable, but the current implementation has four
structural gaps:

- It scans one supplied project root instead of every provider's effective
  cwd/ancestor/configured sources.
- The compatibility classifier is always called with Claude as the target.
- Duplicate names collapse with a silent first-wins rule, so a row cannot
  preserve an exact source selection.
- Raw body injection loses provider-native invocation semantics and does not
  explicitly preserve the skill base directory for relative `scripts/`,
  `references/`, and `assets/` paths.

## Verified Provider Contracts

Research was checked against the installed CLIs on 2026-08-03 (Claude Code
2.1.220, Codex CLI 0.146.0, OpenCode 1.18.11) and current primary docs.

| Provider | Native discovery truth | Native activation | Codemux integration rule |
|---|---|---|---|
| Claude Code | Personal, project, ancestor/nested project, plugin, add-directory, and managed sources. The current sidecar already starts sessions with `settingSources: ["user", "project", "local"]`; `supportedCommands()` reflects the deployed CLI catalog. | Descriptions are model-visible unless invocation policy hides them; full content loads on explicit `/name` or model selection. | Keep native behavior untouched. Merge readable filesystem definitions with a live sidecar catalog only when the SDK returns enough provenance. Metadata-only entries remain native-only. |
| Codex | `skills/list` is the authority for a cwd. It includes repo ancestor roots, user/admin/system/provider roots, enabled state, errors, and runtime extra roots. The app-server emits `skills/changed`. | Native automatic selection follows Codex's own catalog/policy. Explicit turns support a structured `{type:"skill", name, path}` input item. | Add `skills/list`/`skills/changed` to the long-lived adapter and use structured skill input for exact readable paths after the Phase 0 contract probe. Do not recreate Codex precedence in Codemux. |
| OpenCode | `GET /skill?directory=<cwd>` is available in the installed server and returns `name`, `description`, `location`, and `content`. Current docs also cover cwd-to-worktree ancestor discovery, Claude/Agent-compatible roots, configured local/HTTP roots, and source precedence. | The native `skill` tool advertises compact metadata and loads one exact body on demand, subject to OpenCode skill permission and autoinvoke policy. | Add a directory-scoped client method and treat the server response as authoritative. Retain explicit body injection where the HTTP API has no stable structured invocation part, but include the resolved base directory and never translate permissions. |
| Codemux | `~/.codemux/skills` and `<scope>/.codemux/skills` are Codemux-owned portable roots. | No independent runtime; Codemux owns explicit selection. | Scan directly, validate against the Agent Skills baseline, and make these entries explicitly usable in every provider. Do not auto-advertise them in this milestone. |

Primary references:

- Claude Code: <https://code.claude.com/docs/en/slash-commands>
- Codex app-server: <https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md#skills>
- OpenCode: <https://opencode.ai/docs/skills/> and
  <https://opencode.ai/v2/docs/skills>
- Agent Skills specification: <https://agentskills.io/specification>

## Target Architecture

### 1. Separate definitions from target-provider projections

Replace the flat “one parsed file equals one universal skill” assumption with
two records:

```text
SkillDefinition
  id                    stable source identity
  source_provider       claude | codex | opencode | codemux
  source_scope          user | project | plugin | managed | admin | system | configured
  native_id             provider-reported name/id when present
  display_name
  description
  location              readable path/URL/native sentinel
  base_dir?             readable filesystem base
  body?                 only when Codemux can read portable content
  source_policy         native invocation/visibility metadata we can prove
  provenance            filesystem | provider_catalog

SkillProjection
  skill_id
  target_provider
  availability          native | explicit-portable | native-only | unavailable
  compatibility         compatible | degraded | blocked
  reasons[]
  invocation            native-command | codex-skill-item | prompt-prefix
```

`SkillDefinition` answers “what exists?” `SkillProjection` answers “what can
this active provider safely do with it?” Compatibility is therefore calculated
for every `(skill, target provider)` pair, never once against a hardcoded target.

### 2. Introduce a backend inventory service

Move orchestration out of the Tauri command into a managed
`SkillInventoryService`, cached by `(cwd, include_plugins)` with explicit
invalidation. The service merges four adapters:

- **Portable filesystem adapter** for readable Claude/Codex/OpenCode/Codemux
  roots. Walk ancestors where the provider contract requires it; canonicalize
  roots before scanning; preserve the source provider and scope.
- **Claude catalog adapter** through the existing sidecar. Extend the current
  live command probe only if the pinned SDK response can distinguish skills and
  supply stable identity/provenance. Otherwise the filesystem inventory remains
  the cross-provider Claude source and unmatched catalog entries stay in the
  provider-native command group.
- **Codex catalog adapter** through the active app-server using `skills/list`.
  Subscribe to `skills/changed` and invalidate the matching cwd inventory.
- **OpenCode catalog adapter** using `GET /skill?directory=<cwd>`. The shared
  server must always receive the requested directory so project skills cannot
  bleed between workspaces.

Merge rules:

1. Canonical readable path is the strongest identity.
2. Provider-native `(provider, native_id, location)` is the fallback identity.
3. Provider catalogs own precedence and enabled state inside that provider.
4. Filesystem results add portable content; they do not override a provider's
   reported winner.
5. Built-in/system/managed/remote entries without readable content are
   `native-only`, never copied or prompt-injected into a foreign provider.
6. Per-adapter errors are returned alongside successful entries; one broken
   provider must not erase the rest of the registry.

### 3. Carry an exact skill reference through send

Add a target-neutral invocation record to `SendTurnInput`:

```text
SkillInvocation {
  skill_id,
  name,
  path?,
  base_dir?,
  body?,
  source_provider,
}
```

The frontend resolves slash selections to this record before send. The backend
revalidates the reference against the current inventory so a stale or disabled
selection cannot smuggle arbitrary content/path data through IPC.

Invocation behavior:

- **Native Claude skill:** keep the literal native command and allow Claude to
  apply its own frontmatter/argument behavior.
- **Native or readable Codex skill:** send a structured Codex `skill` input item
  beside the user's text. This is gated by a real adapter test proving the
  pinned app-server accepts the selected absolute path. If the contract fails,
  use the portable prefix fallback rather than emitting an ambiguous `$name`.
- **Native OpenCode skill:** allow OpenCode's exact native ID path when the
  installed API supports it. Otherwise use the portable prefix form from its
  authoritative `/skill` response.
- **Foreign portable skill:** prefix a normalized envelope containing the body,
  source/provider provenance, and absolute base directory. State that relative
  references resolve from that directory. Strip no instructions, but do not
  convert source-specific frontmatter into permissions or target settings.
- **Native-only skill in a foreign pane:** show it as unavailable with a reason;
  do not pretend it can be loaded.

The optimistic transcript continues to show what the user typed, not the
expanded body/envelope.

### 4. Make collisions explicit without making common commands ugly

Unique names keep the existing `/name` syntax. Only conflicts need a qualified
Codemux token:

```text
/claude:user:deploy
/codex:project:deploy
/opencode:configured:deploy
```

The popup inserts the qualified form for a conflict and shows provider + scope
in the row. Codemux consumes qualified tokens before provider dispatch. A
provider never receives this Codemux-only namespace as a native slash command.
If two definitions still collide on `(provider, scope, name)`, append a short,
stable identity suffix in the token while keeping the friendly display name.

Do not maintain invisible range-to-id state behind a plain textarea token; it
would become stale under arbitrary user edits and draft persistence.

### 5. Keep Settings honest and small

Do not add a new top-level or master setting. Evolve the existing Skills rows:

- Keep the enable switch, relabel/copy it as “Available in Codemux.”
- Add compact status text: `Auto in Claude`, `Manual in Codex`, `Native only`,
  `Unavailable`, or a compatibility reason.
- Explain once that native providers can still see their own skills when a
  Codemux row is disabled.
- Keep “Include plugin-bundled skills” as the one narrow discovery preference.
- Show conflicts with the exact qualified token that will be inserted.
- Preserve View/Open actions only when the content/path is readable.
- Persist disabled state by stable source identity; migrate old path-hash ids
  opportunistically when the same canonical path is rediscovered.

## Implementation Phases

### Phase 0 — Pin provider contracts before refactoring

1. Add live/fixture probes for:
   - Claude `supportedCommands()` response shape on the pinned Agent SDK,
     including whether skills can be distinguished from commands.
   - Codex `skills/list`, `skills/changed`, and a structured `skill` turn item
     using a temporary readable skill outside the native root.
   - OpenCode `GET /skill?directory`, two different cwd inventories, and the
     available explicit invocation part/endpoint (if any).
2. Store minimal scrubbed fixtures in provider adapter tests.
3. Lock version/capability fallbacks. A missing new API degrades to the current
   portable filesystem/prompt-prefix path; it does not make Agent Chat fail.

Exit gate: every planned adapter behavior is either asserted against the pinned
runtime or explicitly assigned a fallback.

### Phase 1 — Backend inventory and compatibility matrix

1. Add `SkillInventoryService`, definition/projection types, adapter error
   reporting, cwd cache, and invalidation.
2. Refactor `skills::paths` into provider-aware ancestor enumeration for the
   portable filesystem layer.
3. Implement Codex and OpenCode catalog clients; add the Claude merge only to
   the degree Phase 0 proves safe.
4. Parse the Agent Skills baseline fields plus recognized provider invocation
   policies without interpreting foreign permission grants.
5. Compute compatibility per target provider and include readable base-dir /
   supporting-file metadata.
6. Keep the skills-sync root table guard separate: configured, managed, remote,
   system, and plugin catalog entries are discovery sources, not automatically
   syncable content.

Exit gate: fixture tests prove correct cwd isolation, ancestor roots,
precedence, symlink dedupe, partial-provider failure, and target-specific
compatibility.

### Phase 2 — Exact explicit invocation

1. Add `SkillInvocation` to frontend/Rust turn types and validate it backend-side.
2. Add the Codex structured skill input variant and adapter tests.
3. Add provider-native versus portable-prefix routing for Claude/OpenCode.
4. Include base-directory context for prefix fallback and preserve arguments.
5. Replace name-first resolution with exact IDs and qualified conflict tokens.
6. Reject stale, disabled, unreadable, or target-blocked invocations with a
   user-readable error before starting the turn.

Exit gate: one native and one foreign skill run explicitly on each provider;
duplicate-name selection invokes the chosen definition; sibling resources
resolve; foreign permission metadata never changes the target permission mode.

### Phase 3 — Frontend discovery and Settings UX

1. Update `skills-store.ts` to cache inventories per cwd rather than one global
   list and consume adapter invalidation events.
2. Update slash groups, token highlighting, draft materialization, and live send
   to use exact invocations.
3. Add projection/status copy and honest toggle semantics to Settings.
4. Show provider errors without suppressing successful groups.
5. Update the dev mock with native, portable, native-only, collision, disabled,
   and degraded examples.

Exit gate: unit tests cover popup filtering, exact tokens, disabled state,
workspace switching, partial errors, and first-send/live-send parity. Visual QA
covers Settings and the slash popup at desktop and narrow widths.

### Phase 4 — Evaluation, documentation, and rollout

1. Run `npm run verify` plus focused sidecar/Codex/OpenCode adapter suites.
2. Run a live three-provider smoke matrix on Linux with the pinned versions.
3. Update `docs/features/agent-chat.md`, `docs/features/skills-sync.md`, Settings
   copy, and public docs where behavior changed.
4. Add privacy-safe diagnostics for inventory source, projection decision, and
   explicit invocation route; never log skill bodies.
5. Ship automatic discovery directly. No feature flag is needed because the
   behavior is passive and manual execution remains user-selected.

Exit gate: zero cross-workspace leakage, zero silent collision winners, no skill
body in logs, and every unavailable entry explains why.

## Future Gate: Automatic Foreign-Skill Invocation

This plan deliberately does not federate foreign skill descriptions into every
provider's model-visible catalog. Revisit only after a separate design supplies:

- a permission-aware `load_skill`/provider-tool bridge rather than full-body
  prompt mirroring;
- normalized manual-only policy (`disable-model-invocation`, Codex policy,
  OpenCode autoinvoke/permissions) with conservative precedence;
- trust handling for project skills and remote/configured sources;
- collision-safe, bounded metadata catalogs and context budgets;
- transcript evidence when Codemux automatically loads a foreign skill; and
- activation evals measuring false positives, false negatives, duplicate loads,
  latency, and context cost across Claude, Codex, and representative OpenCode
  models.

Until those gates pass, explicit `/` selection is the predictable and reversible
cross-provider contract.

## Verification Matrix

| Scenario | Claude | Codex | OpenCode |
|---|---:|---:|---:|
| User skill unique name | native + explicit | native + structured explicit | native + explicit |
| Ancestor project skill | discovered at correct cwd | discovered at correct cwd | discovered at correct cwd |
| Foreign readable skill | explicit prefix | structured item or proven fallback | explicit prefix/native endpoint |
| Native-only/managed skill in foreign pane | blocked with reason | blocked with reason | blocked with reason |
| Duplicate names | qualified exact selection | qualified exact selection | qualified exact selection |
| Disabled in Codemux | absent from Codemux popup | absent from Codemux popup | absent from Codemux popup |
| Source provider auto-invocation | unchanged | unchanged | unchanged |
| Foreign auto-invocation | not advertised | not advertised | not advertised |
| Supporting files | base dir retained | path/native loader retained | base dir retained |
| Provider catalog failure | filesystem/other providers remain | filesystem/other providers remain | filesystem/other providers remain |

## Risks And Mitigations

- **Provider API drift:** capability-detect at runtime and keep portable fallback;
  pin scrubbed fixtures to the bundled/tested versions.
- **Cross-workspace leakage:** every project catalog call is cwd-scoped; OpenCode's
  shared server must never use a process-global combined project inventory.
- **Permission escalation:** never translate foreign grants; target provider
  approval remains authoritative.
- **Prompt injection from project skills:** require the same workspace trust gate
  used for project execution before explicit invocation; render source path and
  provider before selection.
- **Body/resource drift after selection:** backend re-resolves by ID at send time
  and rejects stale identities instead of trusting frontend content.
- **Context inflation:** inject only explicitly selected portable bodies; native
  automatic catalogs remain provider-owned and bounded.
- **Unstable disabled IDs:** migrate from the current path hash to a stable
  provenance identity while preserving canonical-path matches.
- **Misleading compatibility:** distinguish `degraded` from `blocked`, attach
  concrete reasons, and test the matrix rather than using a single badge.

## Likely Touch Points

### Backend

- `src-tauri/src/skills/{mod,paths,scanner,compatibility,watcher}.rs`
- `src-tauri/src/commands/skills.rs`
- `src-tauri/src/agent_provider/types.rs`
- `src-tauri/src/agent_provider/claude/{session,slash_commands}.rs`
- `src-tauri/src/agent_provider/codex/{protocol,session}.rs`
- `src-tauri/src/agent_provider/opencode/{client,protocol,session}.rs`
- `src-tauri/src/commands/agent_chat.rs`
- `src-tauri/src/lib.rs`
- `sidecar/claude-agent/src/methods/list-commands.ts`

### Frontend

- `src/stores/skills-store.ts`
- `src/lib/agent-chat/{skill-tokens,skill-groups,mode-prefix,materialize}.ts`
- `src/components/chat/{Composer,AgentChatPane,DraftChatSurface}.tsx`
- `src/components/settings/{skills-section,skill-row}.tsx`
- `src/tauri/{types,commands}.ts`
- `src/dev/`

### Docs

- `docs/features/agent-chat.md`
- `docs/features/skills-sync.md`
- `docs/features/settings.md`
- `docs/features/multi-provider-chat.md`

## Completion Record

- Codex discovery uses the live app-server `skills/list` catalog and exact
  structured `{ type: "skill", name, path }` turn items for readable skills.
- OpenCode discovery uses `GET /skill?directory=...`; explicit portable
  invocation uses the base-directory-aware prompt envelope because the pinned
  API does not expose an equivalent structured turn item.
- Claude filesystem definitions participate in the exact inventory. Native
  provider commands remain separate because the pinned SDK command catalog
  does not expose enough skill provenance to merge them safely.
- Provider catalog failures are isolated, filesystem discovery remains usable,
  and exact ids are revalidated in the backend immediately before a turn.
- Settings and the slash popup preserve colliding definitions with qualified
  commands. The per-row switch controls Codemux only; provider-native discovery
  is unchanged.
- Verification completed with the live Codex contract, capped full Rust suite,
  full frontend suite, and browser-based Settings smoke.

## Already Landed

- Cross-provider filesystem scanner and parser.
- Lazy `/` popup registry with explicit full-body prefix injection.
- Settings grouping, conflict reporting, per-skill disable, plugin toggle,
  watcher refresh, View, and Open actions.
- Canonical-path root dedupe and exclusion of Codex `.system` content from the
  portable user scanner.
- Six user roots and five project roots covered by skills-sync lockstep tests.
