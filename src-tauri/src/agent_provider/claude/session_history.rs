//! Read-only import of an adopted terminal session's transcript.
//!
//! When a conversation that started in the Claude CLI is adopted into
//! Codemux, the thread's own `agent_chat_messages` rows begin at the
//! "resumed from the terminal" divider. Everything before that lives in
//! the provider's transcript, which the sidecar's `get-session-messages`
//! JSON-RPC method pages through via the SDK. This module turns one such
//! page into rows shaped EXACTLY like persisted transcript rows —
//! `serde_json::to_string(&ProviderRuntimeEvent)` — so the frontend can
//! hydrate them through the same reducer as a restart replay.
//!
//! Nothing here writes to the database on purpose: imported history must
//! never re-enter the usage ledger, the search index, or the row-id
//! cursor. Rows get synthetic NEGATIVE ids so they sort before every real
//! autoincrement row and can never collide with one.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent_provider::errors::ProviderError;
use crate::agent_provider::events::{ProviderRuntimeEvent, TurnStatus};
use crate::agent_provider::types::{ThreadId, TurnId};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

use super::protocol::METHOD_GET_SESSION_MESSAGES;
use super::translate::{translate_sdk_message_with, SubagentDemux};

/// Spawn + request budget. Same ceiling the other transient sidecar
/// probes use; a page read is a local file scan, so this is generous.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Default page size when the caller does not specify one.
pub const DEFAULT_PAGE_LIMIT: usize = 40;
/// Hard ceiling on one page so a single request cannot pin the UI.
pub const MAX_PAGE_LIMIT: usize = 200;

/// Id slots reserved per source record. One SDK record can fan out into
/// several rows (thinking + text + tool_use, or a synthetic turn marker
/// ahead of a prompt), and ids must stay strictly increasing AND stable
/// across pages, so each record owns a fixed block of the id space.
const ROWS_PER_RECORD: i64 = 1024;

/// Wire shape of the sidecar's `get-session-messages` result. Messages
/// stay opaque JSON: the translator consumes SDK dialect directly and
/// tolerating unknown fields is exactly what we want from history.
#[derive(Debug, Default, Deserialize)]
pub struct SessionMessagesPage {
    #[serde(default)]
    pub messages: Vec<serde_json::Value>,
    #[serde(default)]
    pub total: usize,
    /// Index of `messages[0]` within the whole session. `0` means the
    /// page reaches the beginning of the conversation.
    #[serde(default)]
    pub offset: usize,
}

/// One imported transcript row. Field names mirror the persisted
/// `AgentChatMessageRow` (`{ id, payload, created_at_ms }`) so the
/// frontend hydrates it with the code it already has.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdoptedHistoryRow {
    /// Synthetic, always negative, strictly increasing within a session.
    pub id: i64,
    /// JSON `ProviderRuntimeEvent`, byte-compatible with what
    /// `forward_event` persists for a live session.
    pub payload: String,
    /// Source record's `timestamp`, in ms since the epoch.
    pub created_at_ms: i64,
}

/// A page of imported rows plus the bookkeeping the frontend needs to
/// ask for the page before it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdoptedHistoryPage {
    /// Chronological.
    pub rows: Vec<AdoptedHistoryRow>,
    /// Source records in the whole session (NOT rows — several rows can
    /// come from one record, and noise records yield none).
    pub total: usize,
    /// Source index of the first record this page covers; `0` = beginning
    /// reached.
    pub offset: usize,
}

/// Clamp a caller-supplied page size into the supported range.
pub fn clamp_page_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT)
}

/// Fetch one page of raw SDK records for `session_id` through a
/// transient sidecar. Shuts the child down on every path.
pub async fn fetch_session_messages(
    sidecar_binary: &Path,
    session_id: &str,
    dir: Option<&str>,
    limit: usize,
    before_offset: Option<usize>,
) -> Result<SessionMessagesPage, ProviderError> {
    let mut params = json!({
        "sessionId": session_id,
        "limit": limit,
    });
    if let Some(dir) = dir.filter(|d| !d.trim().is_empty()) {
        params["dir"] = json!(dir);
    }
    if let Some(before) = before_offset {
        params["beforeOffset"] = json!(before);
    }

    let child = tokio::time::timeout(
        FETCH_TIMEOUT,
        JsonRpcChild::spawn(SpawnConfig {
            program: sidecar_binary.to_path_buf(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            default_timeout: FETCH_TIMEOUT,
        }),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        operation: "get-session-messages sidecar spawn".to_string(),
        elapsed_ms: FETCH_TIMEOUT.as_millis() as u64,
    })?
    .map_err(|e| ProviderError::ProcessError {
        message: "failed to spawn claude-agent sidecar".to_string(),
        source: Some(e.to_string()),
    })?;

    let response = child.request(METHOD_GET_SESSION_MESSAGES, params).await;
    let _ = child.shutdown().await;
    let response = response.map_err(|e| ProviderError::RpcError {
        message: format!("get-session-messages RPC failed: {e}"),
    })?;

    serde_json::from_value(response).map_err(|e| ProviderError::RpcError {
        message: format!("get-session-messages decode failed: {e}"),
    })
}

/// Fetch and translate one page of history for an adopted thread.
pub async fn load_adopted_history(
    sidecar_binary: &Path,
    thread_id: &str,
    session_id: &str,
    dir: Option<&str>,
    limit: usize,
    before_offset: Option<usize>,
) -> Result<AdoptedHistoryPage, ProviderError> {
    let page = fetch_session_messages(sidecar_binary, session_id, dir, limit, before_offset).await?;
    Ok(history_page(thread_id, page))
}

/// Pure translation of a fetched page into transcript rows.
pub fn history_page(thread_id: &str, page: SessionMessagesPage) -> AdoptedHistoryPage {
    let rows = history_rows(thread_id, &page.messages, page.offset, page.total);
    AdoptedHistoryPage {
        rows,
        total: page.total,
        offset: page.offset,
    }
}

/// Translate chronological SDK records into persisted-shape rows.
///
/// Turn ids: on-disk records carry none, so every prompt (a `user` record
/// whose content is plain text rather than tool results) opens a new turn
/// keyed by that record's `uuid`, and every following event is stamped
/// with it. A synthetic successful `turn_completed` closes each turn at
/// the next prompt and at the end of the page, mirroring what a live run
/// persists so the reducer settles the turn the same way.
///
/// Records before the first prompt (a page that starts mid-turn) share a
/// deterministic fallback turn id derived from the page offset.
pub fn history_rows(
    thread_id: &str,
    messages: &[serde_json::Value],
    offset: usize,
    total: usize,
) -> Vec<AdoptedHistoryRow> {
    let thread = ThreadId(thread_id.to_string());
    let mut demux = SubagentDemux::default();
    let mut rows: Vec<AdoptedHistoryRow> = Vec::new();
    let mut current_turn: Option<TurnId> = None;
    // Where the next row goes: the id block of the record being
    // processed, the next free slot in it, and that record's stamp. Kept
    // outside the loop so the end-of-page marker lands after the last
    // record's own rows.
    let mut cursor = RowCursor::default();

    for (i, record) in messages.iter().enumerate() {
        let message_index = offset + i;
        cursor.begin_record(message_index, total, record);

        if let Some(prompt) = prompt_text(record) {
            if is_prompt_noise(&prompt) {
                continue;
            }
            let text = strip_system_reminders(&prompt);
            if text.is_empty() {
                continue;
            }
            if let Some(turn) = current_turn.take() {
                cursor.push(&mut rows, &turn_completed(&thread, turn));
            }
            let turn = TurnId(turn_id_for_record(record, message_index));
            current_turn = Some(turn);
            cursor.push(
                &mut rows,
                &ProviderRuntimeEvent::UserMessage {
                    thread_id: thread.clone(),
                    text,
                    images: Vec::new(),
                    client_nonce: None,
                },
            );
            continue;
        }

        let turn = current_turn
            .clone()
            .unwrap_or_else(|| TurnId(format!("adopted-turn-{offset}")));
        for mut event in translate_sdk_message_with(&thread, record, &mut demux) {
            if !is_transcript_event(&event) {
                continue;
            }
            if let ProviderRuntimeEvent::ItemCompleted { turn_id, .. } = &mut event {
                *turn_id = turn.clone();
            }
            cursor.push(&mut rows, &event);
        }
    }

    if let Some(turn) = current_turn {
        cursor.push(&mut rows, &turn_completed(&thread, turn));
    }

    rows
}

/// Row-id and timestamp bookkeeping for [`history_rows`].
#[derive(Default)]
struct RowCursor {
    base_id: i64,
    slot: i64,
    stamp_ms: i64,
}

impl RowCursor {
    fn begin_record(&mut self, message_index: usize, total: usize, record: &serde_json::Value) {
        self.base_id = (message_index as i64 - total as i64) * ROWS_PER_RECORD;
        self.slot = 0;
        // An unparseable stamp inherits the previous record's so rows
        // never jump to the epoch.
        if let Some(ms) = record
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_timestamp_ms)
        {
            self.stamp_ms = ms;
        }
    }

    fn push(&mut self, rows: &mut Vec<AdoptedHistoryRow>, event: &ProviderRuntimeEvent) {
        let Ok(payload) = serde_json::to_string(event) else {
            return;
        };
        rows.push(AdoptedHistoryRow {
            id: self.base_id + self.slot.min(ROWS_PER_RECORD - 1),
            payload,
            created_at_ms: self.stamp_ms,
        });
        self.slot += 1;
    }
}

fn turn_completed(thread: &ThreadId, turn: TurnId) -> ProviderRuntimeEvent {
    ProviderRuntimeEvent::TurnCompleted {
        thread_id: thread.clone(),
        turn_id: turn,
        status: TurnStatus::Success,
        // Never carry usage: the ledger is fed from provider history and
        // imported rows must not count anything a second time.
        usage: None,
    }
}

/// Events that are transcript content. Everything else the translator
/// emits for a record — usage ledger entries, context-meter snapshots,
/// lifecycle notices, warnings about unknown types — is dropped.
fn is_transcript_event(event: &ProviderRuntimeEvent) -> bool {
    matches!(
        event,
        ProviderRuntimeEvent::ItemCompleted { .. }
            | ProviderRuntimeEvent::SubagentUpdated { .. }
            | ProviderRuntimeEvent::WorkflowUpdated { .. }
            | ProviderRuntimeEvent::TasksUpdated { .. }
    )
}

/// The prompt text of a top-level `user` record, or `None` when the
/// record is not a prompt: assistant/system records, tool-result
/// envelopes, and subagent-scoped user records (which belong to a child
/// transcript, not the parent's bubble list).
fn prompt_text(record: &serde_json::Value) -> Option<String> {
    if record.get("type").and_then(|v| v.as_str()) != Some("user") {
        return None;
    }
    if record
        .get("parent_tool_use_id")
        .map(|v| !v.is_null())
        .unwrap_or(false)
    {
        return None;
    }
    let content = record.get("message")?.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let blocks = content.as_array()?;
    let mut text = String::new();
    for block in blocks {
        match block.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
            // Any tool_result makes this a result envelope, which the
            // translator owns (it pairs results with their tool_use).
            Some("tool_result") => return None,
            _ => {}
        }
    }
    Some(text)
}

/// Claude Code writes local-command echoes and injected reminders into
/// the transcript as `user` records; none of them were typed by the user.
fn is_prompt_noise(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.is_empty()
        || trimmed.starts_with("<command-name>")
        || trimmed.starts_with("<local-command-stdout>")
        || trimmed.starts_with("<local-command-caveat>")
}

/// Remove every `<system-reminder>…</system-reminder>` span. The CLI
/// appends these to what the user typed; the live path never shows them
/// because Codemux persists the composer text, not the wire text.
fn strip_system_reminders(text: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        match rest[start..].find(CLOSE) {
            Some(end_rel) => rest = &rest[start + end_rel + CLOSE.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

fn turn_id_for_record(record: &serde_json::Value, message_index: usize) -> String {
    record
        .get("uuid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("adopted-turn-{message_index}"))
}

fn parse_timestamp_ms(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "thread-adopted";
    const SESSION: &str = "0f9a1c2d-3e4f-5a6b-7c8d-9e0f1a2b3c4d";

    fn record(
        ty: &str,
        uuid: &str,
        ts: &str,
        content: serde_json::Value,
        extra: serde_json::Value,
    ) -> serde_json::Value {
        let mut message = json!({
            "role": if ty == "assistant" { "assistant" } else { "user" },
            "content": content,
        });
        if ty == "assistant" {
            message["model"] = json!("claude-sonnet-4-5");
            // The exact block a live stream would carry; the ledger
            // must NOT see it come back out.
            message["usage"] = json!({
                "input_tokens": 1200,
                "output_tokens": 80,
                "cache_read_input_tokens": 400,
                "cache_creation_input_tokens": 0
            });
        }
        let mut value = json!({
            "type": ty,
            "uuid": uuid,
            "session_id": SESSION,
            "message": message,
            "parent_tool_use_id": null,
            "timestamp": ts,
        });
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                value[k] = v.clone();
            }
        }
        value
    }

    /// Two prompts, a tool round-trip, a thinking block, a local-command
    /// echo, and a closing assistant text — the shapes SDK 0.2.114
    /// returns from `getSessionMessages`.
    fn fixture() -> Vec<serde_json::Value> {
        vec![
            record(
                "user",
                "u-1",
                "2026-08-28T10:00:00.000Z",
                json!("Fix the failing test"),
                json!({}),
            ),
            record(
                "assistant",
                "a-1",
                "2026-08-28T10:00:01.500Z",
                json!([{ "type": "thinking", "thinking": "Look at the test first.", "signature": "sig" }]),
                json!({}),
            ),
            record(
                "assistant",
                "a-2",
                "2026-08-28T10:00:02.000Z",
                json!([{ "type": "tool_use", "id": "toolu_1", "name": "Read", "input": { "file_path": "/repo/a.rs" } }]),
                json!({}),
            ),
            record(
                "user",
                "u-2",
                "2026-08-28T10:00:03.000Z",
                json!([{ "type": "tool_result", "tool_use_id": "toolu_1", "is_error": false, "content": "fn main() {}" }]),
                json!({}),
            ),
            record(
                "assistant",
                "a-3",
                "2026-08-28T10:00:04.000Z",
                json!([{ "type": "text", "text": "Fixed the assertion." }]),
                json!({}),
            ),
            record(
                "user",
                "u-3",
                "2026-08-28T10:00:05.000Z",
                json!("<command-name>/clear</command-name>\n<command-message>clear</command-message>"),
                json!({}),
            ),
            record(
                "user",
                "u-4",
                "not-a-timestamp",
                json!([{ "type": "text", "text": "Now add docs" }, { "type": "text", "text": "<system-reminder>ignore me</system-reminder>" }]),
                json!({}),
            ),
            record(
                "assistant",
                "a-4",
                "2026-08-28T10:00:07.000Z",
                json!([{ "type": "text", "text": "Added a doc comment." }]),
                json!({}),
            ),
        ]
    }

    fn parsed(rows: &[AdoptedHistoryRow]) -> Vec<serde_json::Value> {
        rows.iter()
            .map(|r| serde_json::from_str(&r.payload).expect("row payload is JSON"))
            .collect()
    }

    fn types(events: &[serde_json::Value]) -> Vec<String> {
        events
            .iter()
            .map(|e| e["type"].as_str().unwrap_or("").to_string())
            .collect()
    }

    #[test]
    fn adopted_history_translates_a_page_into_persisted_shape_rows() {
        let page = SessionMessagesPage {
            messages: fixture(),
            total: 8,
            offset: 0,
        };
        let out = history_page(THREAD, page);
        assert_eq!(out.total, 8);
        assert_eq!(out.offset, 0);

        let events = parsed(&out.rows);
        assert_eq!(
            types(&events),
            vec![
                "user_message",
                "item_completed", // thinking
                "item_completed", // tool_use
                "item_completed", // tool_result
                "item_completed", // assistant text
                "turn_completed",
                "user_message",
                "item_completed",
                "turn_completed",
            ]
        );

        // The echo is gone and the reminder was stripped from the prompt.
        assert_eq!(events[0]["text"], "Fix the failing test");
        assert_eq!(events[6]["text"], "Now add docs");
        assert!(!out.rows.iter().any(|r| r.payload.contains("command-name")));
        assert!(!out.rows.iter().any(|r| r.payload.contains("system-reminder")));

        // Item kinds land as the live translator represents them.
        assert_eq!(events[1]["item"]["kind"], "assistant_thinking");
        assert_eq!(events[2]["item"]["kind"], "tool_use");
        assert_eq!(events[2]["item"]["tool_use_id"], "toolu_1");
        assert_eq!(events[3]["item"]["kind"], "tool_result");
        assert_eq!(events[3]["item"]["tool_use_id"], "toolu_1");
        assert_eq!(events[4]["item"]["kind"], "assistant_text");

        // Persisted-shape check: a user row is byte-identical to what
        // `persist_user_message` writes for a plain text prompt.
        assert_eq!(
            out.rows[0].payload,
            format!(r#"{{"type":"user_message","thread_id":"{THREAD}","text":"Fix the failing test"}}"#)
        );
    }

    #[test]
    fn adopted_history_synthesizes_one_turn_id_per_prompt() {
        let events = parsed(&history_rows(THREAD, &fixture(), 0, 8));
        let first_turn = "u-1";
        let second_turn = "u-4";
        for e in &events[1..6] {
            assert_eq!(e["turn_id"], first_turn, "{e}");
        }
        assert_eq!(events[7]["turn_id"], second_turn);
        assert_eq!(events[8]["turn_id"], second_turn);
        assert_ne!(first_turn, second_turn);
        // Both markers are clean successes with no usage attached.
        for e in [&events[5], &events[8]] {
            assert_eq!(e["type"], "turn_completed");
            assert_eq!(e["status"]["kind"], "success");
            assert!(e["usage"].is_null());
        }
    }

    #[test]
    fn adopted_history_ids_are_negative_and_strictly_increasing() {
        let rows = history_rows(THREAD, &fixture(), 0, 8);
        assert!(rows.iter().all(|r| r.id < 0), "{rows:#?}");
        for pair in rows.windows(2) {
            assert!(pair[0].id < pair[1].id, "{:?} !< {:?}", pair[0].id, pair[1].id);
        }
        // Ids are a function of the source index, not of the page: the
        // same record produces the same id whichever page it arrives in.
        let later_page = history_rows(THREAD, &fixture()[4..], 4, 8);
        let later_content: Vec<i64> = later_page
            .iter()
            .filter(|r| !r.payload.contains("turn_completed"))
            .map(|r| r.id)
            .collect();
        let full_ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert!(!later_content.is_empty());
        for id in later_content {
            assert!(full_ids.contains(&id), "{id} missing from the full page");
        }
    }

    #[test]
    fn adopted_history_parses_timestamps_and_falls_back_to_the_previous_row() {
        let rows = history_rows(THREAD, &fixture(), 0, 8);
        assert_eq!(rows[0].created_at_ms, 1_787_911_200_000);
        assert_eq!(rows[1].created_at_ms, 1_787_911_201_500);
        // "u-4" has an unparseable stamp: it inherits the previous record's.
        let second_prompt = rows.iter().find(|r| r.payload.contains("Now add docs")).unwrap();
        assert_eq!(second_prompt.created_at_ms, 1_787_911_205_000);
    }

    #[test]
    fn adopted_history_never_emits_usage_or_lifecycle_rows() {
        let events = parsed(&history_rows(THREAD, &fixture(), 0, 8));
        for e in &events {
            let ty = e["type"].as_str().unwrap();
            assert!(
                !matches!(
                    ty,
                    "usage_recorded"
                        | "context_usage_updated"
                        | "session_configured"
                        | "resume_cursor_updated"
                        | "runtime_warning"
                        | "plan_usage_updated"
                ),
                "unexpected {ty}"
            );
            assert!(e["usage"].is_null(), "usage leaked on {ty}");
        }
    }

    #[test]
    fn adopted_history_passes_page_bookkeeping_through() {
        let page = SessionMessagesPage {
            messages: fixture()[4..].to_vec(),
            total: 120,
            offset: 116,
        };
        let out = history_page(THREAD, page);
        assert_eq!(out.total, 120);
        assert_eq!(out.offset, 116);
        assert!(!out.rows.is_empty());
        // A page that starts mid-turn still stamps a deterministic turn id
        // on the orphaned leading content.
        let events = parsed(&out.rows);
        assert_eq!(events[0]["type"], "item_completed");
        assert_eq!(events[0]["turn_id"], "adopted-turn-116");
    }

    #[test]
    fn adopted_history_decodes_the_sidecar_result_tolerantly() {
        let page: SessionMessagesPage = serde_json::from_value(json!({
            "messages": [{ "type": "user", "uuid": "x", "message": { "role": "user", "content": "hi" }, "extra": 1 }],
            "total": 1,
            "offset": 0,
            "unexpected": true
        }))
        .expect("decodes");
        assert_eq!(page.messages.len(), 1);
        let empty: SessionMessagesPage = serde_json::from_value(json!({})).expect("defaults");
        assert_eq!(empty.total, 0);
    }

    #[test]
    fn clamp_page_limit_applies_default_and_bounds() {
        assert_eq!(clamp_page_limit(None), DEFAULT_PAGE_LIMIT);
        assert_eq!(clamp_page_limit(Some(0)), 1);
        assert_eq!(clamp_page_limit(Some(10_000)), MAX_PAGE_LIMIT);
        assert_eq!(clamp_page_limit(Some(7)), 7);
    }
}
