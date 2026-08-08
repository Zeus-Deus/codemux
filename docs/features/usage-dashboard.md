# Usage Dashboard

- Purpose: Describe how Codemux records agent token/cost usage and surfaces it in Settings → Usage.
- Audience: Anyone working on the usage ledger, the provider adapters' accounting paths, or the Usage settings page.
- Authority: Canonical feature-level reality doc for usage accounting.
- Update when: The ledger event, its adapter emission points, the billing classification, or the dashboard's data shape change.
- Read next: `docs/features/agent-chat.md`, `docs/features/settings.md`, `docs/reference/DESIGN-SYSTEM.md`

## What This Feature Is

A local, append-only ledger of every increment of billable work the three
agent-chat providers do, plus the Settings → Usage page that reads it.
It answers three questions: what am I actually being billed for, what
value are my subscriptions delivering, and where did the tokens go
(provider → model → subagent).

Everything is computed from a local SQLite table. Nothing is uploaded,
and no provider billing API is consulted — the figures are Codemux's own
observation of the streams it already receives.

## Current Model

### The ledger event

Adapters emit `ProviderRuntimeEvent::UsageRecorded`
(`src-tauri/src/agent_provider/events.rs`) alongside their existing
events:

```
UsageRecorded {
    thread_id, provider, model, subagent,
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
    cost_usd,
}
```

**The four token fields are non-overlapping.** Each token of work is
counted in exactly one of them. Providers do not agree on this — see the
per-adapter normalization below — so each adapter converts to this shape
before emitting. That invariant is what lets the sink sum the four
fields for a token total and lets `agent_provider::pricing` price the
split as a dot product.

### Why this is separate from `ContextUsageUpdated`

The composer's context meter has its own event, and it is unusable as an
accounting record for three independent reasons:

1. **It is a snapshot, not a delta.** It reports how full the context
   window is right now. Summing snapshots is meaningless.
2. **It is lossy on purpose.** `ContextUsageTracker` suppresses an
   emission when the new snapshot equals the last one. For a meter that
   is correct; for a ledger it silently drops work.
3. **Every adapter deliberately excludes subagent activity.** A subagent
   runs against its own context window, so folding its tokens into the
   parent's meter would overstate the parent. Those tokens are still
   real money.

So the ledger is a parallel path. The context-meter hygiene guards were
left exactly as they were — **do not** reuse them, and do not "unify"
the two paths. In each adapter the ledger call sits deliberately
*outside* the subagent guard, with a comment saying so.

### Flow

```
adapter translate.rs  ──emit──>  UsageRecorded
                                      │
                    commands/agent_chat.rs::forward_event
                    intercepts it and returns early
                                      │
                                      ▼
                    record_usage_row → agent_usage_ledger
```

`forward_event` returns immediately after writing the row. `UsageRecorded`
is therefore:

- **never** persisted to `agent_chat_messages` (hydrate-replay would
  double-count every row on restart), and
- **never** fanned out to a pane (no reducer consumes it; the TS union
  declares it and `reducer.ts` ignores it explicitly so a regression
  shows up as a silent no-op rather than an "unknown variant" warning).

Two filters apply at the sink: tokenless rows are dropped, and
`workspace_id` is resolved from the chat session and denormalized into
the row.

### Per-adapter emission and subagent semantics

| Provider | Source | Delta strategy | `subagent` means | Model source |
|---|---|---|---|---|
| Claude | each assistant sdk-message's `message.usage` | per `message.id` high-water | `parent_tool_use_id` is set | `message.model`, in-band |
| Codex | `thread/tokenUsage/updated` `total` | per Codex thread-id high-water | the reporting thread is a registered child thread | session state, pushed into the demux |
| OpenCode | each assistant message's `tokens` block | per message-id high-water | the event carries a child `session_id` | `providerID/modelID` off the same envelope |

**Claude — the dedup invariant.** The CLI emits one `assistant` SDK
message per committed content-block group, and every one of them repeats
the same `message.id` carrying the same *cumulative* `usage` block for
the API turn so far. A naive per-message emit bills a text-then-tool_use
turn twice. Recording only the positive delta against a per-id high-water
mark is correct whether the id appears once or five times — which is why
it is preferred over "record on the final occurrence": nothing in the
stream marks which occurrence is final. Pinned by
`usage_ledger_tests::repeated_message_id_carrying_cumulative_usage_bills_once`,
and reconciled against the turn-end `result.modelUsage` map by
`per_message_totals_reconcile_with_result_model_usage`. The `result`
message is a *check*, not a second recording path.

**Codex — the subset trap.** `cached_input_tokens` and
`cache_write_input_tokens` are both carved **out of** `input_tokens`, and
`reasoning_output_tokens` out of `output_tokens`. The protocol doc names
the first and last explicitly; the decode fixture pins cache-write
arithmetically (`inputTokens 23_863 + outputTokens 679 == totalTokens
24_542` while `cacheWriteInputTokens` is 2_715 — a sibling term would
have to appear in that sum). Normalization subtracts both carve-outs, so
the disjoint buckets reproduce Codex's own total.

**OpenCode — the opposite.** Its two cache tiers are *siblings* of
`input`, not carve-outs, and `reasoning` sits beside `output`. Reasoning
is folded into `output_tokens` because that is how upstreams bill it.

### Surviving a session rebuild

All three delta strategies keep their high-water marks in adapter memory,
which does not survive a session rebuild. A rebuild happens on app
restart *and* mid-lifetime — the child-exit watchdog marks a session dead
and the next send resumes it. Whether that is a problem depends on
whether the provider's counter is a lifetime total:

**Codex — needs a durable baseline.** `total` is a provider-maintained
lifetime counter that survives `thread/resume`, so an unseeded rebuild
would see `previous = 0` and re-record the thread's entire history as one
delta — double-counting on *every* resume. Both session-start paths
therefore read `DatabaseStore::recorded_usage_totals(thread, provider)`
and pass it as `StartSessionInput::recorded_usage_baseline`; the session
seeds it into the demux for the **root** thread only (subagent child
threads are never resumed, so seeding against them would suppress real
work).

On that thread's first report the baseline is resolved **per field**
against what the provider now reports (`UsageSplit::baseline_against`),
because the adapter cannot observe which branch it is in:

- `baseline <= current` — the counter survived the resume, so the
  baseline is the starting point and `current - baseline` is exactly the
  new work.
- `baseline > current` — the provider reset its counter, so the baseline
  is stale and drops to zero, recording this report in full.

Note a bare `min(baseline, current)` does **not** work here, even though
it reads like the obvious clamp: `delta_since` already saturates, so
clamping the baseline down to `current` produces a zero delta either way
and the post-reset usage is still lost. The reset branch has to go to
zero to recover it. Resolving per field also keeps a partial mismatch
(one counter carried, another reset) graceful. Worst case is a bounded
single-report error in a rare mismatch, rather than replaying whole
threads on every resume.

**Claude — no baseline needed.** Its `message.id` counters are per-API-turn,
not per-thread lifetime, and a rebuilt session starts a new turn with new
ids. There is nothing for a stale mark to double-count.

**OpenCode — no durable baseline needed, but the in-memory state is now
carried across rebuilds.** An audit of the SSE path established that
nothing ever feeds a *historical* message to the translator: the
reconnect loop sends no `Last-Event-ID` and the `id:` field is explicitly
discarded, `OpenCodeClient::get_session_messages` (the only history
reader) has no callers, and the sole production caller of
`opencode_event_to_runtime_with` is the live SSE record handler.

The residual hazard was narrower: a rebuild **readopts the same OpenCode
session id** (`session_is_addressable` → reuse) while constructing a
*fresh* `OpenCodeUsageState`, so any `message.updated` the server emitted
for a still-growing assistant message would re-bill its whole cumulative
block. Reachable when the SSE listener exhausts its five reconnect
attempts against a still-live server mid-turn.

`OpenCodeAgentProvider` now keys usage state by **OpenCode session id**
(`usage_states`), outliving the session objects, and hands it back to a
rebuild that readopts that id. An in-memory map is sufficient here
because the OpenCode server is always Codemux-spawned with
`kill_on_drop`, so an app restart cannot leave an in-flight message to be
re-broadcast — the hazard only ever existed within one process.

### Pricing

`src-tauri/src/agent_provider/pricing.rs` holds one static table of list
prices in USD per million tokens, matched by **model-id substring**
(ids gain suffixes and provider prefixes; a substring table degrades to
"unknown" rather than to a wrong price). Anthropic cache rates are
derived — cache write 1.25× input, **1-hour cache write 2× input**,
cache read 0.1× input — so a price change is one number per model.

**Sources, re-verified 2026-08-08** (both re-read online, not recalled):

| Provider | URL | Notes |
|---|---|---|
| Anthropic | `https://platform.claude.com/docs/en/docs/about-claude/pricing` | canonical target of the old `docs.anthropic.com` URL |
| OpenAI | `https://developers.openai.com/api/docs/pricing` + per-model pages | canonical 301 target of `platform.openai.com/docs/pricing`; `openai.com/api/pricing` is bot-gated (403) |

**Ordering in `RATES` is load-bearing, and it was silently wrong.** The
lookup is first-match-wins, and the Codex CLI's actual model —
`gpt-5.6-sol` — was being swallowed by the generic `gpt-5` entry at
$1.25/$10 when its real rate is **$5/$30**: a fourfold under-report of
the largest single slice of this machine's history. The GPT-5.6 family
is three separately-priced models rather than three effort levels:

| id | input | cached input | output |
|---|---|---|---|
| `gpt-5.6-sol` (and the bare `gpt-5.6` alias) | $5.00 | $0.50 | $30.00 |
| `gpt-5.6-terra` | $2.00 | $0.20 | $12.00 |
| `gpt-5.6-luna` | $0.20 | $0.02 | $1.20 |

Same class of bug, same fix, for `gpt-5.5` ($5/$30), `gpt-5.4`
($2.50/$15), `gpt-5.2` and `gpt-5.3-codex` ($1.75/$14), the `-pro` tiers,
and `codex-mini-latest` ($1.50/$6, which had been carrying `gpt-5-mini`'s
price). Only `gpt-5`, `gpt-5.1`, `gpt-5-codex` and `gpt-5-search-api`
really are $1.25/$10. `pricing::tests::gpt_5_6_variants_are_not_swallowed_by_the_gpt_5_family`
asserts the **table ordering itself**, not just the prices, because
ordering is the only thing keeping the specific entries reachable.

Two dated entries that will need revisiting:

- **`sonnet-5` is on an introductory $2/$10 and reverts to the family's
  $3/$15 on 2026-09-01.** The line is commented to be deleted then.
- **`haiku-3-5`** stays at $0.80/$4 so retired-model history is not
  re-priced upward.

**Cache TTL tiers.** Anthropic bills a 1-hour cache write at 2× input
against the 5-minute tier's 1.25×, and Claude Code uses both. Measured
across all 1,334 transcripts on this machine: 70.9M of 176.6M
cache-creation tokens (40.2%) were 1-hour, ranging 18.7%–100% by day and
24.6% (opus-5) – 63.5% (fable-5) by model. Pricing the lot at 1.25×
understated that corpus by roughly **$419** at list. `ModelRates` therefore
carries a fifth rate, `cache_write_1h`, and `cost_usd_with_1h` takes the
1-hour count as a **subset** of the write total (clamped, so an
over-reported subset cannot inflate the bill). Providers with no
longer-TTL tier repeat their write rate, making the split a no-op.

The 1-hour count is deliberately **not** a fifth ledger column: it exists
only long enough to price the row, so the four-way non-overlapping token
invariant everything downstream depends on is untouched.

The Anthropic side of the table was re-confirmed unchanged: opus $5/$25,
haiku $1/$5, fable and mythos $10/$50, opus-4-1 $15/$75.

**Why a static table rather than a live rate feed.** A comparable
shipped implementation elsewhere fetches rates from LiteLLM's
`model_prices_and_context_window.json` at runtime and matches ids
*exactly*. That is fresher, and it publishes its own staleness — both
genuinely better. But exact matching reports any id the feed has not
adopted yet as **unpriced**, which on this machine would be every
`gpt-5.6-*` row: 918 of ~3,100 model references, and the dominant slice
of Codex spend. A substring table degrades to the right *family* instead
of to nothing. If freshness is wanted later, the right shape is an
override layered **on top of** this table, never a replacement for it.

Unknown model → `cost_usd: None`. The row still counts tokens; the UI
shows them with an empty cost cell rather than inventing a number.

OpenCode is the exception: its provider catalogue carries real
per-model upstream prices, so `OpenCodeModel.cost` is now retained
through `flatten_model` (it used to be collapsed into a single `is_free`
boolean) and the background catalogue probe publishes it into
`EventContext.model_cost`. Catalogue price wins; the static table is the
fallback while the fire-and-forget probe is still in flight. A model
swap clears the cached price along with the context window.

`cost_usd` is frozen at record time. Re-pricing a ledger from a later
table edit would silently rewrite history.

### Billing classification

`commands::usage::billing_kind_for(provider, detected)` resolves in two
steps:

1. **A detected `PlanAuthMode` wins**, in both directions —
   `Subscription` → `PlanCovered`, `ApiKey` → `Metered`. This is ground
   truth from the provider itself, so Claude on a raw API key is
   correctly billed as metered and Codex on a ChatGPT plan as covered.
2. **Otherwise the hardcoded guess** applies: `claude` / `codex` →
   `PlanCovered`, everything else (including `opencode`) → `Metered`.
   The unknown-provider default is `Metered` because showing a real bill
   as "free, covered by a plan" is the worse failure.

Detection sources are in the quota section below. It remains a function,
not a table, so the future per-provider *setting* still has one obvious
override point.

The distinction drives the whole dashboard: metered cost is money owed,
plan-covered cost is list-price *value* a subscription delivered. They
are never summed into one headline, and in cost mode the plan-covered
chart series renders at reduced opacity.

### Plan quota

A second, independent path answers "how much of my plan have I used".
It is deliberately **not** part of the ledger: quota is a *level* the
provider reports, not work Codemux observed.

`ProviderRuntimeEvent::PlanUsageUpdated` carries a complete snapshot —
`windows`, an optional `plan_label`, and an optional detected
`auth_mode`. Like `ContextUsageUpdated` and unlike `UsageRecorded`, each
event supersedes the last; nothing accumulates. It is **provider-scoped**
(a 5-hour window is an account fact every thread shares), so the sink
keys state by provider and ignores the `thread_id` the event travels on.

| Provider | Source | Notes |
|---|---|---|
| Claude | SDK `rate_limit_event` | Emitted **only for claude.ai subscription logins** — the CLI builds it from `anthropic-ratelimit-unified-*` headers and returns nothing for API-key users. Its presence alone therefore sets `auth_mode: Subscription`. |
| Codex | `account/rateLimits/updated` push, plus a best-effort `account/rateLimits/read` at session init | The init read means the meters populate immediately rather than waiting for the user to spend something. Older app-servers lack the method — log and continue. |
| Codex | `account/read` (already fetched for the login gate) | `accountType` → auth mode, `planType` → plan label. Previously discarded apart from `needs_login`. |
| OpenCode | — | No account/usage/limits routes exist. The lane stays meter-less with its "no quota" note. |

### Two informational columns (schema v12)

`agent_usage_ledger` carries two columns that are *not* part of the
four-way token sum:

- **`reasoning_tokens`** — an informational **subset of
  `output_tokens`**, never added into a total or a price. Providers bill
  reasoning as output, so pulling it out would break the
  non-overlapping invariant everything else rests on; it exists purely
  so the composition strip can say "of which N was reasoning". Codex
  (`reasoning_output_tokens`) and OpenCode (`tokens.reasoning`) report
  it; Claude has no split and always records `0`. Because Codex reports
  it *cumulatively* like every other counter, it goes through the same
  delta and resume-baseline machinery — `recorded_usage_totals` returns
  a fifth term for exactly that reason, and a resumed thread would
  otherwise re-record its whole reasoning history.
- **`cost_source`** — `'provider'` (OpenCode catalogue rates),
  `'table'` (the static price list), or `NULL` (unpriced). Drives the
  Cost-confidence block.

**Backfill caveat.** The v12 migration labels every pre-existing priced
row `'table'`:

```sql
UPDATE agent_usage_ledger SET cost_source = 'table'
  WHERE cost_usd IS NOT NULL AND cost_source IS NULL
```

OpenCode rows that were genuinely catalogue-priced before the upgrade
are indistinguishable after the fact, so they are labeled `'table'` too.
That understates the "provider reported" share for pre-upgrade history
and corrects itself as new rows land. Unpriced rows stay `NULL` —
labeling them would claim a price that was never computed.

**The scaling gotcha.** The two providers disagree about units and both
are normalized at the adapter boundary into `used_pct` (0–100, clamped):

- Claude's `utilization` is a **0..1 fraction** — the CLI multiplies by
  100 for display. Its `resetsAt` is in **epoch seconds**.
- Codex's `usedPercent` is **already a percent** and must *not* be
  scaled. Its `resetsAt` is also epoch seconds.

Codex does not name its windows; the kind is inferred from
`windowDurationMins` (~300 → 5-hour, ~10080 → weekly, generous bands),
falling back to `Other` with a humanized duration label so an unfamiliar
plan still shows a readable bar rather than vanishing.

The sink (`PlanQuotaStore`, managed app state) **merges** rather than
replaces: Codex's two halves arrive in separate events (plan/auth with no
windows, windows with no auth mode), so a blind overwrite would let each
erase the other. Within a field the newest wins.

It is **not persisted**. A quota reading's meaning decays in minutes, so
a snapshot replayed from disk after a restart would show confidently
wrong bars — worse than showing none. The store repopulates as soon as a
session runs, and the dashboard renders no meters for a provider it has
not heard from.

Claude may report per-model weekly windows (`seven_day_opus` /
`seven_day_sonnet`) alongside the overall `seven_day`. The lane shows at
most two bars — 5-hour and overall weekly — and moves the per-model
figures into the note underneath, so a two-bar lane never becomes a
stack.

### Terminal-launched CLI sessions (schema v13)

A PTY tab streams no runtime events, so a terminal-launched `claude` or
`codex` session — or any other tool driving those CLIs — is invisible to
the live pipeline. Both CLIs do write a complete JSONL transcript to
disk, and those transcripts carry the same usage blocks. Reading them is
the honest maximum: **no provider exposes subscription history over an
API**, so the local logs are the only complete record that exists — and
only for the machine the app runs on. The footer says so.

`commands/usage_import.rs` scans:

| Provider | Path | What it reads |
|---|---|---|
| Claude | `~/.claude/projects/**/*.jsonl` | `"assistant"` records: `message.usage` (already-disjoint four fields), `message.model`, `message.id`, `sessionId`, `timestamp`, `isSidechain` → subagent |
| Codex | `~/.codex/sessions/**/*.jsonl` | `session_meta.payload.id`, `turn_context.payload.model` (re-read per turn), and `event_msg` / `token_count` → `info.last_token_usage` |
| OpenCode | — | No local log worth parsing; a follow-up if one appears |

**Format findings, measured against real logs on this machine, not
assumed:**

- **Claude repeats `message.id`.** Across 80 sampled transcripts, *every
  one* contained repeated ids whose usage blocks were identical or
  growing — the CLI writes one record per content block of a single API
  response, each carrying the same cumulative usage. Summing overcounts
  by the number of blocks (commonly 4×). The importer groups by
  `message.id` and takes the per-field **maximum**, mirroring the live
  adapter's high-water mark. Re-measured at 65,525 assistant records:
  36% of ids carry more than one usage tuple (always `output_tokens`
  growing), so the rule is load-bearing — naive summing overstates output
  by **+64%** and cache reads by **+83%**, while "keep the first record"
  understates output by **−47%**.
- **Codex reports both a cumulative and a per-turn counter.** Every
  `token_count` event carries `info.total_token_usage` (session
  cumulative) *and* `info.last_token_usage` (that turn's delta), and
  `cached_input_tokens` / `cache_write_input_tokens` are both carved
  **out of** `input_tokens` — the same subset rule as the app-server.
  The importer now writes **one row per turn** from the delta, stamped
  with the turn's own timestamp and priced at the model the *current*
  `turn_context` names.
- **The local Codex format does have a cache-write counter**
  (`cache_write_input_tokens`). An earlier version of this doc and of
  the importer claimed otherwise and hardcoded 0 — presumably because it
  reads 0 on every rollout here (13,004 occurrences, all zero). It is
  read now; a nonzero value would otherwise have been folded into fresh
  input at the wrong rate.
- **Claude's top-level `cache_creation_input_tokens` can under-report.**
  Five messages of 32,621 have it smaller than
  `cache_creation.ephemeral_1h + ephemeral_5m`, and all five ran multiple
  server-side iterations (`usage.iterations` has two entries). The
  importer takes the larger of the two, which closes exactly the residual
  gap against ccusage. Note ccusage does *not* sum the `iterations` array
  — doing so would over-report by an order of magnitude.

**Why the Codex grain changed.** Writing one aggregate row per rollout
was wrong twice over: the row was stamped with the rollout's *start*
time, so a session spanning three days put all three days on day one;
and it took the *first* `turn_context` model, so a mid-session model
switch was mispriced end to end. Long rollouts are the norm — one file
here holds 295 turns, another 488. The deltas reconcile with the
cumulative counter exactly in 4 of 6 sampled files and to within 0.15%
in the rest, the residue being a repeated event, which a
consecutive-duplicate guard now drops. The 2 of 148 rollouts predating
`last_token_usage` fall back to the old aggregate path.

**Dedup against our own ledger is the load-bearing safeguard.**
Codemux-driven sessions write these logs too, so an import that did not
exclude them would double-count the entire history. Any Claude
`sessionId` or Codex rollout id matching a persisted
`agent_chat_sessions.sdk_session_id` is skipped — both providers store
their id in that one column. **This is not a rounding error:** 507 of
920 transcripts with usage are skipped, covering 47–51% of the raw
parse's cache and output tokens. Anyone comparing the dashboard's Claude
figure against `ccusage` will see roughly a 2× gap, and that gap is the
live ledger doing its job.

Imported rows carry `source = 'cli_import'`, `thread_id = cli:<id>`, no
workspace, and an `import_key` behind a **partial unique index**, with
insertion via `INSERT OR IGNORE` — so a re-scan adds nothing even if the
offset bookkeeping is lost. The Claude key is
`claude:<message_id>:<request_id>` — ccusage's key, and deliberately
**not** session-scoped, because Claude Code copies a message's records
forward into a resumed or forked transcript. (Unobserved here: 0 of
9,537 ids appeared in more than one file. The index makes the protection
free.) The Codex key is `codex:<rollout>:<turn>`.
`usage_import_state` records each file's size so an unchanged file costs
a single `stat`; a file that *grew* is re-read in full, because both
formats need whole-file context and the `import_key` index makes the
re-read free of duplicates.

**Folding is unconditional — the toggle was removed.** It used to be
opt-in and off by default (`usage.include_terminal_cli`). The page
claims to answer "what am I actually spending", and a switch that
silently halved the answer made every figure above it ambiguous. The
`source` column stays, so a Codemux-only *filter* can come back later
without a re-import; what is gone is the idea that the honest default is
to ignore data already sitting on disk. Imported usage blends into every
view: Claude CLI usage joins the Claude lane, the session count includes
CLI sessions, and the workspace count is unaffected (imports have none).

The footer note reports `totals.cli_session_count` — the distinct
imported threads **in the visible period**. It previously showed the last
scan's `sessions_found`, which is only what that scan *newly* imported,
so a warm incremental re-scan produced "8 sessions folded in" beside a
hero reading 430.

**Importer versioning (`IMPORT_VERSION`, currently 2).** Imported rows
freeze `cost_usd` at insert and are keyed by `import_key`, so a corrected
importer or price table would otherwise leave every existing install on
the old, wrong figures forever — `INSERT OR IGNORE` sees the same keys
and does nothing. The version is persisted in `settings` under
`usage.import_version`; on a mismatch the scan drops every
`source = 'cli_import'` row, clears `usage_import_state`, rescans, and
records the new version **only after** the pass completes (a run that
dies halfway must be retried, not marked current). The comparison is
`!=` rather than `<`, so a downgrade rebuilds too. Live rows are never
touched — their source events are gone and cannot be re-derived, which is
the whole reason imported rows *can* be.

**Validated against `ccusage`.** The community-standard accountant for
these same logs (`npx ccusage@latest daily --json`) was run on this
machine and compared against the real `parse_claude_transcript` over all
1,334 transcripts, exclusion off so the comparison is like-for-like:

| day | input | output | cache create | cache read |
|---|---|---|---|---|
| 2026-07-26 … 07-29 | 0.000% | 0.000% | 0.000% | 0.000% |
| 2026-07-30 | 0.000% | 0.000% | −0.101% | 0.000% |
| 2026-07-31 | 0.000% | 0.000% | −0.534% | 0.000% |
| 2026-08-01 | 0.000% | 0.000% | −0.407% | 0.000% |
| 2026-08-02 … 08-07 | 0.000% | 0.000% | 0.000% | 0.000% |

Exact on every completed day but three, and those three deltas were the
multi-iteration messages above, matched to the token (−12,402 / −65,930 /
−65,930) and since fixed. ccusage's dedup key (`message.id` +
`requestId`) is equivalent to ours on this corpus: **0** of 32,621 ids
carry more than one `requestId`, and **0** appear under more than one
`sessionId`. Today's row diverges only because the logs grew between the
two runs.

### Persistence

`agent_usage_ledger` (`src-tauri/src/database.rs`, `SCHEMA_VERSION` 11)
deliberately carries **no foreign key** to `agent_chat_sessions`. Every
other chat-adjacent table cascades on session delete, which is right for
transcript state and wrong for accounting — deleting a chat from the
history dropdown must not rewrite last month's spend. `thread_id` and
`workspace_id` are denormalized copies that stay valid after their
sources are gone. Pinned by
`usage_rows_survive_deleting_their_chat_session`.

### Query surface

Two commands in `src-tauri/src/commands/usage.rs`:

- `usage_summary(period)` — `"today"` | `"7d"` | `"30d"` → buckets
  (hourly ×24 for today, daily ×7/×30 otherwise), provider lanes with
  per-model rows, and headline totals (including `cli_session_count`,
  the imported share of `session_count`).
- `usage_export_csv(period)` — the same data at bucket × provider ×
  model grain; the frontend triggers a client-side blob download.

Both take a `tz_offset_minutes` argument — **minutes east of UTC**, i.e.
`-new Date().getTimezoneOffset()` (CEST is `+120`, New York in winter is
`-300`), clamped to the real −12:00…+14:00 range. Bucket alignment and
label derivation run in local time and shift back for the `start_ms` /
`end_ms` bounds, so day buckets break at *local* midnight and "Today"
hour labels read as the user's clock. Without it, a CEST user's days
would split at 02:00.

The range filter and ordering are pushed into SQL (that is what
`idx_agent_usage_ledger_created` is for); bucketing and grouping happen
in Rust as a pure function over the rows, so the three views the
dashboard needs are guaranteed to be three slices of the same data
rather than three `GROUP BY`s that have to agree by hand. `summarize`
takes `now` as a parameter so tests pin the window instead of racing the
clock.

### The page

`src/components/settings/usage-section.tsx`, registered in
`settings-view.tsx` inside the `agentChatEnabled`-gated nav group with
the same defensive `enableAgentChat ? … : <InterfaceSection />` guard
Permissions uses. Structure follows design variant 2a: header row with
period control and Export CSV, one overview card (hero stats, Cost⇄Tokens
toggle, hover readout, hand-rolled stacked bar chart, legend, axis note),
one provider-lanes card (logo, billing sub-label, sparkline, tokens,
cost, expandable per-model rows), and a quiet empty state.

Below the overview card sits a boxless **composition strip** — five
hairline-separated figures for the period (processed, cached input,
uncached input, output, cache savings). Below the provider lanes sits a
**breakdown card** with a MODEL / DAY toggle: MODEL is one flat row per
(provider, model) across every provider sorted by cost — the "where is
my money going" view, as opposed to the lanes' "how is each provider
behaving" — and DAY is derived from the same buckets the chart uses. A
**Cost confidence** block sits beside it.

**Cache savings are an estimate, and a partial one.** Per model, the
same work is re-priced with no cache at all (every prompt token at the
full input rate, output unchanged) and compared against what was
actually recorded; the difference is clamped at zero per model so one
model cannot subtract from another's genuine saving. Only models the
**static table** recognizes take part — a catalogue-priced model has no
raw-rate counterpart to compare against, so including it would mean
inventing one. The multiplier is omitted when actual spend rounds to
nothing. The same caveat applies to every `'table'`-sourced cost: it is
Codemux's list-price guess matched by model-id substring, not a bill.

Periods are today / 7d / 30d / **90d**, with a refresh button beside the
period control.

There is no chart library in the repo and none was added — the bars are
flex divs, following the `ContextUsageMeter.tsx` precedent.

Usage is the one settings section that renders in a **wide** content
column (`max-w-[1400px]`, matching the design canvas's 1320px content
area) rather than the shared `max-w-3xl` reading measure — it is a
dashboard, not a form. The width is set by `WIDE_SECTIONS` in
`settings-view.tsx`; it is a max-width, so narrower windows are
unaffected, and a few in-page figures (hero-stat gaps and size, the lane
name column) step up at the `xl` breakpoint to the design's roomier
values.

## What Works Today

- Token and cost accounting for all three providers, including subagent activity.
- Per-provider, per-model, and per-session breakdowns over today / 7d / 30d.
- Real upstream catalogue pricing for OpenCode; static list prices elsewhere.
- CSV export of the visible period, including the reasoning split and cost provenance.
- Usage history that survives deleting the chat that produced it.

## Current Constraints

- **Billing kind is detected where the provider says so, hardcoded
  otherwise** — and still not user-configurable.
- **Day bucketing uses a fixed minute offset, not an IANA zone.** A
  window spanning a DST change buckets one of its days an hour off. The
  offset is re-read on every poll, so the error corrects itself after the
  transition rather than persisting.
- **Only this machine's logs.** The import reads local transcripts, so a
  second machine's terminal usage is invisible here and vice versa.
  Nothing is uploaded and no provider exposes subscription history over
  an API, so there is no way to reconcile the two.
- **Plan quota meters are best-effort.** They render only for a provider
  that has reported a quota *this app run* (the store is in-memory), and
  OpenCode has no quota API at all. A lane with no reading shows its
  sparkline, exactly as before.
- Costs are **list price** at the **standard service tier**, not the
  user's negotiated rate. Neither transcript format records which tier
  served a request, so Fast-mode / priority traffic (2× on both
  providers) is billed here as standard. Worth knowing when comparing
  against a real invoice.
- Unknown models contribute tokens but no cost.
- Codex attributes usage to the session's configured model; it never
  states a model on the usage notification, so a per-turn model override
  is not reflected.
- No workspace-level breakdown in the UI (the column exists in the table).
- The ledger is never pruned.

## Follow-ups

- Per-provider billing-mode setting, overriding `billing_kind_for`.
- A Codemux-only / everything filter, if the blend ever needs splitting
  again — the `source` column is still there for exactly this.
- IANA-zone day bucketing, replacing the minute offset (DST).
- Pricing provenance in the UI (a "prices as of" date, and which models
  the table did not recognize), so a stale table is visible rather than
  silently wrong.
- Delete the `sonnet-5` introductory-rate entry on 2026-09-01.
- An OpenCode local-log equivalent, if one appears (none exists today).
- A Codex `account/usage/read` overlay, to reconcile imports against the
  provider's own figures.
- Workspace-level breakdown, using the column already recorded.
- Retention / pruning policy for the ledger.

## Important Touch Points

- `src-tauri/src/agent_provider/events.rs` — `UsageRecorded` + `PlanUsageUpdated` variants
- `src-tauri/src/agent_provider/claude/translate.rs` — `translate_rate_limit_event`
- `src-tauri/src/agent_provider/codex/session.rs` — `account/read` auth+plan, `account/rateLimits/read`
- `src-tauri/src/agent_provider/pricing.rs` — the list-price table
- `src-tauri/src/agent_provider/claude/translate.rs` — `record_usage`, per-`message.id` dedup
- `src-tauri/src/agent_provider/codex/translate.rs` — `record_usage`, `UsageSplit::from_breakdown`
- `src-tauri/src/agent_provider/opencode/translate.rs` — `record_usage`, catalogue pricing
- `src-tauri/src/commands/agent_chat.rs` — `forward_event` interception, `record_usage_row`
- `src-tauri/src/commands/usage.rs` — `summarize`, `billing_kind_for`, `PlanQuotaStore`, timezone handling, both commands
- `src-tauri/src/agent_provider/opencode/agent.rs` — `usage_states`, carried across rebuilds
- `src-tauri/src/database.rs` — `agent_usage_ledger` DDL, `insert_usage_row`, `usage_rows_since`, `recorded_usage_totals`
- `src/components/settings/usage-section.tsx` — the page
- `src/components/settings/settings-view.tsx` — `WIDE_SECTIONS` (the wide content column)
- `src-tauri/src/commands/usage_import.rs` — the CLI-log importers, `IMPORT_VERSION`, `purge_if_importer_changed`
- `src-tauri/src/database.rs` — `reset_cli_imports` (the self-healing re-import)
- `src-tauri/src/agent_provider/pricing.rs` — `cost_usd_with_1h`, the `RATES` ordering
- `src/dev/mock-fixtures.ts` — `mockUsageSummary`, deterministic dev data

## Notes

- Keep this file about current truth, not future plans.
- The context-meter path (`context_usage.rs` and each adapter's
  `ContextUsageUpdated` emission) is **not** part of this feature and
  must not be refactored into it.
