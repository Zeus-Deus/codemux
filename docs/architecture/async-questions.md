# Asynchronous user questions

CodeMux exposes shared question presentation and durable state, with native delivery owned by the provider adapter. Only Codex opts in. There is no model-name allowlist: a runtime must actually emit an asynchronous question before the UI appears. Older Codex runtimes and other providers keep their existing question and approval paths.

## Native Codex contract

Codex 0.153 introduced `request_user_input_async`. Its app-server emits an `agentMessage` with `delivery: "async"` and `questions: [{ title, options? }]` through `item/started` and `item/completed`. CodeMux suppresses that item's ordinary text deltas and persists the completed question once, retaining the native conversation, source item, source turn, and subagent identity.

An answer is a new user message, not a response to an approval callback. The adapter reads the original conversation's current state and uses `turn/steer` with `expectedTurnId` and `clientUserMessageId` while a turn is active. A known turn-completion rejection allows one state refresh; if the root conversation is idle, the answer starts a normal follow-up turn. Neither path uses interruption or the existing “send now” queue operation.

Answers to a child conversation are directed to that child. If it is no longer active, CodeMux reports that limitation and keeps the answer; it never substitutes the parent conversation. Answer recovery requires the original native conversation and cannot silently fall back to a fresh session.

## Ownership and recovery

`agent_chat_questions` is a durable projection of `questions_asked` and `question_resolved` transcript events. Each state change and its transcript row commit together. A compare-and-set claim prevents competing windows from sending twice. Question state is independent of running, permission, and queue state.

States are pending, submitting, answered, dismissed, failed, and unknown. Explicit rejections allow retry. Transport loss and internal errors remain unknown because the provider may have accepted the message. “Check delivery” searches native history by client message ID. Resending after uncertainty is an explicit user action; the UI warns about possible duplication. Client IDs provide correlation, not a claim of exactly-once provider execution.

The UI preserves drafts locally, supports free text and multiple question sets, and never auto-submits a preselected answer. Native questions do not install global keyboard handlers or take focus from the composer. Questions remain actionable after a turn ends and after transcript replay. A separate workspace badge tracks unanswered questions without replacing running or permission status.

Accepted in-flight answers remain inside the original turn in transcript grouping, navigation, and subagent context. Late answers capture the ordinary pre-dispatch Git checkpoint. Its original transcript cutoff is retained, so a late acknowledgment cannot move the rollback boundary past generated output. Removing an accepted answer during rollback also removes its earlier dispatch claim, restoring the surviving question state. Question rows follow session promotion and cascade with conversation deletion; visible questions and accepted answers participate in search.

## Verification

Focused coverage includes native wire decoding, continued output before an answer, same-turn steering, completion races, late follow-ups, unsupported/rejected operations, lost acknowledgment and reconciliation, duplicate commands, provider opt-in, persistence, rollback, search, drafts, focus, transcript order, and existing callback question behavior. The installed Codex runtime was also exercised against a live model using synthetic prompts; see the [evidence notes](../evidence/async-questions/README.md).

Protocol reference: [Codex app-server: steer an active turn](https://learn.chatgpt.com/docs/app-server#steer-an-active-turn).
