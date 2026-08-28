//! Hermes ACP adapter integration coverage.
//!
//! Everything here runs against `fake_hermes_acp`, a fixture whose frames
//! are transcribed from a live `hermes -p codemuxdev acp` capture — so a
//! failure here means the adapter drifted from the protocol, not from a
//! guess about it. The one live smoke test is ignored: it needs an
//! installed, authenticated Hermes and must only ever touch the throwaway
//! `codemuxdev` profile.
//!
//! The fixture journals every method it receives to the file named by
//! `FAKE_HERMES_JOURNAL`. That journal is how these tests tell apart
//! things the event stream cannot show: whether a detach said goodbye to
//! the agent, whether a profile change poked a live session or launched a
//! second child, and which option id an approval actually answered with.

use std::path::PathBuf;

use codemux_lib::agent_provider::hermes::{HermesAgentProvider, HermesProviderConfig};
use codemux_lib::agent_provider::{
    AgentProvider, ApprovalDecision, CompletedItem, ProviderError, ProviderEventStream,
    ProviderRuntimeEvent, SendTurnInput, StartSessionInput, ThreadId, TurnStatus,
};
use futures_util::StreamExt;
use tempfile::TempDir;
use tokio::time::{timeout, Duration};

/// Provider wired to the `fake_hermes_acp` fixture instead of a real CLI.
fn fixture_provider() -> HermesAgentProvider {
    HermesAgentProvider::new(HermesProviderConfig {
        binary: PathBuf::from(env!("CARGO_BIN_EXE_fake_hermes_acp")),
        event_channel_capacity: 1024,
    })
}

/// One journal file per launched child, so a test that starts several can
/// tell which child was handed what.
struct Journal {
    _dir: TempDir,
    path: PathBuf,
}

impl Journal {
    fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(format!("{name}.log"));
        Self { _dir: dir, path }
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn saw(&self, entry: &str) -> bool {
        self.lines().iter().any(|line| line == entry)
    }

    /// Block until the child has journalled `entry`.
    ///
    /// Used to pin a Stop to the window where the prompt is genuinely in
    /// flight. A Stop that lands before the prompt goes out is honored
    /// too — the worker abandons the turn instead of sending it — but
    /// then there is nothing on the child to cancel, and this test is
    /// about the delivery.
    async fn wait_for(&self, entry: &str) {
        timeout(Duration::from_secs(20), async {
            while !self.saw(entry) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the child never received `{entry}`: {:?}", self.lines()));
    }
}

/// A start with no model or permission-mode selection, so the journal
/// carries only what the adapter itself decided to send.
fn fixture_start_input(
    thread_id: &str,
    profile: Option<&str>,
    journal: &Journal,
) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(thread_id.into()),
        cwd: std::env::current_dir().expect("test cwd"),
        model: None,
        profile: profile.map(str::to_string),
        resume_cursor: None,
        permission_mode: None,
        effort: None,
        context_window: None,
        fast_mode: false,
        additional_directories: vec![],
        env: Some(
            [(
                "FAKE_HERMES_JOURNAL".to_string(),
                journal.path.display().to_string(),
            )]
            .into_iter()
            .collect(),
        ),
        workspace_id: None,
        extra: serde_json::Value::Null,
        recorded_usage_baseline: None,
    }
}

fn fixture_turn(thread_id: &str, text: &str) -> SendTurnInput {
    SendTurnInput {
        thread_id: ThreadId(thread_id.into()),
        text: text.into(),
        images: vec![],
        model_override: None,
        effort_override: None,
        permission_mode_override: None,
        client_nonce: None,
        display_text: None,
        skill_invocations: vec![],
        turn_checkpoint: None,
    }
}

/// Everything the provider emitted from subscription up to (and
/// including) `SessionConfigured`, which start-up publishes only once the
/// session is fully established.
async fn events_until_configured(events: &mut ProviderEventStream) -> Vec<ProviderRuntimeEvent> {
    timeout(Duration::from_secs(20), async {
        let mut collected = Vec::new();
        while let Some(event) = events.next().await {
            let done = matches!(event, ProviderRuntimeEvent::SessionConfigured { .. });
            collected.push(event);
            if done {
                return collected;
            }
        }
        panic!("event stream closed before the session was configured");
    })
    .await
    .expect("session start timed out")
}

/// Everything from subscription up to (and including) `TurnCompleted`.
async fn events_until_turn_completed(
    events: &mut ProviderEventStream,
) -> Vec<ProviderRuntimeEvent> {
    timeout(Duration::from_secs(20), async {
        let mut collected = Vec::new();
        while let Some(event) = events.next().await {
            let done = matches!(event, ProviderRuntimeEvent::TurnCompleted { .. });
            collected.push(event);
            if done {
                return collected;
            }
        }
        panic!("event stream closed before turn completion");
    })
    .await
    .expect("turn timed out")
}

fn assistant_texts(events: &[ProviderRuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::AssistantText { text },
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn user_texts(events: &[ProviderRuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderRuntimeEvent::UserMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Every `ToolUse` item, as `(tool_name, input)`.
fn tool_uses(events: &[ProviderRuntimeEvent]) -> Vec<(String, serde_json::Value)> {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderRuntimeEvent::ItemCompleted {
                item:
                    CompletedItem::ToolUse {
                        tool_name, input, ..
                    },
                ..
            } => Some((tool_name.clone(), input.clone())),
            _ => None,
        })
        .collect()
}

fn warnings(events: &[ProviderRuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderRuntimeEvent::RuntimeWarning { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// Start-up is where Hermes differs most from every other ACP agent: the
/// session catalogue rides on the `session/new` response, and the agent
/// pushes `available_commands_update` and `usage_update` unprompted the
/// moment the session exists — with no turn in flight. An adapter that
/// only accepts updates inside a turn loses the context meter on every
/// start and warns twice for the privilege.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_start_adopts_the_agents_catalogue_and_its_unprompted_frames() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("start");
    let thread_id = "hermes-start";

    let session = provider
        .start_session(fixture_start_input(thread_id, Some("codemuxdev"), &journal))
        .await
        .expect("start fixture session");
    let collected = events_until_configured(&mut events).await;

    assert_eq!(session.session_id.0, "hermes-codemuxdev-session");
    assert_eq!(
        session.resume_cursor,
        Some(serde_json::json!({
            "schemaVersion": 1,
            "sessionId": "hermes-codemuxdev-session",
            "profile": "codemuxdev",
        }))
    );
    // The `-p` flag really reached the launch line; the profile is not a
    // suffix on some model id.
    assert!(journal.saw("launch profile=codemuxdev"));
    // Authentication used a method the agent offered. The fixture errors
    // on any other id, so reaching a configured session is the proof.
    assert!(journal.saw("authenticate"));
    assert!(
        collected
            .iter()
            .any(|event| matches!(event, ProviderRuntimeEvent::ContextUsageUpdated { .. })),
        "the unprompted usage_update is the only source of the context meter"
    );
    assert!(
        collected.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::ThreadTitleSuggested { title, .. }
                if title.starts_with("Create /tmp/spike_demo.txt")
        )),
        "the agent auto-titles its sessions and session/list is where that title lives"
    );
    assert!(
        warnings(&collected).is_empty(),
        "a clean start must not warn: {:?}",
        warnings(&collected)
    );
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// `authMethods` is derived from the launched profile's configured
/// runtime, so its ids differ between profiles on the same machine. The
/// fixture rejects any method id it did not offer, which makes a
/// successful start under a renamed runtime the proof that the id was
/// READ rather than assumed. When every method on offer is an interactive
/// terminal one, there is nothing a chat pane can complete, and the error
/// names the command the user has to run instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_authenticates_with_a_method_the_agent_actually_offered() {
    let provider = fixture_provider();
    let journal = Journal::new("auth");
    let thread_id = "hermes-auth";

    let mut input = fixture_start_input(thread_id, Some("codemuxdev"), &journal);
    input
        .env
        .as_mut()
        .expect("fixture env")
        .insert("FAKE_HERMES_RUNTIME".into(), "anthropic".into());
    provider
        .start_session(input)
        .await
        .expect("a start must authenticate with whatever runtime the profile reports");
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");

    let interactive_journal = Journal::new("auth-interactive");
    let mut interactive = fixture_start_input(thread_id, Some("codemuxdev"), &interactive_journal);
    interactive
        .env
        .as_mut()
        .expect("fixture env")
        .insert("FAKE_HERMES_AUTH".into(), "terminal-only".into());
    let error = provider
        .start_session(interactive)
        .await
        .expect_err("a terminal-only method cannot be completed from a chat pane");
    let ProviderError::NotAuthenticated { hint, .. } = error else {
        panic!("expected a NotAuthenticated error, got {error:?}");
    };
    assert!(
        hint.contains("hermes -p codemuxdev --setup"),
        "the hint has to name the command that fixes it: {hint}"
    );
    assert!(
        !interactive_journal.saw("authenticate"),
        "an interactive method must never be sent to `authenticate`: it would \
         block the child on a program with no tty"
    );
}

/// The ordinary turn: prompt in, streamed message out, sealed by the
/// response — whose per-turn `usage` object is the meter's fallback when
/// a turn emitted no `usage_update` of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_turn_completes_with_its_message_and_usage() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("prompt");
    let thread_id = "hermes-prompt";

    provider
        .start_session(fixture_start_input(thread_id, Some("codemuxdev"), &journal))
        .await
        .expect("start fixture session");
    provider
        .send_turn(fixture_turn(thread_id, "say something"))
        .await
        .expect("send turn");
    let collected = events_until_turn_completed(&mut events).await;

    assert_eq!(assistant_texts(&collected), vec!["DONE".to_string()]);
    assert!(matches!(
        collected.last(),
        Some(ProviderRuntimeEvent::TurnCompleted {
            status: TurnStatus::Success,
            ..
        })
    ));
    assert!(!provider.turn_active(&ThreadId(thread_id.into())).await);
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// The permission round trip, end to end: the agent asks, the adapter
/// opens the request with the tool call NORMALISED (so the approval card
/// shows the same diff the transcript would), the user allows, and the
/// answer the child receives is the option id whose `kind` matched —
/// never a position in the list. The fixture holds the turn open until it
/// gets that answer, so the turn completing at all is the round trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_permission_round_trip_answers_with_the_matching_option_kind() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("permission");
    let thread_id = "hermes-permission";

    provider
        .start_session(fixture_start_input(thread_id, Some("codemuxdev"), &journal))
        .await
        .expect("start fixture session");
    provider
        .send_turn(fixture_turn(thread_id, "edit a file, needs permission"))
        .await
        .expect("send turn");

    let (request_id, payload, tool_use_id) = timeout(Duration::from_secs(20), async {
        while let Some(event) = events.next().await {
            if let ProviderRuntimeEvent::RequestOpened {
                request_id,
                payload,
                tool_use_id,
                request_kind,
                ..
            } = event
            {
                assert_eq!(request_kind, "tool_approval");
                return (request_id, payload, tool_use_id);
            }
        }
        panic!("event stream closed before the approval opened");
    })
    .await
    .expect("the approval never opened");

    assert_eq!(tool_use_id.as_deref(), Some("edit-approval-1"));
    // The stable tool name survives verbatim for anything that wants it…
    assert_eq!(payload["toolCall"]["rawInput"]["tool"], "write_file");
    // …while the card renders the normalised call: a diff with no
    // `oldText` is a creation, so it is a Write, not an Edit.
    assert_eq!(payload["toolName"], "Write");
    assert_eq!(
        payload["toolInput"]["file_path"],
        "/tmp/spikework/perm_demo.txt"
    );
    assert_eq!(payload["toolInput"]["content"], "banana");
    // Both options reach the UI with their own labels; nothing is keyed
    // on how many arrived.
    let options = payload["options"].as_array().expect("options");
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["name"], "Allow edit");
    assert_eq!(options[1]["name"], "Deny");

    provider
        .respond_to_request(
            ThreadId(thread_id.into()),
            request_id.clone(),
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .expect("answer the approval");

    let collected = events_until_turn_completed(&mut events).await;
    assert!(
        collected.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::RequestResolved { request_id: resolved, decision: ApprovalDecision::Allow { .. }, .. }
                if *resolved == request_id
        )),
        "the approval must resolve, not just disappear"
    );
    assert!(
        collected.iter().any(
            |event| matches!(event, ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::ToolResult { tool_use_id, is_error: false, .. },
                ..
            } if tool_use_id == "edit-approval-1")
        ),
        "an allowed edit reports its result"
    );
    // The option id the child received, not the index of the button.
    assert!(journal.saw("permission-outcome allow_once"));
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// Stop has to reach the child. The fixture holds this prompt open until
/// a `session/cancel` arrives, so the turn can only complete if the
/// adapter delivered the interrupt — and delivered it after the prompt,
/// since the fixture had to receive the prompt to hold it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_stop_reaches_the_child_and_ends_the_turn() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("cancel");
    let thread_id = "hermes-stop";

    provider
        .start_session(fixture_start_input(thread_id, Some("codemuxdev"), &journal))
        .await
        .expect("start fixture session");
    let started = provider
        .send_turn(fixture_turn(thread_id, "await-cancel please"))
        .await
        .expect("send turn");
    journal.wait_for("session/prompt").await;
    provider
        .interrupt_turn(ThreadId(thread_id.into()), Some(started.turn_id))
        .await
        .expect("interrupt");

    let status = timeout(Duration::from_secs(20), async {
        while let Some(event) = events.next().await {
            if let ProviderRuntimeEvent::TurnCompleted { status, .. } = event {
                return status;
            }
        }
        panic!("event stream closed before turn completion");
    })
    .await
    .expect("the interrupted turn never completed — Stop was dropped");

    assert!(
        matches!(status, TurnStatus::Error { ref subtype, .. } if subtype == "interrupted"),
        "expected an interrupted turn, got {status:?}"
    );
    assert!(journal.saw("session/cancel"));
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// Diff translation, on both captured shapes. The presence of `oldText`
/// is the ONLY thing that distinguishes a replace from a creation — both
/// carry `path` and `newText` — and the transcript's renderers key off
/// Codemux's own tool vocabulary, so a Hermes-native name would fall
/// through to a generic JSON card and lose the diff entirely.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_diff_blocks_become_write_edit_and_bash() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("diffs");
    let thread_id = "hermes-diffs";

    provider
        .start_session(fixture_start_input(thread_id, Some("codemuxdev"), &journal))
        .await
        .expect("start fixture session");
    provider
        .send_turn(fixture_turn(thread_id, "show me diffs"))
        .await
        .expect("send turn");
    let collected = events_until_turn_completed(&mut events).await;

    let calls = tool_uses(&collected);
    let names: Vec<&str> = calls.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["Write", "Edit", "Bash"]);

    let (_, write) = &calls[0];
    assert_eq!(write["file_path"], "/tmp/spike_demo.txt");
    assert_eq!(write["content"], "hello");

    let (_, edit) = &calls[1];
    assert_eq!(edit["file_path"], "/tmp/spike_demo.txt");
    assert_eq!(edit["old_string"], "hello");
    assert_eq!(edit["new_string"], "hello world");

    let (_, bash) = &calls[2];
    assert_eq!(bash["command"], "echo spike-ok");

    // A shell result is plain text, not a diff, and reaches the tool card
    // as rendered text rather than as a JSON block array.
    assert!(
        collected.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::ToolResult { content, .. },
                ..
            } if content.as_str().is_some_and(|text| text.contains("exit_code"))
        )),
        "the shell result must arrive as text"
    );
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// The resume envelope is persisted verbatim and replayed unchanged, so
/// every key in it has to survive a restart — the profile above all,
/// because it is part of the session's identity rather than a preference:
/// the same session id under a different profile addresses a different
/// runtime's store. The second start must LOAD the session, not create a
/// new one and quietly lose the conversation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_resume_envelope_round_trips_including_the_profile() {
    let provider = fixture_provider();
    let first_journal = Journal::new("resume-first");
    let second_journal = Journal::new("resume-second");
    let thread_id = "hermes-resume";

    let first = provider
        .start_session(fixture_start_input(
            thread_id,
            Some("codemuxdev"),
            &first_journal,
        ))
        .await
        .expect("start fixture session");
    let envelope = first
        .resume_cursor
        .clone()
        .expect("a started session resumes");
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");

    let mut resumed_input = fixture_start_input(thread_id, Some("codemuxdev"), &second_journal);
    resumed_input.resume_cursor = Some(envelope.clone());
    let second = provider
        .start_session(resumed_input)
        .await
        .expect("resume fixture session");

    assert_eq!(second.session_id, first.session_id);
    // Round trip: what comes back out is what went in, profile included.
    assert_eq!(second.resume_cursor, Some(envelope));
    assert_eq!(
        second.resume_cursor.as_ref().unwrap()["profile"],
        "codemuxdev"
    );
    assert!(second_journal.saw("session/load"));
    assert!(
        !second_journal.saw("session/new"),
        "a resume that silently created a fresh session is not a resume"
    );
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop resumed session");
}

/// The other half of the profile rule: an envelope created under one
/// profile must not be loaded under another. `session/load` answers an
/// unknown id with `{}` and a silently fresh session rather than an
/// error, so a cross-profile resume would look like it worked and have no
/// history. The adapter drops the cursor, says so, and starts new.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_a_cross_profile_envelope_is_reported_and_not_resumed() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("cross-profile");
    let thread_id = "hermes-cross-profile";

    let mut input = fixture_start_input(thread_id, Some("other"), &journal);
    input.resume_cursor = Some(serde_json::json!({
        "schemaVersion": 1,
        "sessionId": "hermes-codemuxdev-session",
        "profile": "codemuxdev",
    }));
    let session = provider
        .start_session(input)
        .await
        .expect("start fixture session");
    let collected = events_until_configured(&mut events).await;

    assert!(
        warnings(&collected)
            .iter()
            .any(|message| message.contains("different Hermes profile")),
        "a dropped resume has to be visible: {:?}",
        warnings(&collected)
    );
    assert!(journal.saw("session/new"));
    assert!(!journal.saw("session/load"));
    assert_eq!(session.session_id.0, "hermes-other-session");
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// Closing a pane is not ending the conversation. A Hermes session is
/// durable — `session/list` still names it and `session/load` picks the
/// transcript back up — so detach kills the local child WITHOUT sending
/// `session/cancel`, which would have cancelled a turn agent-side and
/// genuinely lost the work. Proven twice over: the child never sees a
/// cancel, and the same envelope still loads its transcript afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_detaching_a_pane_leaves_the_agent_session_alive() {
    let provider = fixture_provider();
    let detached_journal = Journal::new("detach");
    let reattached_journal = Journal::new("reattach");
    let thread_id = "hermes-detach";

    let session = provider
        .start_session(fixture_start_input(
            thread_id,
            Some("codemuxdev"),
            &detached_journal,
        ))
        .await
        .expect("start fixture session");
    let envelope = session
        .resume_cursor
        .clone()
        .expect("a started session resumes");

    provider
        .detach_session(ThreadId(thread_id.into()))
        .await
        .expect("detach");
    assert!(!provider.has_session(&ThreadId(thread_id.into())).await);
    assert!(
        !detached_journal.saw("session/cancel"),
        "detach must not cancel the agent's turn: {:?}",
        detached_journal.lines()
    );
    assert!(
        !detached_journal.saw("session/close"),
        "session/close is a no-op upstream and is deliberately never sent"
    );

    let mut events = provider.event_stream();
    let mut reattach = fixture_start_input(thread_id, Some("codemuxdev"), &reattached_journal);
    reattach.resume_cursor = Some(envelope);
    reattach.extra = serde_json::json!({ "adoptTranscript": true });
    let resumed = provider
        .start_session(reattach)
        .await
        .expect("reattach to the detached session");
    let collected = events_until_configured(&mut events).await;

    assert_eq!(resumed.session_id, session.session_id);
    assert!(
        !user_texts(&collected).is_empty(),
        "the detached conversation is still there to load"
    );
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop reattached session");
}

/// `session/load` replays a session's whole transcript as `session/update`
/// notifications, ahead of its own response and therefore with no turn in
/// flight. Dropping turn-less updates — which is what an ACP adapter
/// written for turn-scoped streaming does — discards the entire replay,
/// and for a session created outside Codemux that replay is the ONLY copy
/// of the conversation. These are the events the chat reducer consumes,
/// so what reaches the stream is what reaches the transcript.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_session_load_replay_reaches_the_event_stream() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("replay");
    let thread_id = "hermes-replay";

    let mut input = fixture_start_input(thread_id, Some("codemuxdev"), &journal);
    input.resume_cursor = Some(serde_json::json!({
        "schemaVersion": 1,
        "sessionId": "hermes-codemuxdev-session",
        "profile": "codemuxdev",
    }));
    input.extra = serde_json::json!({ "adoptTranscript": true });
    provider
        .start_session(input)
        .await
        .expect("start resumed fixture session");
    let collected = events_until_configured(&mut events).await;

    assert_eq!(
        user_texts(&collected),
        vec!["first question".to_string(), "second question".to_string()],
    );
    assert_eq!(
        assistant_texts(&collected),
        vec!["DONE".to_string(), "second answer".to_string()],
    );
    assert!(
        collected.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::AssistantThinking { text },
                ..
            } if text.contains("Planning sequential")
        )),
        "replayed reasoning must survive"
    );
    assert!(
        collected.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::ToolUse { tool_use_id, .. },
                ..
            } if tool_use_id == "call_yf51QttEMinSqbkNyRqCnK98"
        )),
        "replayed tool calls must survive"
    );
    // Each replayed user message opens its own sealed turn, or the pane
    // hydrates into a turn that never closes.
    let replay_turns: Vec<&str> = collected
        .iter()
        .filter_map(|event| match event {
            ProviderRuntimeEvent::TurnCompleted { turn_id, .. } => Some(turn_id.0.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replay_turns.len(), 2);
    assert!(replay_turns
        .iter()
        .all(|turn_id| turn_id.starts_with("hermes-replay-")));
    assert_ne!(replay_turns[0], replay_turns[1]);

    // A replay is not a turn: it must not have claimed the session's
    // single prompt slot, or the user's first message queues behind a
    // phantom.
    assert!(!provider.turn_active(&ThreadId(thread_id.into())).await);
    provider
        .send_turn(fixture_turn(thread_id, "after the replay"))
        .await
        .expect("send turn after replay");
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// The profile is a LAUNCH parameter: `-p <profile>` selects the runtime,
/// the model catalogue and the approval policy the child boots with, and
/// no protocol call re-profiles a running process. So changing it has to
/// restart the session. The journals are the evidence — the first child
/// is never poked with a `set_model`/`set_mode`, and a second child is
/// launched under the new profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_changing_the_profile_restarts_the_session() {
    let provider = fixture_provider();
    let first_journal = Journal::new("profile-a");
    let second_journal = Journal::new("profile-b");
    let thread_id = "hermes-profile-change";

    let first = provider
        .start_session(fixture_start_input(
            thread_id,
            Some("profile-a"),
            &first_journal,
        ))
        .await
        .expect("start under the first profile");
    assert_eq!(first.session_id.0, "hermes-profile-a-session");

    // No stop in between: the provider evicts the live session itself,
    // which is what "changing the profile restarts it" means in practice.
    let second = provider
        .start_session(fixture_start_input(
            thread_id,
            Some("profile-b"),
            &second_journal,
        ))
        .await
        .expect("restart under the second profile");

    assert_eq!(second.session_id.0, "hermes-profile-b-session");
    assert_eq!(
        second.resume_cursor.as_ref().unwrap()["profile"],
        "profile-b"
    );
    assert!(second_journal.saw("launch profile=profile-b"));
    assert!(second_journal.saw("session/new"));
    assert!(
        !first_journal.saw("session/set_model") && !first_journal.saw("session/set_mode"),
        "a profile change must never become a live poke: {:?}",
        first_journal.lines()
    );
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// An agent is free to add update kinds and notification methods Codemux
/// has never heard of, and a build that met one by failing the session
/// would break on an upgrade it had no part in. Both degrade to a
/// `RuntimeWarning`, and the turn they arrived in still finishes with its
/// message intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_unknown_events_warn_without_killing_the_session() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("unknown");
    let thread_id = "hermes-unknown";

    provider
        .start_session(fixture_start_input(thread_id, Some("codemuxdev"), &journal))
        .await
        .expect("start fixture session");
    provider
        .send_turn(fixture_turn(thread_id, "unknown-event please"))
        .await
        .expect("send turn");
    let collected = events_until_turn_completed(&mut events).await;

    let warned = warnings(&collected);
    assert!(
        warned
            .iter()
            .any(|message| message.contains("quantum_flux_update")),
        "an unrecognised session update has to be reported: {warned:?}"
    );
    assert!(
        warned
            .iter()
            .any(|message| message.contains("hermes/telemetry")),
        "an unrecognised notification has to be reported: {warned:?}"
    );
    assert_eq!(assistant_texts(&collected), vec!["DONE".to_string()]);
    assert!(matches!(
        collected.last(),
        Some(ProviderRuntimeEvent::TurnCompleted {
            status: TurnStatus::Success,
            ..
        })
    ));

    // The session is still usable afterwards — a warning is not a wound.
    assert!(provider.has_session(&ThreadId(thread_id.into())).await);
    provider
        .send_turn(fixture_turn(thread_id, "still here?"))
        .await
        .expect("send another turn");
    let after = events_until_turn_completed(&mut events).await;
    assert_eq!(assistant_texts(&after), vec!["DONE".to_string()]);
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// An approval left outstanding when a turn ends must be answered, or the
/// child blocks forever and the UI's approval card can never be
/// dismissed. Stop mid-approval is the way to get there: the turn ends
/// while the request is still open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_an_open_approval_is_cancelled_when_its_turn_ends() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let journal = Journal::new("approval-cancel");
    let thread_id = "hermes-approval-cancel";

    provider
        .start_session(fixture_start_input(thread_id, Some("codemuxdev"), &journal))
        .await
        .expect("start fixture session");
    let started = provider
        .send_turn(fixture_turn(thread_id, "edit a file, needs permission"))
        .await
        .expect("send turn");

    let opened = timeout(Duration::from_secs(20), async {
        while let Some(event) = events.next().await {
            if let ProviderRuntimeEvent::RequestOpened { request_id, .. } = event {
                return request_id;
            }
        }
        panic!("event stream closed before the approval opened");
    })
    .await
    .expect("the approval never opened");

    provider
        .interrupt_turn(ThreadId(thread_id.into()), Some(started.turn_id))
        .await
        .expect("interrupt");
    let collected = events_until_turn_completed(&mut events).await;

    assert!(
        collected.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::RequestResolved {
                request_id, decision: ApprovalDecision::Cancel, ..
            } if *request_id == opened
        )),
        "an abandoned approval must be resolved as cancelled"
    );
    let err = provider
        .respond_to_request(
            ThreadId(thread_id.into()),
            opened,
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .expect_err("a cancelled request is no longer pending");
    assert!(matches!(err, ProviderError::RequestNotPending { .. }));
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// Run manually with:
/// `cargo test -j 2 --manifest-path src-tauri/Cargo.toml --test hermes_adapter \
///   hermes_real_session_resumes_under_the_throwaway_profile -- --ignored --nocapture`
///
/// Uses ONLY the throwaway `codemuxdev` profile. Never point it at
/// `default`, `coder` or `omar` — it starts sessions and sends prompts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a real hermes CLI + an authenticated codemuxdev profile; run manually"]
async fn hermes_real_session_resumes_under_the_throwaway_profile() {
    let provider = HermesAgentProvider::new(HermesProviderConfig::default());
    let mut events = provider.event_stream();
    let thread_id = format!(
        "hermes-live-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    );
    let live_input = |resume_cursor: Option<serde_json::Value>| StartSessionInput {
        thread_id: ThreadId(thread_id.clone()),
        cwd: std::env::current_dir().expect("test cwd"),
        model: None,
        profile: Some("codemuxdev".into()),
        resume_cursor,
        permission_mode: Some("default".into()),
        effort: None,
        context_window: None,
        fast_mode: false,
        additional_directories: vec![],
        env: None,
        workspace_id: None,
        extra: serde_json::Value::Null,
        recorded_usage_baseline: None,
    };

    let started = provider
        .start_session(live_input(None))
        .await
        .expect("start a live Hermes session");
    let envelope = started
        .resume_cursor
        .clone()
        .expect("a live session resumes");
    provider
        .send_turn(fixture_turn(&thread_id, "Reply with OK only."))
        .await
        .expect("send the live turn");
    // `session/new` measured 4.3 s warm and a turn 15.8 s; a cold model
    // catalogue is far worse.
    timeout(Duration::from_secs(300), async {
        while let Some(event) = events.next().await {
            if matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }) {
                return;
            }
        }
        panic!("the live event stream closed before turn completion");
    })
    .await
    .expect("the live turn timed out");
    provider
        .stop_session(ThreadId(thread_id.clone()))
        .await
        .expect("stop the live session");

    let resumed = provider
        .start_session(live_input(Some(envelope.clone())))
        .await
        .expect("resume the live session");
    assert_eq!(resumed.session_id, started.session_id);
    assert_eq!(resumed.resume_cursor, Some(envelope));
    provider
        .stop_session(ThreadId(thread_id))
        .await
        .expect("stop the resumed live session");
}

/// The paths the fixture cannot vouch for, driven against the REAL agent.
///
/// The fixture was already caught being more permissive than Hermes once — it
/// accepted an `initialize` the real server rejects, which hid a total start
/// failure behind a green suite. Permissions, tool calls and diff translation
/// are the remaining paths where the client sends a frame the agent
/// validates, so they are pinned here against the binary rather than against
/// our own mock. `session/load` replay is included because the real agent
/// floods the whole transcript inline before the load response resolves,
/// which no fixture reproduces faithfully.
///
/// Run manually with:
/// `cargo test -j 2 --manifest-path src-tauri/Cargo.toml --test hermes_adapter \
///   hermes_real_permission_and_replay -- --ignored --nocapture`
///
/// Uses ONLY the throwaway `codemuxdev` profile, and only ever writes inside a
/// `TempDir`. Never point it at `default`, `coder` or `omar`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a real hermes CLI + an authenticated codemuxdev profile; run manually"]
async fn hermes_real_permission_and_replay() {
    let workdir = TempDir::new().expect("temp workdir");
    let target = workdir.path().join("live_demo.txt");
    let provider = HermesAgentProvider::new(HermesProviderConfig::default());
    let mut events = provider.event_stream();
    let thread_id = format!(
        "hermes-live-perm-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    );
    let live_input = |resume_cursor: Option<serde_json::Value>, adopt: bool| StartSessionInput {
        thread_id: ThreadId(thread_id.clone()),
        cwd: workdir.path().to_path_buf(),
        model: None,
        profile: Some("codemuxdev".into()),
        resume_cursor,
        // `default` is "ask before edits" — the mode that makes the agent
        // raise a real approval instead of auto-allowing one.
        permission_mode: Some("default".into()),
        effort: None,
        context_window: None,
        fast_mode: false,
        additional_directories: vec![],
        env: None,
        workspace_id: None,
        extra: serde_json::json!({ "adoptTranscript": adopt }),
        recorded_usage_baseline: None,
    };

    let started = provider
        .start_session(live_input(None, false))
        .await
        .expect("start a live Hermes session");
    let envelope = started
        .resume_cursor
        .clone()
        .expect("a live session resumes");

    provider
        .send_turn(fixture_turn(
            &thread_id,
            &format!(
                "Create a file at {} containing exactly the word banana. \
                 Then reply DONE. Do not run any shell commands.",
                target.display()
            ),
        ))
        .await
        .expect("send the live turn");

    // 1. A real approval must open, carry the stable tool name, and be
    //    translated into a Write (a diff with no `oldText` is a creation).
    let (request_id, payload) = timeout(Duration::from_secs(300), async {
        while let Some(event) = events.next().await {
            if let ProviderRuntimeEvent::RequestOpened {
                request_id,
                payload,
                request_kind,
                ..
            } = event
            {
                assert_eq!(request_kind, "tool_approval");
                return (request_id, payload);
            }
        }
        panic!("the live event stream closed before an approval opened");
    })
    .await
    .expect("the live approval never opened");

    assert_eq!(
        payload["toolCall"]["rawInput"]["tool"], "write_file",
        "the stable tool name must survive verbatim from the real agent"
    );
    assert_eq!(
        payload["toolName"], "Write",
        "a real diff block with no oldText must translate to a Write"
    );
    assert_eq!(
        payload["toolInput"]["file_path"],
        target.to_string_lossy().as_ref()
    );
    let options = payload["options"].as_array().expect("options");
    assert!(
        options.iter().all(|option| option["name"].is_string()),
        "every real option must reach the UI with its own label"
    );

    provider
        .respond_to_request(
            ThreadId(thread_id.clone()),
            request_id,
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .expect("answer the live approval");

    timeout(Duration::from_secs(300), async {
        while let Some(event) = events.next().await {
            if matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }) {
                return;
            }
        }
        panic!("the live event stream closed before turn completion");
    })
    .await
    .expect("the live turn timed out");

    // 2. Allowing the approval must have actually let the write through — the
    //    round trip is only real if the edit reached the filesystem.
    let written = std::fs::read_to_string(&target)
        .expect("the approved edit must have reached the filesystem");
    assert!(
        written.to_lowercase().contains("banana"),
        "unexpected file contents: {written:?}"
    );

    provider
        .stop_session(ThreadId(thread_id.clone()))
        .await
        .expect("stop the live session");

    // 3. `session/load` replay, both halves, against the real agent. Hermes
    //    emits the whole transcript as `session/update` frames with no turn in
    //    flight — the burst the client used to discard wholesale before the
    //    Phase 1 replay-window fix.
    //
    //    Suppressed first: a thread that already owns persisted rows must NOT
    //    re-emit them, or every bubble doubles on each restart. Getting this
    //    backwards is silent data corruption, so it is worth pinning that the
    //    replay is consumed and dropped rather than never arriving at all.
    let mut suppressed_events = provider.event_stream();
    provider
        .start_session(live_input(Some(envelope.clone()), false))
        .await
        .expect("resume the live session without adopting");
    let suppressed = drain_until_configured(&mut suppressed_events).await;
    assert!(
        assistant_texts(&suppressed).is_empty(),
        "a non-adopting resume must not re-emit the transcript; got {:?}",
        assistant_texts(&suppressed)
    );
    provider
        .stop_session(ThreadId(thread_id.clone()))
        .await
        .expect("stop the suppressed resume");

    //    Adopting second: this is the Hermes case the feature exists for — a
    //    session whose transcript lives only in Hermes, where replay is the
    //    ONLY source. It must actually reach the event stream.
    let mut adopted_events = provider.event_stream();
    provider
        .start_session(live_input(Some(envelope), true))
        .await
        .expect("resume the live session adopting the transcript");
    let adopted = drain_until_configured(&mut adopted_events).await;
    let adopted_text = assistant_texts(&adopted).join("\n");
    assert!(
        !adopted_text.is_empty(),
        "an adopting session/load must replay the real transcript, not silently \
         drop it; got {} events",
        adopted.len()
    );
    assert!(
        adopted_text.to_lowercase().contains("done"),
        "the replayed transcript should carry the agent's own reply; got {adopted_text:?}"
    );

    provider
        .stop_session(ThreadId(thread_id))
        .await
        .expect("stop the resumed live session");
}

/// Collect events until the session settles, so a resume can be inspected as
/// a whole. `SessionConfigured` is published once start-up is complete, which
/// is after any `session/load` replay has been drained.
async fn drain_until_configured(events: &mut ProviderEventStream) -> Vec<ProviderRuntimeEvent> {
    timeout(Duration::from_secs(300), async {
        let mut collected = Vec::new();
        while let Some(event) = events.next().await {
            let configured = matches!(event, ProviderRuntimeEvent::SessionConfigured { .. });
            collected.push(event);
            if configured {
                return collected;
            }
        }
        panic!("the live event stream closed before the resume settled");
    })
    .await
    .expect("the live resume timed out")
}
