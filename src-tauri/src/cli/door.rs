//! How the CLI reaches the control plane: the per-user control socket, as
//! the caller `--client`/`--access` named (the owner when unnamed).
//!
//! Registry tools go through the socket's `tool_call` envelope, never as bare
//! socket commands, so `codemux call` reaches registry tools only. The CLI
//! adds no business defaults: the server mints the caller and enforces
//! access, permission modes and argument schemas.

use super::output::{CliError, ErrorBody};
use crate::control::{send_control_request, ControlRequest};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::Instant;

/// Codes the CLI itself reports; every other code comes from the server.
pub(crate) mod code {
    pub(crate) const UNAVAILABLE: &str = "codemux_unavailable";
    pub(crate) const OUTDATED: &str = "codemux_outdated";
    pub(crate) const INVALID_ARGUMENTS: &str = "invalid_arguments";
    pub(crate) const NO_REPLY: &str = "no_reply";
    pub(crate) const TIMED_OUT: &str = "timed_out";
    /// A newer server reported a run state this CLI does not know.
    pub(crate) const UNEXPECTED_STATE: &str = "unexpected_state";
    /// A server error without a `code:` prefix.
    pub(crate) const UNCODED: &str = "operation_failed";
}

/// One call may take this long before its reply counts as lost.
const CALL_BOUND: Duration = Duration::from_secs(120);
/// A `thread_wait` call may run its own `timeout_ms` plus this.
const WAIT_MARGIN: Duration = Duration::from_secs(30);
/// Inside settle and follow loops, transient failures (an app restart, a
/// lost reply) are retried for this long with the same arguments.
const RETRY_WINDOW: Duration = Duration::from_secs(30);

/// How this invocation reaches registry tools.
pub(crate) enum Door {
    /// The per-user control socket. Every request carries this process's
    /// identity (`ControlClient::current`).
    Socket,
    #[cfg(test)]
    Fake(fake::FakeDoor),
}

/// What `tools_list` answers: MCP tool definitions and who the server says
/// is calling.
pub(crate) struct ToolList {
    pub(crate) caller: Value,
    pub(crate) tools: Vec<Value>,
}

impl Door {
    /// One registry tool call: the tool's value, or its error.
    pub(crate) async fn call(&self, tool: &str, arguments: Value) -> Result<Value, CliError> {
        let bound = call_bound(tool, &arguments);
        match self {
            Door::Socket => {
                let reply = bounded(bound, socket("tool_call", json!({"name": tool, "arguments": arguments}))).await?;
                mcp_result_value(&reply).map_err(CliError::from)
            }
            #[cfg(test)]
            Door::Fake(fake) => {
                let answer = fake.answer(tool, arguments.clone());
                fake::elapse(tool, &arguments, &answer).await;
                answer
            }
        }
    }

    /// The registry tools this caller may run.
    pub(crate) async fn list(&self) -> Result<ToolList, CliError> {
        let data = match self {
            Door::Socket => bounded(CALL_BOUND, socket("tools_list", json!({}))).await?,
            #[cfg(test)]
            Door::Fake(fake) => fake.answer("tools_list", json!({}))?,
        };
        let tools = data["tools"].as_array().cloned().ok_or_else(|| {
            CliError::failed(code::UNCODED, "The running Codemux answered tools_list without a tool list")
        })?;
        Ok(ToolList { caller: data["caller"].clone(), tools })
    }

    /// A call inside a settle or follow loop. What an app restart or a lost
    /// reply causes is retried with the identical arguments, so a mutation's
    /// `client_request_id` keeps it idempotent. A verb's first call never
    /// comes through here, so a stopped app fails fast.
    pub(crate) async fn call_in_loop(&self, tool: &str, arguments: Value) -> Result<Value, CliError> {
        let started = Instant::now();
        let mut backoff = Duration::from_millis(250);
        loop {
            match self.call(tool, arguments.clone()).await {
                Err(error) if is_transient(&error) && started.elapsed() + backoff <= RETRY_WINDOW => {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(2));
                }
                outcome => return outcome,
            }
        }
    }
}

fn call_bound(tool: &str, arguments: &Value) -> Duration {
    match arguments["timeout_ms"].as_u64() {
        Some(wait) if tool == super::verbs::THREAD_WAIT => Duration::from_millis(wait) + WAIT_MARGIN,
        _ => CALL_BOUND,
    }
}

async fn bounded<T>(bound: Duration, work: impl std::future::Future<Output = Result<T, CliError>>) -> Result<T, CliError> {
    tokio::time::timeout(bound, work).await.unwrap_or_else(|_| {
        Err(CliError::failed(
            code::NO_REPLY,
            format!("Codemux sent no reply within {}s. The request may have run; retry it with the same --request-id.", bound.as_secs()),
        ))
    })
}

/// Lost replies and an unreachable app, plus the server's own retryable codes.
fn is_transient(error: &CliError) -> bool {
    let code = error.error.code.as_str();
    code == code::UNAVAILABLE
        || code == code::NO_REPLY
        || crate::agent_control::ErrorCode::parse(code).is_some_and(crate::agent_control::ErrorCode::retryable)
}

async fn socket(command: &str, params: Value) -> Result<Value, CliError> {
    let response = send_control_request(ControlRequest { command: command.into(), params })
        .await
        .map_err(|error| transport_error(&error))?;
    if response.ok {
        Ok(response.data.unwrap_or(Value::Null))
    } else {
        Err(refusal(response.error.as_deref()).into())
    }
}

/// A request that never got an answer. Connection failures mean nothing is
/// listening; anything later (a closed connection, a garbled line) means the
/// request may have run.
pub(crate) fn transport_error(error: &str) -> CliError {
    if error.starts_with("Failed to connect") || error == "Control socket path unavailable" {
        let socket = crate::control::control_socket_candidates()
            .first()
            .map_or_else(|| "the control socket".to_owned(), |path| path.display().to_string());
        return CliError::failed(
            code::UNAVAILABLE,
            format!("Codemux is not running here (nothing answered at {socket}). Start the Codemux app or `codemux serve`."),
        );
    }
    let coded = split_code(error);
    if coded.code != code::UNCODED {
        // `server_too_old` and other refusals the client side raises itself.
        return coded.into();
    }
    CliError::failed(code::NO_REPLY, format!("{error}. The request may have run; retry it with the same --request-id."))
}

/// An `ok:false` reply. An app that does not know a command is older than
/// this CLI: say so, instead of passing its bare "Unknown control command".
pub(crate) fn refusal(error: Option<&str>) -> ErrorBody {
    let text = error.unwrap_or("Codemux refused the request without a reason");
    match text.strip_prefix("Unknown control command: ") {
        Some(command) => ErrorBody::new(
            code::OUTDATED,
            format!(
                "The running Codemux is older than this codemux CLI: it does not know `{command}`. \
                 Restart Codemux (or `codemux serve`) so both are the same version."
            ),
        ),
        None => split_code(text),
    }
}

/// `code: message`, the control plane's error text, as a structured error.
/// A code this build does not know (a newer server) is kept as sent.
pub(crate) fn split_code(text: &str) -> ErrorBody {
    match text.split_once(": ") {
        Some((code, message)) if is_code(code) => ErrorBody::new(code, message),
        _ => ErrorBody::new(code::UNCODED, text),
    }
}

fn is_code(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_lowercase())
        && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// An MCP `tools/call` result as the tool's value or its error.
pub(crate) fn mcp_result_value(result: &Value) -> Result<Value, ErrorBody> {
    let texts: Vec<&str> = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "text")
        .filter_map(|item| item["text"].as_str())
        .collect();
    if result["isError"] == true {
        let error = &result["structuredContent"]["error"];
        return Err(match (error["code"].as_str(), error["message"].as_str()) {
            (Some(code), Some(message)) => ErrorBody::new(code, message),
            _ => split_code(&texts.join("\n")),
        });
    }
    if let Some(structured) = result.get("structuredContent").filter(|value| !value.is_null()) {
        return Ok(structured.clone());
    }
    match (texts.as_slice(), result["content"].as_array().map(Vec::len)) {
        ([text], Some(1)) => Ok(serde_json::from_str(text).unwrap_or_else(|_| Value::String((*text).to_owned()))),
        _ => Ok(json!({"content": result["content"]})),
    }
}

/// Receipt states that are still moving.
fn in_progress(state: &str) -> bool {
    matches!(state, "accepted" | "running")
}

/// Waits until a receipt is final (`succeeded`, `failed`, `needs_attention`)
/// by polling `operation_status`. A value that is not a receipt passes
/// through. The deadline elapsing is `timed_out` (exit 3) with the last
/// receipt attached; the work keeps going.
pub(crate) async fn settle(door: &Door, receipt: Value, deadline: Instant) -> Result<Value, CliError> {
    let (Some(operation), Some(_)) = (receipt["operation_id"].as_str().map(str::to_owned), receipt["state"].as_str()) else {
        return Ok(receipt);
    };
    let mut receipt = receipt;
    let mut pause = Duration::from_millis(100);
    while receipt["state"].as_str().is_some_and(in_progress) {
        if Instant::now() + pause > deadline {
            return Err(CliError::new(
                super::output::Exit::TimedOut,
                code::TIMED_OUT,
                format!("Operation {operation} is still {} at the deadline. It keeps going; check it with `codemux call operation_status`.", receipt["state"].as_str().unwrap_or("running")),
            )
            .with_last(receipt));
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(Duration::from_secs(1));
        receipt = door.call_in_loop(super::verbs::OPERATION_STATUS, json!({"operation_id": operation})).await?;
    }
    Ok(receipt)
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A scripted door: each call must match the next scripted name, gets
    /// its scripted answer, and is recorded with its arguments.
    pub(crate) struct FakeDoor {
        script: Mutex<VecDeque<(String, Result<Value, CliError>)>>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl FakeDoor {
        pub(crate) fn new(script: Vec<(&str, Result<Value, CliError>)>) -> Self {
            Self {
                script: Mutex::new(script.into_iter().map(|(name, answer)| (name.to_owned(), answer)).collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn answer(&self, name: &str, arguments: Value) -> Result<Value, CliError> {
            self.calls.lock().unwrap().push((name.to_owned(), arguments.clone()));
            let (expected, answer) = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unscripted call {name} {arguments}"));
            assert_eq!(expected, name, "unexpected call with {arguments}");
            answer
        }

        pub(crate) fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }

        pub(crate) fn names(&self) -> Vec<String> {
            self.calls().into_iter().map(|(name, _)| name).collect()
        }

        /// Every scripted answer was used.
        pub(crate) fn finished(&self) -> bool {
            self.script.lock().unwrap().is_empty()
        }
    }

    /// A scripted wait that timed out took its whole `timeout_ms`, as the
    /// server's would, so paused-clock tests see deadlines come.
    pub(crate) async fn elapse(tool: &str, arguments: &Value, answer: &Result<Value, CliError>) {
        if let (super::super::verbs::THREAD_WAIT, Ok(waited)) = (tool, answer) {
            if waited["timed_out"] == true {
                tokio::time::sleep(Duration::from_millis(arguments["timeout_ms"].as_u64().unwrap_or(0))).await;
            }
        }
    }

    impl Door {
        pub(crate) fn fake(script: Vec<(&str, Result<Value, CliError>)>) -> Self {
            Door::Fake(FakeDoor::new(script))
        }

        pub(crate) fn script(&self) -> &FakeDoor {
            match self {
                Door::Fake(fake) => fake,
                Door::Socket => unreachable!("a socket door has no script"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_results_map_to_values_and_errors() {
        let structured = json!({"isError": false, "content": [{"type": "text", "text": "{\"a\":1}"}], "structuredContent": {"a": 1}});
        assert_eq!(mcp_result_value(&structured), Ok(json!({"a": 1})));

        let json_text = json!({"content": [{"type": "text", "text": "{\"workspaces\":[]}"}]});
        assert_eq!(mcp_result_value(&json_text), Ok(json!({"workspaces": []})));

        let plain = json!({"content": [{"type": "text", "text": "done"}]});
        assert_eq!(mcp_result_value(&plain), Ok(json!("done")));

        let mixed = json!({"content": [{"type": "text", "text": "a"}, {"type": "image", "data": "…"}]});
        assert_eq!(mcp_result_value(&mixed), Ok(json!({"content": mixed["content"]})));

        let coded = json!({"isError": true, "content": [{"type": "text", "text": "ignored"}],
            "structuredContent": {"error": {"code": "workspace_not_found", "message": "No such workspace", "retryable": false}}});
        assert_eq!(mcp_result_value(&coded), Err(ErrorBody::new("workspace_not_found", "No such workspace")));

        let text_only = json!({"isError": true, "content": [{"type": "text", "text": "permission_denied: Supervised callers cannot"}]});
        assert_eq!(mcp_result_value(&text_only), Err(ErrorBody::new("permission_denied", "Supervised callers cannot")));
    }

    #[test]
    fn socket_errors_keep_their_code() {
        assert_eq!(refusal(Some("permission_denied: Not yours")), ErrorBody::new("permission_denied", "Not yours"));
        // A code from a newer server is kept as sent.
        assert_eq!(refusal(Some("brand_new_code: later")), ErrorBody::new("brand_new_code", "later"));
        // Text without a code prefix still gets one.
        assert_eq!(refusal(Some("missing field `label`")), ErrorBody::new(code::UNCODED, "missing field `label`"));
        assert_eq!(refusal(Some("Turn the connector on first: codemux connector enable")).code, code::UNCODED);
        assert_eq!(refusal(None).code, code::UNCODED);

        let outdated = refusal(Some("Unknown control command: tool_call"));
        assert_eq!(outdated.code, code::OUTDATED);
        assert!(outdated.message.contains("`tool_call`") && outdated.message.contains("Restart Codemux"), "{outdated}");
    }

    #[test]
    fn transport_errors_tell_a_stopped_app_from_a_lost_reply() {
        let stopped = transport_error("Failed to connect to Codemux control endpoint: No such file or directory (os error 2)");
        assert_eq!(stopped.error.code, code::UNAVAILABLE);
        assert!(stopped.error.message.contains("codemux serve"), "{stopped}");
        assert!(is_transient(&stopped));

        let lost = transport_error("No response received from Codemux");
        assert_eq!(lost.error.code, code::NO_REPLY);
        assert!(lost.error.message.contains("--request-id"), "{lost}");
        assert!(is_transient(&lost));

        let too_old = transport_error("server_too_old: the running Codemux does not enforce --access; restart Codemux");
        assert_eq!(too_old.error.code, "server_too_old");
        assert!(!is_transient(&too_old));
        assert!(!is_transient(&CliError::failed("workspace_not_found", "x")));
        assert!(is_transient(&CliError::failed("wait_budget_exhausted", "x")));
    }

    #[test]
    fn waits_get_their_own_timeout_plus_a_margin() {
        assert_eq!(call_bound("thread_wait", &json!({"timeout_ms": 60_000})), Duration::from_secs(90));
        assert_eq!(call_bound("thread_wait", &json!({})), CALL_BOUND);
        assert_eq!(call_bound("thread_read", &json!({"timeout_ms": 1})), CALL_BOUND);
    }

    #[tokio::test(start_paused = true)]
    async fn settling_polls_until_the_receipt_is_final() {
        let door = Door::fake(vec![
            ("operation_status", Ok(json!({"operation_id": "op", "state": "running"}))),
            ("operation_status", Err(CliError::failed(code::UNAVAILABLE, "restarting"))),
            ("operation_status", Ok(json!({"operation_id": "op", "state": "succeeded", "result": {"ok": 1}}))),
        ]);
        let deadline = Instant::now() + Duration::from_secs(120);
        let settled = settle(&door, json!({"operation_id": "op", "state": "accepted"}), deadline).await.unwrap();
        assert_eq!(settled["state"], "succeeded");
        assert!(door.script().finished());
        // Every poll asked for the same operation.
        assert!(door.script().calls().iter().all(|(_, args)| args == &json!({"operation_id": "op"})));

        // Not a receipt: passes through without a call.
        let plain = Door::fake(vec![]);
        assert_eq!(settle(&plain, json!({"workspaces": []}), deadline).await.unwrap(), json!({"workspaces": []}));
    }

    #[tokio::test(start_paused = true)]
    async fn settling_past_the_deadline_times_out_with_the_last_receipt() {
        let running = || Ok(json!({"operation_id": "op", "state": "running"}));
        let door = Door::fake(vec![("operation_status", running()), ("operation_status", running()), ("operation_status", running())]);
        let deadline = Instant::now() + Duration::from_millis(500);
        let error = settle(&door, json!({"operation_id": "op", "state": "accepted"}), deadline).await.unwrap_err();
        assert_eq!((error.error.code.as_str(), error.exit.code()), (code::TIMED_OUT, 3));
        assert_eq!(error.last, Some(json!({"operation_id": "op", "state": "running"})));
        assert!(!door.script().names().iter().any(|name| name != "operation_status"), "a deadline never cancels work");
    }
}
