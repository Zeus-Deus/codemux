## Verdict: all three issues are fixed and I found no remaining defects

I only used Read, Glob and Grep. The evidence comes from a browser mock (`providerCalls: 0`, `modelTokens: 0`), so it says nothing about paid runs, the real Tauri/WebKit window or mobile behaviour.

**1. Detail opening scrolled to the middle: fixed.** The detail now resets when you select a different task and when the layout switches from split to single. In single layout it sets `detail.scrollTop = 0` and scrolls with `block: "start"` (`workflow-panel.tsx:139-148`). `short-height-detail-pane.png` now shows "Back to checkpoints", the title, the status and the route at 363×378. Tests cover both the plain resize and the case where you're typing in an outside composer (`workflow-panel.test.tsx:345-354`, `:374-402`).

**2. Panel taking focus from the rest of the app: fixed.** Focus is tracked at document level (`:122-134`). Focus moving outside the panel, a pointer press outside it, or the window losing focus all clear the "focus is inside" flag. Focus only goes back to a task row when that flag is set (`:149`). After a resize, the title only takes focus if focus was already in the panel (`:145`). A removed row fires no focus event, so the flag stays set and the Back/settle behaviour still works. The test at `:404-421` checks that nothing grabs focus after you click outside.

**3. Disabled Apply button on older attempts: fixed.** Apply now only renders when `canApply && current` (`:368`). Older attempts show "Earlier attempt · review only…" instead (`:367`). Tests are at `:244-245` and `:517`.

**Route trim-to-null: correct.** Model and effort are trimmed, and empty values become `null`, only at launch (`:49`). Validation uses the same trimmed values (`workflow-setup.tsx:33-34`, `:90`), so what you type isn't changed while you edit.

**OpenCode and Cursor checks: they match the backend.**
- The OpenCode format check `^[A-Za-z0-9._-]+\/\S+$` behaves the same as the backend's `validate_route` (`capabilities.rs:59-79`). Both split on the first `/` and use the same provider characters and no-whitespace rule.
- Cursor with an effort set blocks only the live run. Dry runs stay enabled, which also matches the backend: the preflight check returns early for dry runs (`commands/workflows.rs:211-213`).

## Notes, not defects
- **Requirements screenshot:** in `setup-provider-requirements-narrow-pane.png` the footer shows the "Live execution is unavailable…" message rather than the model/effort hint. That's because the mock marks every provider "Dry run only", and that message takes priority (`workflow-setup.tsx:79`). The field-level hint does show. The footer hint only appears once a provider is live, and the screenshots don't show that case.
- **Layout count:** of the 19 entries in `verification.json`, 3 are mobile-shell boundary checks with `workflowVisible: false`, not pane measurements. Strictly, 16 layouts were measured.
- **Test count mismatch:** `provenance.json` still records 67 affected tests (30 panel + 24 presentation + 13 mock). That doesn't match the 112 you quoted, so the doc needs updating, though the code is fine.
- **Integrated flows:** `integrated/verification.json` lists 19 checked flows, including historical artifacts having no current authority.
