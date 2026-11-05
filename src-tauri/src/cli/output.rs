//! How the CLI reports a result: the exit status scripts branch on, and the
//! writers every command prints through. Results go to stdout, hints go to
//! stderr, and a reader that went away (`codemux logs | head -1`) ends the
//! output quietly instead of panicking the way `println!` does.

use serde::Serialize;
use serde_json::Value;
use std::fmt;
use std::io::{self, Write};

/// Process exit status of a CLI command. Stable: scripts branch on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The command succeeded. A waited run completed.
    Done,
    /// A transport failure, an `ok:false` reply, a tool error, or a run that
    /// ended without completing.
    Failed,
    /// A waited run is blocked on an approval, a question or a secret card.
    NeedsInput,
    /// The deadline elapsed while work was still running. Nothing was cancelled.
    TimedOut,
    /// clap rejected the arguments. EX_USAGE instead of clap's 2, so 2 only
    /// ever means "needs input".
    Usage,
}

impl Exit {
    pub fn code(self) -> i32 {
        match self {
            Self::Done => 0,
            Self::Failed => 1,
            Self::NeedsInput => 2,
            Self::TimedOut => 3,
            Self::Usage => 64,
        }
    }
}

/// A failure as the JSON envelopes carry it: `{"code": …, "message": …}`.
/// Server codes pass through unchanged; the CLI adds a few of its own
/// (`codemux_unavailable`, `invalid_arguments`, `timed_out`, …).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ErrorBody {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl ErrorBody {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Same `code: message` shape the control plane's errors print with, so a
/// human reading stderr sees one format whichever side failed.
impl fmt::Display for ErrorBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// A command that could not finish: what to report and the exit status it
/// maps to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CliError {
    pub(crate) error: ErrorBody,
    pub(crate) exit: Exit,
    /// The last value seen before giving up, such as the receipt a settle
    /// loop timed out on, so JSON output can still name the thread.
    pub(crate) last: Option<Value>,
}

impl CliError {
    pub(crate) fn new(exit: Exit, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: ErrorBody::new(code, message),
            exit,
            last: None,
        }
    }

    pub(crate) fn failed(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Exit::Failed, code, message)
    }

    pub(crate) fn with_last(mut self, last: Value) -> Self {
        self.last = Some(last);
        self
    }
}

impl From<ErrorBody> for CliError {
    fn from(error: ErrorBody) -> Self {
        Self {
            error,
            exit: Exit::Failed,
            last: None,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

/// What `call` and every non-waiting verb print: `{ok, tool, result}` or
/// `{ok, tool, error: {code, message}}`. A verb that settles a receipt keeps
/// the receipt in `result` and sets `ok` from its state, so a failed receipt
/// is `ok: false` with a `result`; hence the public fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct CallEnvelope {
    pub(crate) ok: bool,
    pub(crate) tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<ErrorBody>,
}

impl CallEnvelope {
    /// A tool's outcome exactly as it came back, the way `call` reports it.
    pub(crate) fn of(tool: impl Into<String>, outcome: Result<Value, ErrorBody>) -> Self {
        let (result, error) = match outcome {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            ok: error.is_none(),
            tool: tool.into(),
            result,
            error,
        }
    }

    /// 0 or 1. Only a run report exits 2 (needs input) or 3 (timed out).
    pub(crate) fn exit(&self) -> Exit {
        if self.ok {
            Exit::Done
        } else {
            Exit::Failed
        }
    }

    /// A verb's settled outcome. A receipt that did not succeed is `ok: false`
    /// with the receipt in `result` and its reason in `error`. A failure that
    /// stopped at a receipt (a settle deadline) keeps that receipt too.
    pub(crate) fn settled(tool: &str, outcome: Result<Value, CliError>) -> (Self, Exit) {
        match outcome {
            Ok(value) if succeeded(&value) => (Self::of(tool, Ok(value)), Exit::Done),
            Ok(receipt) => {
                let error = receipt_error(&receipt);
                (Self { ok: false, tool: tool.into(), result: Some(receipt), error: Some(error) }, Exit::Failed)
            }
            Err(failure) => (Self { ok: false, tool: tool.into(), result: failure.last, error: Some(failure.error) }, failure.exit),
        }
    }
}

/// Whether `value` is an operation receipt: it names its operation and the
/// operation's state. Other values with a `state`, such as `thread_status`
/// (the run's state), are plain results.
pub(crate) fn is_receipt(value: &Value) -> bool {
    value["operation_id"].is_string() && value["state"].is_string()
}

/// Whether a settled value means the action happened: a receipt that
/// succeeded, or any plain result.
pub(crate) fn succeeded(value: &Value) -> bool {
    !is_receipt(value) || value["state"] == "succeeded"
}

/// Why a settled receipt did not succeed: its own error, else its state.
pub(crate) fn receipt_error(receipt: &Value) -> ErrorBody {
    let error = &receipt["error"];
    match (error["code"].as_str(), error["message"].as_str()) {
        (Some(code), Some(message)) => ErrorBody::new(code, message),
        _ if receipt["state"] == "needs_attention" => ErrorBody::new(
            "operation_failed",
            "Codemux could not confirm whether this request ran (needs_attention). Inspect the thread before sending it again.",
        ),
        _ => ErrorBody::new("operation_failed", format!("The operation ended {}", receipt["state"].as_str().unwrap_or("without a state"))),
    }
}

/// Print a non-waiting verb's outcome: the call envelope with `--json`;
/// otherwise `text` for a success, or `code: message` on stderr. A mutation
/// whose reply was lost also gets the retry hint for its key.
pub(crate) fn report(
    tool: &str,
    outcome: Result<Value, CliError>,
    json: bool,
    request_id: Option<&str>,
    text: impl FnOnce(&Value) -> Result<(), String>,
) -> Result<Exit, String> {
    if let (Err(failure), Some(key)) = (&outcome, request_id) {
        retry_hint(failure, key);
    }
    let (envelope, exit) = CallEnvelope::settled(tool, outcome);
    if json {
        emit_json(&envelope)?;
    } else if envelope.ok {
        text(envelope.result.as_ref().unwrap_or(&Value::Null))?;
    } else if let Some(error) = &envelope.error {
        note(&error.to_string());
    }
    Ok(exit)
}

/// When nothing confirmed whether a mutation ran, the same key makes trying
/// again safe.
pub(crate) fn retry_hint(failure: &CliError, request_id: &str) {
    if matches!(failure.error.code.as_str(), "no_reply" | "codemux_unavailable") {
        note(&format!("Retry safely with --request-id {request_id}"));
    }
}

/// Rows as columns two spaces apart, the last column unpadded.
pub(crate) fn table(rows: &[Vec<String>]) -> Result<(), String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| rows.iter().filter_map(|row| row.get(column)).map(|cell| cell.chars().count()).max().unwrap_or(0))
        .collect();
    for row in rows {
        let mut line = String::new();
        for (column, cell) in row.iter().enumerate() {
            if column + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{cell:<width$}  ", width = widths[column]));
            }
        }
        out(line.trim_end())?;
    }
    Ok(())
}

/// Print one line of the command's result on stdout.
pub(crate) fn out(text: &str) -> Result<(), String> {
    write_line(&mut io::stdout().lock(), text)
}

/// Print a value on stdout as pretty JSON, the shape every JSON-printing
/// verb has always used.
pub(crate) fn emit_json(value: &impl Serialize) -> Result<(), String> {
    write_json(&mut io::stdout().lock(), value)
}

/// A progress line or hint on stderr, so stdout stays machine-readable.
/// A stderr that cannot be written has nowhere to report to, so failures
/// are dropped.
pub(crate) fn note(text: &str) {
    let _ = write_line(&mut io::stderr().lock(), text);
}

pub(crate) fn write_json(writer: &mut impl Write, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    write_line(writer, &text)
}

/// Write `text` and a newline, then flush. A closed pipe is not a failure:
/// the reader stopped listening, which is its call (`| head -1`). Any other
/// write error is, so a full disk never passes as success.
pub(crate) fn write_line(writer: &mut impl Write, text: &str) -> Result<(), String> {
    match writeln!(writer, "{text}").and_then(|()| writer.flush()) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            Err(format!("Could not write the output: {error}"))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exit_codes_are_stable() {
        // Scripts and the outside-agent guide branch on these numbers.
        let codes: Vec<i32> = [Exit::Done, Exit::Failed, Exit::NeedsInput, Exit::TimedOut, Exit::Usage]
            .into_iter()
            .map(Exit::code)
            .collect();
        assert_eq!(codes, [0, 1, 2, 3, 64]);
    }

    /// A writer that fails every write with one error kind.
    struct Failing(io::ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_pipe_ends_output_quietly_but_other_write_errors_fail() {
        assert_eq!(write_line(&mut Failing(io::ErrorKind::BrokenPipe), "x"), Ok(()));
        assert_eq!(write_json(&mut Failing(io::ErrorKind::BrokenPipe), &json!({"ok": true})), Ok(()));

        let error = write_line(&mut Failing(io::ErrorKind::PermissionDenied), "x").unwrap_err();
        assert!(error.starts_with("Could not write the output:"), "{error}");
    }

    #[test]
    fn json_prints_pretty_with_a_trailing_newline() {
        let mut printed = Vec::new();
        write_json(&mut printed, &json!({"data": {"n": 1}})).unwrap();
        assert_eq!(
            String::from_utf8(printed).unwrap(),
            "{\n  \"data\": {\n    \"n\": 1\n  }\n}\n"
        );
    }

    #[test]
    fn errors_serialize_as_code_and_message_and_print_like_control_errors() {
        let body = ErrorBody::new("codemux_unavailable", "Start the Codemux app or `codemux serve`.");
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({"code": "codemux_unavailable", "message": "Start the Codemux app or `codemux serve`."})
        );
        assert_eq!(body.to_string(), "codemux_unavailable: Start the Codemux app or `codemux serve`.");

        let error = CliError::from(body.clone());
        assert_eq!((error.exit, error.last.as_ref()), (Exit::Failed, None));
        assert_eq!(error.to_string(), body.to_string());

        let timed_out = CliError::new(Exit::TimedOut, "timed_out", "Still running after 2m.")
            .with_last(json!({"operation_id": "op-1", "state": "running"}));
        assert_eq!(timed_out.exit.code(), 3);
        assert_eq!(timed_out.last, Some(json!({"operation_id": "op-1", "state": "running"})));
        assert_eq!(CliError::failed("tool_error", "boom").exit, Exit::Failed);
    }

    #[test]
    fn call_envelopes_carry_the_result_or_the_error() {
        let listed = CallEnvelope::of("workspace_list", Ok(json!({"workspaces": []})));
        assert_eq!(listed.exit(), Exit::Done);
        // Printed in field order, so `ok` is always the first key a reader sees.
        let mut printed = Vec::new();
        write_json(&mut printed, &listed).unwrap();
        assert_eq!(
            String::from_utf8(printed).unwrap(),
            "{\n  \"ok\": true,\n  \"tool\": \"workspace_list\",\n  \"result\": {\n    \"workspaces\": []\n  }\n}\n"
        );

        let refused = CallEnvelope::of(
            "thread_read",
            Err(ErrorBody::new("workspace_not_found", "Select an existing workspace explicitly")),
        );
        assert_eq!(refused.exit(), Exit::Failed);
        assert_eq!(
            serde_json::to_value(&refused).unwrap(),
            json!({
                "ok": false,
                "tool": "thread_read",
                "error": {"code": "workspace_not_found", "message": "Select an existing workspace explicitly"},
            })
        );

        // A tool that returns null still reports a result, not a missing one.
        assert_eq!(
            serde_json::to_value(CallEnvelope::of("thread_stop", Ok(Value::Null))).unwrap(),
            json!({"ok": true, "tool": "thread_stop", "result": null})
        );
    }

    #[test]
    fn settled_receipts_report_why_they_did_not_succeed() {
        assert!(succeeded(&json!({"operation_id": "op", "state": "succeeded"})));
        assert!(succeeded(&json!({"workspaces": []})));
        assert!(!succeeded(&json!({"operation_id": "op", "state": "failed"})));
        let failed = json!({"operation_id": "op", "state": "failed", "error": {"code": "provider_not_ready", "message": "Starting", "retryable": true}});
        assert_eq!(receipt_error(&failed), ErrorBody::new("provider_not_ready", "Starting"));
        assert!(receipt_error(&json!({"operation_id": "op", "state": "needs_attention"})).message.contains("needs_attention"));

        let (envelope, exit) = CallEnvelope::settled("thread_stop", Ok(failed.clone()));
        assert_eq!(exit, Exit::Failed);
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            json!({"ok": false, "tool": "thread_stop", "result": failed, "error": {"code": "provider_not_ready", "message": "Starting"}})
        );
        let (envelope, exit) = CallEnvelope::settled("thread_stop", Ok(json!({"operation_id": "op", "state": "succeeded"})));
        assert_eq!((envelope.ok, exit), (true, Exit::Done));

        // A run's state is not a receipt's: thread_status succeeds whatever
        // state the run is in.
        for state in ["running", "needs_input", "done", "failed"] {
            let status = json!({"thread_id": "th", "phase": "completed", "state": state, "failure": null});
            assert!(!is_receipt(&status) && succeeded(&status), "{state}");
            let (envelope, exit) = CallEnvelope::settled("thread_status", Ok(status));
            assert_eq!((envelope.ok, exit, envelope.error), (true, Exit::Done, None), "{state}");
        }

        // A settle deadline keeps the receipt it stopped at and exits 3.
        let receipt = json!({"operation_id": "op", "state": "running"});
        let late = CliError::new(Exit::TimedOut, "timed_out", "still running").with_last(receipt.clone());
        let (envelope, exit) = CallEnvelope::settled("thread_launch", Err(late));
        assert_eq!((envelope.ok, exit, envelope.result), (false, Exit::TimedOut, Some(receipt)));
    }
}
