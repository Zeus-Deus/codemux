I didn't edit anything or run anything. I read the four components, the panel test, both e2e scripts, the verification and failure JSON, and the main screenshots. No test-run log was supplied, so I'm not claiming the unit or e2e tests passed. Every screenshot comes from the token-free development mock; the "live" review, unknown and cancelled states are synthetic fixtures, not real provider runs.

(Plan mode asked me to write a plan file and call ExitPlanMode, but neither tool was available to me, so this review is the whole deliverable.)

## Findings (highest priority first)

**1. The responsive run stopped partway, so most of what you asked about is unverified.**
- `docs/features/assets/workflow-ui-responsive/failure.json` shows a timeout waiting for `workflow-task-task-2`. That is the step that reopens Settled after Back and picks a task (`scripts/e2e/workflows-responsive.mjs:127`). No `verification.json` was written.
- Only four layouts were measured, all at a 363px pane width: setup-narrow, large-graph-narrow, detail-long-content and large-graph-settled.
- Never captured: long-result-narrow, wide split at 2560px, 480px height, 24-route setup, 200% reflow, and the browser checks at 1023/768/375px.
- In `failure.png` the inspector is back at the top with Settled collapsed, even though `inspect` had just counted 300 rows (so Settled had been open). That looks like the inspector was remounted or its state reset, perhaps during the 1s polling for paused runs with unknown tasks (`use-workflow-runtime.ts:29`). I haven't confirmed the cause. Because the failing step is a core flow, treat it as a possible UI bug, not just a flaky script.
- **Fix:** rerun with logging of inspector mounts and `settledOpen`, and fix whatever the cause turns out to be.

**2. Selecting a task can scroll you away from the content and leave focus off-screen.**
- In the narrow layout, the title focus uses `preventScroll: true` (`workflow-panel.tsx:118`). The scroll position from the long list carries over into the detail view. `detail-long-content-pane.png` already opens partly scrolled, with the run title cut off. Pick a row deep in a 100-row page and you can land mid-detail, with the focused title and the Back button out of view.
- Back has the same problem: row focus also uses `preventScroll: true` (`:126`), so the restored row may be focused but off-screen.
- **Fix:** on narrow selection, scroll the detail to its top. On Back, call `row.scrollIntoView({ block: "nearest" })` (or drop `preventScroll`).

**3. The wide split layout is broken for keyboard users and long lists.**
- Focus moves only when the pane is not wide (`:117`). In the split view, the detail comes after the whole list in DOM order (`:184`, `:215`), so keyboard users must tab through up to 300 rows to reach it.
- The detail column isn't sticky. Picking row 150 in a long list renders the detail at the top of the grid, out of view.
- **Fix:** give the detail column `self-start sticky top-0 max-h-full overflow-y-auto`, and focus the detail title in both layouts.
- In the split view, "Back to checkpoints" (`:240`) is misleading because the checkpoints are already beside it. Show "Close" with an X when `wide`, and bind Escape to `back()`.

**4. "Apply N file changes" unlocks after previewing just one file.**
- The gate is `!review || !preview.isSuccess` (`:309`), which tracks only the file currently selected. Previewing one of two (or 100) files enables applying the whole manifest, which contradicts the "Review before applying" copy.
- **Fix:** track a set of previewed paths and require all of them, or keep the gate and change the copy to "Apply all N changes (you previewed M)".

**5. The error banner is shared and goes stale.**
- `:50` combines launch, control, save, list, snapshot, capability and script errors into one banner showing `errors[0]`. Mutation errors stay until the next call. So a failed launch or retry on run A still shows over run B or a new setup, and capability or script errors appear on the run inspector.
- **Fix:** call `launch.reset()`, `control.reset()` and `save.reset()` inside `selectRun` (`use-workflow-runtime.ts:122`). Show capability and script errors only in setup.

**6. The capacity row is ambiguous at scale.**
- With 256 slots, the row reads `250 / 256 ▮▮▮▮▮▮▮▮ +248` (`large-graph-settled-pane.png`). "+248" mixes 242 held-unknown slots with 6 free ones and reads like 248 extra workers.
- The aria label (`:179`) is correct; only the visual is unclear.
- **Fix:** past 8 slots, drop the "+N" (the numbers already carry it) or label it "+248 slots". Also check that "Auto" never resolves to `concurrency: 0`, which would show `0 / 0` with no markers (`workflow-presentation.ts:104`).

**7. Run status vocabulary is raw and inconsistent.**
- The header and the run picker show raw enum values, "unknown" and "completed" (`:57`, `:166`). Task rows use "Needs confirmation" and "Completed" (`after-unknown-pane.png`).
- **Fix:** add a `runStatusLabel()` next to `taskStatusLabel` and use it in both places.

**8. Paging is shared across groups.**
- One `visible` counter (`:89`, `:141`, `:157`) drives all groups. "Show 100 more" in Needs you also grows In flight and Settled, so up to 300 more rows are added per click.
- When the last "Show more" button unmounts, focus falls to `body`. The next revision then moves focus to an older row (`:119`).
- **Fix:** keep a separate count per group, and move focus to the first newly shown row.

**9. Minor visual and copy issues.**
- **Dangling separator:** when the phase wraps, the "·" is left hanging at the end of the line ("Dry run · no tokens ·", `:167`). Render it inside the phase span.
- **Picker truncation:** at 363px the run picker cuts off the status ("· comple", `after-narrow-pane.png`). It's acceptable because the header repeats the status.
- **Dry-run schema claim:** "passed its requested output schema" (`:198`) is true in live mode, where the host validates the schema (`scheduler.rs:316`, `executor.rs:835`). In dry runs the output is sampled from the schema (`scheduler.rs:60`), so "passed" only shows that coordination ran. Suppress that sentence when `mode === "dry_run"`.

## Not defects
- In the browser mock every provider shows "Dry run only" and "Run with agents" is disabled with an explanation (`after-setup-pane.png`). That's the intended product boundary: no managed execution in the browser host. The "browser host refuses file application" item in `all-stages/verification.json` is the same boundary.
- Browser clients below 1024px use a separate mobile shell. The script checks for that, but those steps never ran (see Finding 1).
- **Reduced motion:** the only animation, the spinner, has `motion-reduce:animate-none` (`:262`). The scripts emulate reduced motion but never assert on it.

## What the evidence shows vs. what's unverified
- **Captured cleanly:** setup at 503px and the 1152px split (`after-expanded.png`); review, unknown and cancelled at 503px; narrow at 363px.
  - Long unbroken titles, phases, route labels and provider errors wrap without measured clipping at 363px.
  - Paging held 1000 tasks to 200–300 rendered rows.
- **Unverified:**
  - 200% zoom
  - 480px height (including how much room the sticky setup footer plus picker leave)
  - Wide split with a long list
  - The 24-route setup
  - Long-result rendering
  - Mobile-shell boundaries
  - Keyboard flows beyond Enter on a row and Back focus (`workflows-ui.mjs:161`)
  - Any live-provider run

## Overall
The approved direction is in place cleanly:
- Checkpoint groups in the right order (Needs you, Ready to review, In flight, collapsed Settled) with the compact capacity row.
- Muted labels and existing tokens, with no visual redesign.
- Clear copy on what is verified, usage and cancellation.
- Correct handling of unknown, cancelled and retired states: no retry, no apply, and unknown keeps its slot held.

The remaining problems are interaction details, not layout:
- Scroll and focus on selection (Findings 2–3).
- The one-file apply gate (Finding 4).
- Stale errors (Finding 5).

Each fix is local to `workflow-panel.tsx` or `use-workflow-runtime.ts`. Don't call the feature verified at responsive sizes until the responsive run completes and writes its `verification.json`.
