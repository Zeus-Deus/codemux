## Verdict: no layout defects found, but two focus/scroll bugs remain, plus one small apply-button gap

I only used Read, Glob and Grep, and I didn't run any tests. The evidence is mock data: `providerCalls: 0` and `modelTokens: 0`. It doesn't show that paid runs or any particular provider work.

The 17 measured layouts all report `overflow: false` and an empty `overflowing` list. They cover the 363px pane, the 2560px split, 480px height, 200% zoom, 24 routes and the 375px native layout. With 1000 tasks the panel renders at most 200–300 rows. The screenshots match the existing design tokens, and long unbroken titles and model names wrap cleanly. The 375px screenshot shows the status vocabulary is now consistent ("Completed", "Needs confirmation"). Most of the earlier findings are fixed:
- **Stale errors:** mutation errors now reset whenever the selected run changes (`use-workflow-runtime.ts:122-128`).
- **Paging:** each group pages separately and keeps state across remounts (`workflow-inspector-store.ts`).
- **Wide split:** the detail column stays in place while the list scrolls (`workflow-panel.tsx:271`), Escape closes it (`:191`), and it says "Close details" (`:272`).
- **Capacity row:** now reads "+N slots" (`:213`).
- **Dry-run schema claim:** now limited to live runs (`:230`).
- **Apply copy:** states that every manifest file is applied and shows previewed vs total counts (`:328`, `:347`).

## Defects

**1. The task detail can open scrolled to the middle, hiding the title, status and Back button.**
- Your screenshot `short-height-detail-pane.png` shows this: the pane shows nothing but result text.
- How it happens: the script captures it right after shrinking the maximized split back to a narrow pane (`workflows-responsive.mjs:153-156`). The scroll position from the long list carries over into the detail.
- Why the existing fix misses it: `scrollIntoView({ block: "nearest" })` (`workflow-panel.tsx:128`) does nothing when the detail is taller than the pane and already covers it. It also only runs when the selection changes, not when the layout switches from split to single.
- The same thing happens when you pick a row deep in a long narrow list. The e2e only ever picks `task-0` (`workflows-responsive.mjs:135`), so it never catches this.
- Meanwhile the title takes focus with `preventScroll` (`:127`), so keyboard focus ends up somewhere you can't see.
- **Fix:** whenever the single-column detail first appears (new selection, or `wide` turning false while a task is selected), use `block: "start"` or reset the scroll container to the top.

**2. The panel can grab focus from the rest of the app while a run is active.**
- At `:129`, focus goes back to the last task row whenever `document.activeElement === document.body`. That check runs on every new run revision, which is every second while a run is running (`use-workflow-runtime.ts:28`).
- Clicking any non-focusable area anywhere in the app (for example, chat text) leaves focus on `body`. Within about a second the workflow row takes focus and scrolls into view. If that row is in Settled, Settled also opens (`:130-133`).
- After that, Space or Enter activates the row, and if the user scrolled the panel with the mouse, it jumps back.
- I found this by reading the code; no test checks it (there are no `activeElement` assertions in `workflow-panel.test.tsx`). Still, the condition is plain.
- **Fix:** only restore focus when it was inside the panel just before the row unmounted. For example, keep a "focus is inside" ref updated by `onFocus`/`onBlur` on the panel, and require it alongside the `body` check.

**3. Older attempts show an Apply button that is always disabled, with no reason given.**
- "Earlier reports & changes" are rendered with `current=false` (`:262`, `:281`). But `canApply` (`:270`) doesn't check whether the attempt is current.
- So on a live run that allows writes, a superseded snapshot shows the "Apply includes every file…" note and an "Apply N file changes" button. The button is disabled because `!current` (`:347`), and nothing says why.
- The browser mock only offers dry runs, so this can't show up in the captured evidence. It's a small, verified display gap.
- **Fix:** pass `canApply && current`, or say "Superseded by a later attempt".

## Limitations, not defects
- **Apply after previewing one file:** Apply still unlocks once the selected file has loaded (`:347`). The copy now discloses that the whole manifest is applied and shows previewed vs total counts, so this is an accepted design choice, not a bug.
- **Focus on remount:** when the inspector remounts with a task still selected, `previousSelection` starts out `null`, so the title takes focus again (`:98`, `:123-127`). That's acceptable when you switch runs, but could take focus if the pane remounts while you're typing elsewhere. I didn't confirm when remounts happen in the app.
- **Launching then navigating away:** if you start a launch and then pick "New workflow" before it finishes, the launch still completes and jumps you to the new run (`use-workflow-runtime.ts:93-101`). That's by design but could surprise someone.
- **Split detail height:** the wide detail is capped at `80dvh` (`:271`), which is based on the window, not the pane. When the window has a lot of chrome, the bottom of the detail can sit below the visible area until you scroll to the end of the list. I haven't measured this.
- **Simulated layouts:** the native, mobile and 200% layouts are simulated states. They show reflow, not real platform behaviour such as WebKit's handling of focus on click.

The run states, cancellation copy, unknown-task handling (slot held, no retry), the separation of reports from file changes, and per-group paging all look correct. Fix 1 and 2, which are each a few lines in `workflow-panel.tsx`, and I see nothing else material.
