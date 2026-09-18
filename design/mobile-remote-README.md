# Codemux mobile remote control — concept 01

Interactive prototype: https://passpage.space/v/UiZuoABPfEMj4LAjxvfhwy/

Published September 18, 2026. The service reports `expires_at: null` (no scheduled expiry). Update this same share with PUT when iterating; preserve the URL. The source is `design/mobile-remote/`. It is a standalone design artifact, not wired into the production application. All conversations, commands, files, Git operations, device information, and agent activity are sample data. State lasts for the current page session.

## Direction

Keep Codemux’s DM Sans / JetBrains Mono typography and dark, quiet workspace. Give phones one focused view, large touch targets, readable inputs, and persistent navigation. Chat, Changes, Files, and Terminal each have a full-width destination; More exposes Preview, panes, tasks, PRs, workspaces, devices, automations, and settings. The workspace title opens a searchable switcher. Tablet width restores a persistent sidebar. Short landscape viewports prioritize vertical space.

The prototype includes simulated sending, model selection, file attachment names (no upload), approval/decline, agent stopping, unified diffs, staging/commit, file edits, terminal commands, PR feedback, and theme switching. These illustrate interaction direction, not production feature parity. Terminal control keys and connection recovery are simulated. It is not an installable PWA yet.

## How to preview

Run `npm run dev`, then open `/design/mobile-remote/` on that server. For this session port 1420 was occupied; the task’s server ran on 1422 and was stopped after verification. Browser inspection used Codemux’s browser, not a system browser.

Fonts are embedded in the CSS because PassPage’s opaque sandbox origin prevents the same-origin font files from loading normally. Original font files and their license notices are in `assets/`.

## Relationship to issue #366

Remote control renders the React application locally through a transport shim (`src/remote/bootstrap.tsx`, `src/remote/shim.ts`, `src/main.tsx`). Layout can already differ per device. A mobile prototype or responsive shell does not require #366 first.

[Issue #366](https://github.com/Zeus-Deus/codemux/issues/366) should be implemented in parallel: active workspace selection currently comes from the shared app snapshot. It is needed for independent concurrent workspace navigation.

It does not by itself settle all cross-device presentation state:

- Workspace snapshots also contain active tab/surface selection, and surfaces contain active pane selection (`src/tauri/types.ts`). Keep presentation-only selection local where appropriate.
- `TerminalPane.tsx` sends PTY resize commands from visible clients. A phone and desktop viewing the same terminal can compete over rows/columns.
- `BrowserPane.tsx` sends viewport changes to the shared browser session on resize. Phone and desktop can compete over browser dimensions too.

Decide explicit control ownership or a stable/pinned session viewport for the latter two. A mobile pane picker should select a pane for local presentation without changing the shared desktop split tree.

## Current UI findings

- The titlebar sidebar toggle updates desktop `sidebarOpen`, while the mobile Sheet uses `openMobile`. Use the provider’s mobile-aware toggle.
- The right panel has a 360px minimum width; phone Files/Changes/Review need full-screen destinations or sheets.
- Desktop titlebar groups collide on narrow screens. Use an adaptive header rather than progressively shrinking its controls.
- `h-screen` shells, missing safe-area spacing, and dense composer controls need a coordinated viewport/keyboard treatment.
- Expose tab/pane actions that currently require hover, right-click, shortcuts, or precision dragging through labeled touch menus.

## Suggested implementation sequence

1. Agree the phone navigation, tool visibility, and visual direction using this concept.
2. Build an adaptive production shell using existing stores/components. Keep layout preference local and allow #366 to land independently.
3. Adapt composer, sheets, menus, approval/question forms, file/editor/diff/review views, terminal special keys, browser controls, and empty/disconnected states. Audit all existing desktop commands for a touch-accessible route.
4. Set viewport/safe-area policies; keep focused inputs at least 16px, touch controls at least 44px, pinch zoom enabled, and honor reduced motion. Preserve drafts, scroll position, and focus across navigation/reconnect.
5. Resolve shared terminal/browser dimension ownership; test actual remote transport with two clients.
6. Test physical iPhone Safari, Android Chrome, iPad, browser toolbars, keyboard transitions, landscape, text scaling, VoiceOver, connection loss/recovery, and background/resume. Add installable PWA behavior as a separate capability if desired.

## Verification performed

- `npm run check` passed. `node --check design/mobile-remote/app.js` passed.
- Codemux browser at 320×640: all five primary destinations fit without horizontal page/main overflow, and visible buttons meet 44×44 minimum dimensions.
- Inspected 390×844 phone and 768×1024 tablet screenshots, including unified-diff sheet. Captured 844×390 landscape; composer and navigation remain within the visible area and sidebar collapses.
- Exercised staging/commit, edit/save/reopen, terminal `pwd`, chat send, and approval decline successfully in the browser.
- Verified the published page executes JavaScript, has no horizontal page overflow, and loads embedded fonts.
- Browser emulation cannot prove iOS keyboard/PWA behavior. No live remote-control end-to-end validation has been performed for this standalone concept.

Visual evidence is in `design/mobile-remote-evidence/`. The before image uses the existing mock app on port 1420; after images use this concept on port 1422. All evidence uses sample data.

## Research

- [Cursor web/mobile agents](https://cursor.com/blog/agent-web): an agent-first mobile workflow and Home Screen access.
- [Cursor mobile documentation](https://prod.cursor.com/docs/cloud-agent/mobile): useful inspiration, but its mobile feature scope does not cover the full editor/terminal/file-browser parity requested here.
- [WebKit safe areas](https://webkit.org/blog/7929/designing-websites-for-iphone-x/).
- [WebKit dynamic viewport units](https://webkit.org/blog/12445/new-webkit-features-in-safari-15-4/).
- [MDN VisualViewport](https://developer.mozilla.org/en-US/docs/Web/API/VisualViewport): virtual keyboard can shrink the visual viewport without shrinking the layout viewport.
- [MDN viewport meta](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/meta/name/viewport): retain user zoom.
- [W3C enhanced target size](https://www.w3.org/WAI/WCAG22/Understanding/target-size-enhanced.html): 44×44 CSS pixel targets.

## Open-PR alignment review — September 18, 2026

Reviewed the descriptions of all 19 currently open PRs (#368, #369, #371–#387), the proposed UI conventions and Button implementation at `ui-pass/09-polish`, selected screenshot evidence (#380 and #386), and the composer-fade proposal. This was a read-only review of GitHub: no PR, branch, review, or merge state was changed. These are proposed changes, not assumed to have landed; recheck their final merged versions before implementation.

### Design baseline for the next iteration

The #372–#380 consistency stack is the intended baseline. The first prototype's fixed colors, sizes, radii, and custom SVGs are exploratory choices, not a second production design system.

- Consume the shared theme roles, rem type scale, radius ladder, surface/hairline tokens, focus styling, motion timings, and tabular-number treatment. Preserve user-selected themes and interface sizing. The prototype's green accent is provisional; do not make it the mobile brand color independently of the app's theme.
- Use `Button`, `PanelHeader`, and `Eyebrow`, and the normalized Lucide icon treatment. If mobile needs larger controls or headers, introduce a documented touch variant or shared responsive policy in the primitive. Avoid restoring the per-call-site geometry overrides these PRs remove. Phone targets should still reach 44px and text-entry fields at least 16px through that explicit policy.
- Preserve #380's quieter workspace cards: consistent spacing, restrained emphasis, no duplicate project/title names, and useful status cues. Adapt the hierarchy into the phone workspace picker.
- Respect the UI token contract when production components are implemented. The standalone prototype is not proof of compliance with that contract.
- The PR #380 body includes a correction that its conventions document has not fully caught up with: `thin-scrollbar` does NOT globally reserve a stable gutter; individual scrollers opt in. Use final code as authority for this detail.

### Interaction states to include on mobile

| Proposed change | Mobile consequence |
| --- | --- |
| [#381 composer fades while reading history](https://github.com/Zeus-Deus/codemux/pull/381) | Preserve the reading-space goal and jump-to-latest access; validate touch/focus restoration without relying on hover, and keep pending questions discoverable. Test with the software keyboard and long conversations. |
| [#382 slash-command execution preview](https://github.com/Zeus-Deus/codemux/pull/382) | Keep the command chip, description, inline highlighting, argument hints, and provider-aware suggestions. Maintain caret/text alignment and give the popup room above the keyboard. |
| [#383 pending agent questions](https://github.com/Zeus-Deus/codemux/pull/383) | Preserve answerable questions across remount/reconnect and make queued follow-ups visible. A navigation change must not imply an unanswered question has expired. |
| [#384 multiple PRs per workspace](https://github.com/Zeus-Deus/codemux/pull/384) | Show the aggregate PR count/state and a tappable list of the stack, rather than a hover-only card or a single PR assumption. |
| [#385 background-task wait line](https://github.com/Zeus-Deus/codemux/pull/385) | Let the wait status open task/subagent details directly, using a mobile sheet or full-screen view. Keep live thinking/writing markers distinct from actionable waiting states. |
| [#386 usage-limit recovery](https://github.com/Zeus-Deus/codemux/pull/386) | Make room for the countdown and Cancel / Try now / Resume actions, goal hints, and auto-resume preference. Backend owns scheduling; the phone must not create another scheduler. |
| [#368 worktree base branch](https://github.com/Zeus-Deus/codemux/pull/368) | Keep branch/base details available through a tap-accessible workspace detail view. |
| [#369 customizable footer](https://github.com/Zeus-Deus/codemux/pull/369) | Preserve access to pinned destinations and preferences; offer explicit reorder controls suitable for touch instead of relying only on precision dragging or hover labels. |
| [#371 hosted sign-in context](https://github.com/Zeus-Deus/codemux/pull/371) | Include branded sign-in, host selection, explanatory copy, and short-viewport scrolling in the eventual mobile journey. |
| [#387 app icon](https://github.com/Zeus-Deus/codemux/pull/387) | Reuse the final brand asset for a future Home Screen icon; the icon's warm-paper tile does not itself prescribe a new in-app palette. |

The next visual iteration should align with this baseline and demonstrate the incoming chat states before the responsive shell is connected to production. The currently published concept has not been changed by this review.

## Phone-format presentation update

At the user's request, the shared concept now stays in phone format on larger screens: a centered 390px app preview, with sheets anchored to the same frame. On a phone, the app fills the viewport naturally. This replaces the prior automatic tablet/sidebar presentation in the published prototype; earlier tablet screenshots are historical evidence from the first iteration. No iframe or scale transform is used. Actual responsive checks still use Codemux browser viewport resizing.

Verified desktop 1280×800 (390px app frame), phone 390×844 (edge-to-edge), and workspace-sheet bounds in both sizes. `npm run check` and JavaScript syntax validation passed. This update changes only the standalone design artifact; no open PRs or production code were modified.

## Competitive research

See [mobile-competition-research.md](mobile-competition-research.md) for the source-backed T3 Code, Orca, Conductor, and Superset review, the full-capability mapping, and recommended next prototype states. The current PassPage was left unchanged during this research.

## Concept 02 — workspace flow and phone onboarding

The published prototype now starts at a workspace overview, keeping the phone presentation introduced in the previous iteration. It adds:

- Needs-you / working / review grouping, workspace creation with project, agent and first instruction, plus searchable actions/workspaces.
- A session picker with separate in-memory chat drafts, queued follow-ups and simulated run completion.
- Pending question → answer → resolved workspace status.
- Reconnection preview that retains drafts and disables sending while disconnected.
- Full-screen file-by-file review, reviewed progress, file jump, and draft comments.
- A dismissible Home Screen suggestion and separate iPhone/iPad and Android setup guides.
- Installed-app notification onboarding, optional categories, Not now, blocked-permission preview, and an in-page sample notification that opens the relevant workspace.
- Neutral theme surfaces and semantic status colors closer to the pending Codemux UI consistency work.

All of these are prototype interactions. No installation, Notification permission API, service worker, push subscription, production remote control, Git operation, or agent execution is invoked. Files remain under `design/`; existing PRs and production sources are untouched.

Verification: syntax checks for both scripts; `npm run check`; browser interaction checks for new workspaces, questions, queued messages, session draft retention, reconnect draft retention, review comments/progress, and notification onboarding. At 320×640 all seven main destinations had no horizontal page/main overflow and visible buttons met 44px minimum dimensions. iOS/Android guide and notification dialogs fit the 320px viewport without horizontal overflow. Screenshots inspected at phone size and in the desktop phone preview. Physical iPhone Safari keyboard and real push delivery still require device tests during implementation.

## Required production work: installable hosted app and notifications

User direction: installation and iOS/Android notifications are required features of the mobile remote-control experience, not merely optional research ideas. The intended installation origin is **app.codemux.org**, whose existing hosted bootstrap already supports sign-in → device list → connect. The prototype is only illustrating the UX.

1. Add the manifest, appropriate brand icons and standalone launch behavior at the hosted app origin, with correct start URL/scope and authentication/deep-link restoration. Keep the usable browser path available without installation.
2. Show a dismissible installation suggestion on eligible mobile browsers; retain dismissal, avoid repeated nagging, and suppress it while running standalone. Use an actual available install event on supporting browsers; provide iOS Share → Add to Home Screen guidance otherwise. Do not equate an install-button click with successful installation. Android wording varies by browser. Opening standalone removes ordinary browser chrome, not necessarily all system status UI.
3. On iPhone/iPad, gate push onboarding on a supported Home Screen app context. Show the benefit and preferences first; request system permission only after the user taps Enable. Handle unsupported, default, granted, denied, and later-revoked permission states. A denied permission cannot be reset by the website itself.
4. Build a service worker and authenticated push-subscription lifecycle at the hosted origin, plus event-to-push delivery that does not depend on the phone holding an active remote-control WebSocket. Merely making the UI installable does not provide notifications. Desktop agent activity must reach the notification backend/relay with the correct user/device/workspace identity.
5. Provide notification categories for questions/approvals, review-ready work, and failures; deduplicate events and respect dismissal/revocation/preferences. Notification clicks should restore authentication if needed, select the correct host/workspace/session locally, and land at the relevant question/review state.
6. Test actual iPhone Safari → Home Screen → first open → permission → background notification → tap-to-workspace, plus decline, revocation, sign-out, host offline, and reconnect cases. Test equivalent Android Chrome behavior on an emulator or device; browser viewport resizing cannot prove push delivery or OS installation behavior.

Sources: [MDN installation guidance](https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Installing), [WebKit Home Screen Web Push](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/). iOS/iPadOS Home Screen Web Push is supported from 16.4, with permission requested from a user action. No Apple Developer Program membership is required for standards-based Web Push.
