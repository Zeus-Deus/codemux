# Mobile developer-tool research

Research date: September 18, 2026. Three sub-agents investigated T3 Code, Orca, and Conductor/Superset. The goal is full Codemux capability through the browser, with a phone-appropriate interface. This research changes no production code, open PRs, or competitor repositories. The current PassPage remains as reviewed by the user.

## What the evidence supports

These sources have different scope: repository code demonstrates implementation, documentation describes intended behavior, and store screenshots show curated examples. We did not run the competitors against real agent sessions or use the user's TestFlight access. A native implementation is a reference for interaction design, not something to paste directly into a browser application.

## Orca

Confirmed React Native/Expo mobile source at commit `a98314e8bb8e33d1129d8091d6bc831b763380a7`. Official documentation describes an iOS/Android beta companion and links App Store/TestFlight. Its stated scope is not a full editor, so Codemux should learn its presentation without adopting its feature ceiling.

Useful implemented patterns:

- A compact workspace/connection header and scrollable session strip, with creation/quick actions kept outside the scrollable area. [Header implementation](https://github.com/stablyai/orca/blob/a98314e8bb8e33d1129d8091d6bc831b763380a7/mobile/src/session/MobileSessionHeader.tsx).
- Narrow screens push files/source control into separate views; wider screens dock panels only when adequate room remains for the primary view. [Panel policy](https://github.com/stablyai/orca/blob/a98314e8bb8e33d1129d8091d6bc831b763380a7/mobile/src/session/session-panel-host.ts).
- The terminal command dock moves above the keyboard without resizing the PTY merely because the keyboard appeared. A dismiss-keyboard control remains reachable outside the scrolling key strip. [Command dock](https://github.com/stablyai/orca/blob/a98314e8bb8e33d1129d8091d6bc831b763380a7/mobile/src/session/MobileSessionCommandDock.tsx), [layout rationale](https://github.com/stablyai/orca/blob/a98314e8bb8e33d1129d8091d6bc831b763380a7/mobile/src/session/MobileSessionContentRow.tsx).
- Buffered input stays editable during disconnection, while sending is gated. Draft restoration avoids overwriting newer edits. [Draft handling](https://github.com/stablyai/orca/blob/a98314e8bb8e33d1129d8091d6bc831b763380a7/mobile/src/terminal/use-buffered-terminal-drafts.ts).
- Diff review includes previous/next file, comments, reviewed progress, and staging actions. [Review footer](https://github.com/stablyai/orca/blob/a98314e8bb8e33d1129d8091d6bc831b763380a7/mobile/src/components/MobileDiffReviewFooter.tsx).

[Official mobile overview](https://github.com/stablyai/orca/blob/a98314e8bb8e33d1129d8091d6bc831b763380a7/docs/site/content/docs/mobile.mdx) · [Official desktop/phone showcase](https://raw.githubusercontent.com/stablyai/orca/a98314e8bb8e33d1129d8091d6bc831b763380a7/docs/assets/feature-wall/mobile-companion-app-showcase.jpg).

## T3 Code

Inspected the user's local checkout at `/home/zeus/projects/t3code`, commit `fff33f9e851912363c5b1f3ac65598be35eb5f0d` (origin `pingdotgg/t3code`). It contains a substantial React Native app. Public release status was not established: the root instructions and mobile README disagree. No representative mobile screenshot output was present in the primary checkout, so the findings are based on implementation rather than invented visual evidence.

- Phone navigation is thread list → chat → dedicated files, review, and terminal views. Tablet split presentation requires both ≥720px width and ≥600px height, avoiding a cramped sidebar on landscape phones. [Layout policy](https://github.com/pingdotgg/t3code/blob/fff33f9e851912363c5b1f3ac65598be35eb5f0d/apps/mobile/src/lib/layout.ts), [navigation rules](https://github.com/pingdotgg/t3code/blob/fff33f9e851912363c5b1f3ac65598be35eb5f0d/apps/mobile/src/lib/adaptive-navigation.ts).
- Keyboard-aware composer layout reserves matching transcript space. Pending questions can occupy an expanded answer card or compact attention bar while retaining editor state. Manual scrolling suspends live following. [Thread detail](https://github.com/pingdotgg/t3code/blob/fff33f9e851912363c5b1f3ac65598be35eb5f0d/apps/mobile/src/features/threads/ThreadDetailScreen.tsx), [live-follow policy](https://github.com/pingdotgg/t3code/blob/fff33f9e851912363c5b1f3ac65598be35eb5f0d/apps/mobile/src/features/threads/thread-feed-live-follow.ts).
- Terminal has real special keys, session switching, text sizing, and keyboard show/hide. Review has changed-file navigation, viewed markers, line selection, and comments. File browsing/preview was verified in source; full mobile editing was not. [Terminal](https://github.com/pingdotgg/t3code/blob/fff33f9e851912363c5b1f3ac65598be35eb5f0d/apps/mobile/src/features/terminal/ThreadTerminalRouteScreen.tsx), [review](https://github.com/pingdotgg/t3code/blob/fff33f9e851912363c5b1f3ac65598be35eb5f0d/apps/mobile/src/features/review/ReviewSheet.tsx).
- Disconnected composition has an explicit Queue label backed by a persistent outbox carrying message identity, attachments, and configuration. Brief connection changes are debounced to avoid flashing UI. This is a larger behavior than simply adding a Retry button. [Outbox model](https://github.com/pingdotgg/t3code/blob/fff33f9e851912363c5b1f3ac65598be35eb5f0d/apps/mobile/src/state/thread-outbox-model.ts), [connection title](https://github.com/pingdotgg/t3code/blob/fff33f9e851912363c5b1f3ac65598be35eb5f0d/apps/mobile/src/features/home/WorkspaceConnectionTitle.tsx).

For Codemux: keep the focused destinations and preserve draft/scroll/session state. A durable offline outbox is a possible later enhancement; do not imply automatic sending until command identity and reconciliation are implemented. Avoid native-only glass effects or keyboard internals as browser requirements.

## Conductor

An official iPhone App Store listing now exists. Apple metadata reports initial release September 14, 2026, and version 1.0.1 on September 16. This is newer evidence than the homepage's “iOS SOON” label. No public mobile source repository was confirmed.

The official iPhone screenshots show a sparse pinned-workspace list, prompt entry above the keyboard, a model/settings bottom sheet, and conversation with a contextual Ready to merge action. This makes Conductor a useful visual reference for restraint and progressive disclosure, but its screenshots do not establish every feature or its reliability.

[Official listing and screenshots](https://apps.apple.com/us/app/conductor-build/id6791228564) · [Apple release metadata](https://itunes.apple.com/lookup?id=6791228564).

For Codemux: keep primary screens quiet and make status actionable. Advanced controls can live in labeled sheets; they should remain discoverable rather than requiring hidden gestures. Do not copy a merge shortcut without the associated review/check context.

## Superset

Confirmed official iPhone listing and substantial public React Native/Expo mobile source at commit `b6394996b43521b9d887e6c5bbc1b4df3bc1e0f5`. Apple metadata reports initial release September 9, 2026, version 1.0.1 on September 17. Some website/roadmap text still describes mobile as in progress. iOS availability does not prove released Android or mobile-web parity.

Official screenshots show project-grouped workspaces, host/sorting controls, PR detail with checks before files, full-screen unified diffs, and a terminal workspace with session controls and special keys.

Source confirms useful details:

- Session selecting/reordering/closing uses a separate sheet because dragging tiny tabs conflicts with horizontal scrolling. [Sessions sheet](https://github.com/superset-sh/superset/blob/b6394996b43521b9d887e6c5bbc1b4df3bc1e0f5/apps/mobile/screens/%28authenticated%29/workspace/%5Bid%5D/sessions/SessionsSheet.tsx).
- Last session selection is stored per device, keyed by workspace. This is directly relevant to Codemux #366 and subsequent pane-selection work. [Selection store](https://github.com/superset-sh/superset/blob/b6394996b43521b9d887e6c5bbc1b4df3bc1e0f5/apps/mobile/screens/%28authenticated%29/stores/lastSessionTabStore/lastSessionTabStore.ts).
- A small terminal snapshot cache paints recent output while reconnecting, then catches up from a stream position. [Terminal cache](https://github.com/superset-sh/superset/blob/b6394996b43521b9d887e6c5bbc1b4df3bc1e0f5/apps/mobile/lib/terminal/warmTerminalCache.ts).
- Review disables full-screen native back gestures because they conflict with diff scrolling. Browser edge navigation is controlled differently, but the gesture conflict is an important phone test case. [Navigation configuration](https://github.com/superset-sh/superset/blob/b6394996b43521b9d887e6c5bbc1b4df3bc1e0f5/apps/mobile/app/%28authenticated%29/_layout.tsx).

[Official listing and screenshots](https://apps.apple.com/us/app/superset-100-coding-agents/id6788926383) · [Apple metadata](https://itunes.apple.com/lookup?id=6788926383) · [Inspected mobile source](https://github.com/superset-sh/superset/tree/b6394996b43521b9d887e6c5bbc1b4df3bc1e0f5/apps/mobile).

## Recommended changes to Codemux's next prototype

These are design recommendations, not claims that the existing mockup already implements them.

1. **Add a workspace overview focused on what needs attention.** The existing title picker remains a quick switcher. A fuller overview should show Needs you, Working, Ready for review, and recent work, with project/host filters and a clear New workspace action. Returning from a phone notification should land on the exact pending item.
2. **Keep the current five tool destinations initially.** The user likes the direction: Chat, Changes, Files, Terminal, More. Add an explicit current-agent/session selector within the workspace. Do not replace familiarity with new navigation merely because a competitor uses it.
3. **Give pending work one coherent place above the composer.** Adapt the existing composer strip: unanswered question, usage-limit countdown/resume, background tasks, queued message, or interrupted goal. Expand for detail. Preserve the incoming PR behavior and avoid piling multiple banners over half a phone screen.
4. **Turn Changes into an efficient review flow.** File list → full-width unified diff → next/previous file, reviewed progress, comment actions, then a commit/PR summary. Keep access to staging, unstage, push, checks, and multiple PRs. Do not require horizontal swipes for navigation where the same gesture scrolls code.
5. **Design keyboard-open screens, not just keyboard-closed screenshots.** Composer and Send stay reachable; secondary navigation can collapse temporarily. Terminal gets Esc, Tab, Ctrl, arrows, paste, and an explicit keyboard-dismiss action. Buffered command input and live terminal key input must be distinguishable. Keyboard dismissal must not also send or lose text.
6. **Make reconnect behavior visible and trustworthy.** Preserve per-session drafts, reading location, selected tool, pending review comments, and file context. Clearly distinguish draft, queued, sent, failed, and uncertain execution. A retry must not silently execute a command twice. Agent work stays on the desktop.
7. **Add a touch-accessible command search.** The app already has a command registry. A visible search/action entry gives every supported command a route, including less common tools, without overcrowding the bottom bar. Menus and labeled controls must cover shortcuts, hover, right-click, and precision drag actions.
8. **Use Codemux's shared design primitives.** Carry forward the open UI consistency stack (#372–#380), semantic theme colors, typography, radius and surface ladders, and the new chat states (#381–#386). Introduce shared touch sizing rather than individual overrides. Competitor styling should not replace Codemux's identity.

## Feature parity means equivalent actions, adapted presentation

| Existing capability | Suggested phone route | Evidence needed before calling it complete |
| --- | --- | --- |
| Projects, workspaces, archive, host selection | Workspace overview, picker, lifecycle menu | Create/open/rename/archive/restore, and independent device selection |
| Agent chat and multiple sessions | Chat + session picker + action sheet | Model/options, attachments, slash commands, questions, approvals, queue/edit/stop/resume |
| Files and editing | Files → full-screen document/editor | File/content search, read/write, keyboard/selection, unsaved draft recovery |
| Changes and Git | Changes → focused diff/review | Stage/unstage, commit/push, conflicts, PR stack, checks, comments, supported merge actions |
| Terminal and multiple panes | Terminal + pane/session picker | Interactive programs, special keys, text selection/copy/paste, disconnect/reconnect |
| Browser preview and dev commands | Preview + run/ports actions | Pointer/keyboard forwarding, useful viewport policy, URL/navigation, dev-server status |
| Tasks, agents, goals, automations | Pending activity + More | Monitoring and control actions remain reachable |
| Preferences, devices, skills, MCP, usage | Searchable Settings/More | All applicable settings/actions have a usable touch route |

Codemux source checked: `src/components/overlays/command-palette.tsx`, `src/lib/settings-sections.ts`, `src/lib/footer-actions.ts`, chat ComposerStrip, and remote transport/connection components. The current prototype covers examples, not this entire matrix.

## Browser-specific constraints and opportunities

A Home Screen web app can open standalone on iOS. Web Push is supported for Home Screen web apps on iOS/iPadOS 16.4+, subject to user permission requested from an explicit user action. That makes an installable browser client and attention notifications a feasible direction without shipping an App Store binary. Neither has been implemented in the current PassPage. [WebKit documentation](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/).

Do not assume native Expo keyboard, terminal, notification, or background execution code transfers directly to Safari. Implement the UX using web APIs and verify it on actual iPhone Safari/Home Screen and Android Chrome. The phone can disconnect or be suspended while the desktop agent continues working.

Issue #366 remains needed for independent workspace navigation. Shared terminal dimensions and browser-session viewport ownership require additional care. Codemux already has a pinned browser-viewport option in `BrowserPane.tsx`; evaluate reuse before adding another mechanism. Keyboard opening should not inadvertently reshape the other device's terminal.

## Proposed next design slice

Keep the approved phone frame and basic navigation. Prototype four realistic states next: a workspace waiting for an answer, a running agent with a queued follow-up, a file-by-file review, and reconnecting with an intact draft. Test each with the keyboard visible and preserve return/back behavior. Then map the remaining desktop actions through the same shared navigation/components before claiming full parity.
