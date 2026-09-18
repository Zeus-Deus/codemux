# Mobile remote control

Implemented on `mobile-responsive-remote-control`, based on PR #391's independent-client work (8bcdbc11). This branch does not modify or merge any other open PR. The original PassPage prototype under `design/` remains separate from the production implementation.

## Using it

Deploy the web bundle and an updated desktop together. On a phone, open the normal remote-control URL, including `https://app.codemux.org` for hosted access, authenticate and choose the desktop. Browsers below 1024 CSS pixels use the mobile shell. Native Tauri windows retain their desktop layout even when narrow.

- **Workspaces:** search, attention/working/settled filters, create, settle/unsettle, and open a workspace. Working or blocked agents resurface automatically. The Actions/Tools sheet includes rename, pin, mute, archive, projects, cloning, settings, automations and pull requests.
- **New workspaces:** New opens the GUI chat composer directly: choose project, provider/model and checkout/worktree, then send the first prompt. Shared create-workspace commands route into the same mobile flow. The mobile add menu offers New chat, Terminal and Browser, without CLI presets or Shift-click behavior. Existing CLI sessions remain accessible. Desktop creation behavior is unchanged.
- **Development:** existing providers, models, prompt composer, approvals, images, agent tasks, subagents, file explorer/editor, changes and review remain the shared production components. Phone navigation shows one tool or split pane at a time. A single compact header replaces the persistent tabs and bottom navigation: tap the title for Sessions, or Tools for files, changes and workspace details. Tool panels overlay the mounted conversation without changing its width, preserving drafts and virtualized row measurements. Workspace, tab and pane selection belong to the browser; files, running agents and other mutations remain shared with the desktop.
- **Terminals:** touch keys and an explicit keyboard button. By default the phone follows the desktop terminal dimensions with scrolling. “Fit to phone” explicitly resizes the shared PTY; another viewer can also resize it. Remote browser previews follow the host viewport instead of resizing the agent browser on mount.
- **Connection loss:** the composer retains editable drafts and blocks submission until reconnected. The application needs a live desktop connection for work; installing it does not turn the phone into an agent host.

## Installation and notifications

The bell menu and Settings → Notifications explain installation and offer an explicit notification opt-in. No permission prompt is triggered by incoming agent events.

**iOS/iPadOS:** Share → Add to Home Screen, then open the installed icon and enable notifications. Web Push requires iOS/iPadOS 16.4 or later and a supported Home Screen web app. No Apple Developer account or native app build is involved. [WebKit platform announcement](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/).

**Android:** use the captured browser Install prompt where available, or Install app / Add to Home screen in the browser menu. Push support is detected instead of assuming an OS/browser supports it. Denied permission and insecure origins have actionable explanations.

Each desktop has its own service-worker push scope and VAPID key. The desktop encrypts notification payloads and sends directly to the browser's push service, so delivery does not depend on the phone keeping its WebSocket alive. Categories are questions/approvals, completion and failures. A failed GUI turn does not also emit completion. Desktop focus does not suppress phone alerts; workspace mute does.

Push registration is bound to the authenticated, approved remote session on the server, never a caller-supplied session ID. Delivery rechecks session revocation, current subscription and category preferences. The endpoint allowlist covers Safari, Chrome, Firefox and Windows push services; redirects and arbitrary hosts are rejected. Expired endpoints are pruned. Private key and subscriptions live in the user's Codemux config directory in `web-push.json` (0600 on Unix), outside synced settings. Logs do not include endpoints, keys or payloads.

Tapping an alert opens the correct origin/device/workspace/pane through normal authentication. Hosted notification targets survive the GitHub OAuth round trip. The worker caches no credentials, source files, API responses or workspace data; a failed offline navigation displays a reconnect screen.

## Deployment

`npm run build` includes `/manifest.webmanifest`, `/sw.js` and `/icons/`. The existing hosted release bundle and Tauri embedded assets include these files. Serve the hosted bundle at the origin root over HTTPS. Keep `/sw.js` revalidatable, serve it as JavaScript, and allow same-origin workers under the site's CSP. Service-worker scopes must remain same-origin. No additional cloud push service or API deployment is required: the updated desktop owns push delivery and needs outbound HTTPS access to the browser push services.

This work has **not** been deployed to app.codemux.org. It also does not grant remote clients native OS/account administration or destructive worktree removal; the explicit remote command policy is retained. Normal development actions needed by review, presets and automations are included in that policy.

## Verification

Completed locally:

- TypeScript check and production build.
- Targeted tests covering remote state/activation and shim, mobile viewport and pinch-zoom behavior, composer and image attachment, terminal teardown and explicit mobile resizing, workspace creation, app shell/right panel, pull requests, hosted OAuth, Web Push permission/rollback/unsubscribe, service-worker navigation/offline handling and notification routing.
- Rust check and targeted push tests for endpoint validation, subscription keys and failure/completion deduplication.
- Real HTTP/WebSocket integration: pair and authenticate, bootstrap, register/configure/remove push, reject private endpoints, preserve independent client selection, create a bound chat workspace, archive/restore without moving the desktop, and reject forbidden commands.
- Codemux browser at 320×568, 390×844, 768×1024 and 1280×800: creation, settle/unsettle, chat, draft preservation, files/file preview, settings, installation guidance and pull-request navigation. Editable fields use at least 16px without disabling accessibility zoom; the composer and its text mirror have matching font metrics. No document-width overflow in the checked screens.
- Production preview: manifest loads, JavaScript MIME is correct, root and per-host service workers register/activate, and the pairing form fits at phone width. Temporary validation registrations were removed.

The browser uses mock development data for visual work; the WebSocket test uses an isolated backend and sends no real push notification. Physical iPhone/Android keyboard, OS permission sheets and closed-app push delivery are **not yet device-verified**.

Before release, on physical iPhone and Android:

1. Install from HTTPS and opt in using the bell/settings screen. Send the test notification, then background/close the app and trigger approval, success and failure on the desktop.
2. Tap each alert, including after login expires and while connected to another desktop. Confirm it opens the intended workspace and pane.
3. Type a multiline prompt, rotate, show/hide the keyboard, open model selection, attach an image, switch workspaces and return. Check draft/caret, safe areas and accessible pinch zoom.
4. Drop connectivity, verify the retained draft and disabled Send, then reconnect. Revoke the remote session on the desktop and verify push stops. Test notification denial and re-enabling.
5. Keep desktop and phone open simultaneously: verify browsing is independent and shared edits, prompts and explicit terminal resizing behave consistently.

## Visual evidence

| Screen | Capture |
| --- | --- |
| New GUI workspace | [390px](assets/mobile-remote/new-chat-390.png) |
| Phone workspaces | [390px](assets/mobile-remote/workspaces-390.png) |
| Small-phone chat | [320px](assets/mobile-remote/chat-320.png) |
| File explorer | [320px](assets/mobile-remote/files-320.png) |
| File preview | [320px](assets/mobile-remote/file-preview-320.png) |
| Phone chat and draft | [390px](assets/mobile-remote/chat-390.png) |
| Pull request review | [390px](assets/mobile-remote/review-390.png) |
| Production pairing | [390px](assets/mobile-remote/pairing-390.png) |
| Tablet workspaces | [768px](assets/mobile-remote/workspaces-tablet.png) |
| Desktop layout | [1280px](assets/mobile-remote/desktop-1280.png) |

## Focused mobile navigation refinement

Read-only research of Remodex and Hermex found the same useful pattern: a dedicated phone conversation destination with a compact header and secondary controls disclosed in menus/sheets. Both preserve the underlying input while changing composer presentation. Codemux applies the navigation/disclosure pattern while retaining its established composer for provider/model selection and input stability.

- [Remodex phone navigation](https://github.com/Emanuele-web04/remodex/blob/fc3aedea8aade44b50ac944b4801e7a16733ee4d/CodexMobile/CodexMobile/ContentView.swift#L480)
- [Remodex contextual toolbar](https://github.com/Emanuele-web04/remodex/blob/fc3aedea8aade44b50ac944b4801e7a16733ee4d/CodexMobile/CodexMobile/Views/Turn/Core/TurnToolbarContent.swift#L41)
- [Hermex conversation navigation](https://github.com/uzairansaruzi/hermex/blob/fbec84ff49749f0e3fae566a314c928b7137ca88/HermesMobile/Features/Chat/ChatView.swift#L803-L837)
- [Hermex persistent input and composer disclosure](https://github.com/uzairansaruzi/hermex/blob/fbec84ff49749f0e3fae566a314c928b7137ca88/HermesMobile/Features/Chat/ChatComposerView.swift#L680-L780)

The conversation now has one 53px header. Sessions (including terminal, browser, editor, diff and split panes) are in a title-triggered bottom sheet. Files, Changes, Review, Tasks, Subagents, Browser and git/issue details are in Tools. The keyboard dismissal control appears in the header only while needed. Touch targets remain at least 44px; sheet motion respects reduced motion and focus returns to its trigger. Installation and notification settings remain available from Home and app actions. Native desktop layout is unchanged.

Focused tests cover ordered tab/pane activation, failures, editor dirty/status labels, sheet focus restoration, launching a new session, draft preservation through tools/home, and keeping the hidden conversation full-width under tools. These are browser interactions, not a claim of identical UIKit/SwiftUI effects or physical-device verification.

Latest captures supersede the earlier persistent-footer screenshots:

| Focused screen | Capture |
| --- | --- |
| Conversation | [390px](assets/mobile-remote/focused-chat-390.png) |
| Workspace tools | [390px](assets/mobile-remote/focused-tools-390.png) |
| Sessions | [390px](assets/mobile-remote/focused-sessions-390.png) |
