//! Test helper binary: a minimal stand-in for `agy acp`.
//!
//! Speaks the parts of Antigravity's ACP server the Codemux adapter relies
//! on, shaped like `agy acp` 1.5.6:
//!   - `initialize` advertises the four Antigravity auth methods and the
//!     `resume`/`close` session capabilities. `authenticate` accepts
//!     `oauth-personal`, unless `FAKE_AGY_SIGNED_OUT` is set, when it fails
//!     the way a signed-out machine without a browser does.
//!   - `session/new` and `session/resume` answer with config options listing
//!     the Model option FIRST, then Mode, then a thinking level whose values
//!     depend on the selected model's family.
//!   - `session/set_config_option` validates the value and echoes the options.
//!   - `session/prompt` behaviour is chosen by the prompt text:
//!       * `need-approval` asks `session/request_permission` with allow-once /
//!         allow-always / reject-once options and answers `approval:<id>`.
//!         `need-lasting-approval` offers only allow-always / reject-once.
//!       * `ask-question` asks an `interaction_` question (two answers) and
//!         answers `answer:<id>`.
//!       * `self-mode:<mode>` switches its own mode, announcing it with a
//!         `current_mode_update`, and answers `ok`.
//!       * `report` answers `mode:<mode> model:<model> thinking:<level>`.
//!       * `arm-drift` makes the next switch to `plan` be acknowledged and
//!         then immediately followed by a switch of its own to `auto`.
//!       * anything else answers `ok`.
//!
//! With `FAKE_AGY_MODE_LOG` set, every mode change is appended to that file,
//! so a test can read the native mode without sending a prompt (a prompt
//! makes the adapter enforce the mode first).
//!
//! Shutdown: closing stdin ends the process.

use std::io::{BufRead, BufReader, Write};

use serde_json::{json, Value};

const SESSION_ID: &str = "agy-session-1";
const MODES: [&str; 4] = ["default", "accept-edits", "auto", "plan"];

fn write_line(value: &Value) {
    let mut bytes = serde_json::to_vec(value).expect("serialize");
    bytes.push(b'\n');
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&bytes).expect("write");
    handle.flush().expect("flush");
}

fn respond(id: &Value, result: Value) {
    write_line(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn fail(id: &Value, code: i64, message: &str) {
    write_line(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
}

fn chunk(text: &str) {
    write_line(&json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": SESSION_ID,
            "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } }
        }
    }));
}

fn log_mode(mode: &str) {
    if let Some(path) = std::env::var_os("FAKE_AGY_MODE_LOG") {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("mode log");
        writeln!(file, "{mode}").expect("write mode log");
    }
}

fn thinking_levels(model: &str) -> &'static [&'static str] {
    if model == "gemini-flash" {
        &["minimal", "medium"]
    } else {
        &["low", "high"]
    }
}

struct State {
    authenticated: bool,
    mode: String,
    model: String,
    thinking: String,
    /// The `session/prompt` id and kind waiting on our own outgoing request.
    waiting: Option<(Value, &'static str)>,
    /// Drift to `auto` right after acknowledging the next switch to `plan`.
    drift_after_plan: bool,
}

impl State {
    fn config_options(&self) -> Value {
        let options = |values: &[&str]| {
            values
                .iter()
                .map(|value| json!({ "value": value, "name": value }))
                .collect::<Vec<_>>()
        };
        json!([
            { "id": "model", "name": "Model", "category": "model", "type": "select",
              "currentValue": self.model,
              "options": [{ "value": "gemini-pro", "name": "Gemini Pro" },
                          { "value": "gemini-flash", "name": "Gemini Flash" }] },
            { "id": "mode", "name": "Mode", "category": "mode", "type": "select",
              "currentValue": self.mode, "options": options(&MODES) },
            { "id": "thinking_level", "name": "Thinking level", "category": "thought_level",
              "type": "select", "currentValue": self.thinking,
              "options": options(thinking_levels(&self.model)) }
        ])
    }

    fn set_option(&mut self, id: &Value, params: &Value) {
        let value = params.get("value").and_then(Value::as_str).unwrap_or_default();
        let accepted = match params.get("configId").and_then(Value::as_str) {
            Some("mode") if MODES.contains(&value) => {
                self.mode = value.into();
                log_mode(&self.mode);
                true
            }
            Some("model") if matches!(value, "gemini-pro" | "gemini-flash") => {
                self.model = value.into();
                self.thinking = thinking_levels(value)[1].into();
                true
            }
            Some("thinking_level") if thinking_levels(&self.model).contains(&value) => {
                self.thinking = value.into();
                true
            }
            _ => false,
        };
        if accepted {
            respond(id, json!({ "configOptions": self.config_options() }));
            if self.drift_after_plan && self.mode == "plan" {
                self.drift_after_plan = false;
                self.mode = "auto".into();
                log_mode(&self.mode);
                write_line(&json!({
                    "jsonrpc": "2.0", "method": "session/update",
                    "params": { "sessionId": SESSION_ID,
                                "update": { "sessionUpdate": "current_mode_update", "currentModeId": "auto" } }
                }));
            }
        } else {
            fail(id, -32602, &format!("Unknown config option value: {value}"));
        }
    }

    fn prompt(&mut self, id: Value, text: &str) {
        if text.contains("need-lasting-approval") {
            write_line(&json!({
                "jsonrpc": "2.0", "id": "perm-3", "method": "session/request_permission",
                "params": {
                    "sessionId": SESSION_ID,
                    "toolCall": { "toolCallId": "call_2", "title": "Fetch a URL" },
                    "options": [
                        { "optionId": "always", "name": "Allow always", "kind": "allow_always" },
                        { "optionId": "reject", "name": "Deny", "kind": "reject_once" }
                    ]
                }
            }));
            self.waiting = Some((id, "approval"));
        } else if text.contains("need-approval") {
            write_line(&json!({
                "jsonrpc": "2.0", "id": "perm-1", "method": "session/request_permission",
                "params": {
                    "sessionId": SESSION_ID,
                    "toolCall": { "toolCallId": "call_1", "title": "Run ls" },
                    "options": [
                        { "optionId": "once", "name": "Allow once", "kind": "allow_once" },
                        { "optionId": "always", "name": "Allow always", "kind": "allow_always" },
                        { "optionId": "reject", "name": "Deny", "kind": "reject_once" }
                    ]
                }
            }));
            self.waiting = Some((id, "approval"));
        } else if text.contains("ask-question") {
            write_line(&json!({
                "jsonrpc": "2.0", "id": "perm-2", "method": "session/request_permission",
                "params": {
                    "sessionId": SESSION_ID,
                    "toolCall": { "toolCallId": "interaction_1", "title": "Which database?" },
                    "options": [
                        { "optionId": "opt-a", "name": "Postgres", "kind": "allow_once" },
                        { "optionId": "opt-b", "name": "SQLite", "kind": "allow_once" }
                    ]
                }
            }));
            self.waiting = Some((id, "answer"));
        } else if let Some(mode) = text.split("self-mode:").nth(1) {
            self.mode = mode.trim().to_string();
            log_mode(&self.mode);
            write_line(&json!({
                "jsonrpc": "2.0", "method": "session/update",
                "params": { "sessionId": SESSION_ID,
                            "update": { "sessionUpdate": "current_mode_update", "currentModeId": self.mode } }
            }));
            chunk("ok");
            respond(&id, json!({ "stopReason": "end_turn" }));
        } else if text.contains("arm-drift") {
            self.drift_after_plan = true;
            chunk("ok");
            respond(&id, json!({ "stopReason": "end_turn" }));
        } else if text.contains("report") {
            chunk(&format!("mode:{} model:{} thinking:{}", self.mode, self.model, self.thinking));
            respond(&id, json!({ "stopReason": "end_turn" }));
        } else {
            chunk("ok");
            respond(&id, json!({ "stopReason": "end_turn" }));
        }
    }

    fn answered(&mut self, response: &Value) {
        let Some((prompt_id, kind)) = self.waiting.take() else { return };
        let outcome = response.pointer("/result/outcome");
        let picked = match outcome.and_then(|o| o.get("outcome")).and_then(Value::as_str) {
            Some("selected") => outcome
                .and_then(|o| o.get("optionId"))
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            _ => "cancelled".into(),
        };
        chunk(&format!("{kind}:{picked}"));
        respond(&prompt_id, json!({ "stopReason": "end_turn" }));
    }
}

fn main() {
    let mut state = State {
        authenticated: false,
        mode: "default".into(),
        model: "gemini-pro".into(),
        thinking: "high".into(),
        waiting: None,
        drift_after_plan: false,
    };
    let stdin = std::io::stdin();
    for line in BufReader::new(stdin.lock()).lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            state.answered(&message);
            continue;
        };
        match method {
            "initialize" => respond(&id, json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": { "image": true, "audio": false, "embeddedContext": true },
                    "sessionCapabilities": { "resume": {}, "close": {}, "load": {} }
                },
                "agentInfo": { "name": "Antigravity ACP Server", "version": "0.0.0-fake" },
                "authMethods": [
                    { "id": "oauth-personal", "name": "Log in with Google" },
                    { "id": "oauth-business", "name": "Log in with Gemini Enterprise" },
                    { "id": "gemini-api-key", "name": "Gemini API key" },
                    { "id": "agent-platform", "name": "Gemini Enterprise Agent Platform" }
                ]
            })),
            "authenticate" => {
                if std::env::var_os("FAKE_AGY_SIGNED_OUT").is_some() {
                    fail(&id, -32603, "No browser is available here (no graphical display is available to open a browser). Run `agy acp --login` in a terminal on this machine, then retry.");
                } else if params.get("methodId").and_then(Value::as_str) == Some("oauth-personal") {
                    state.authenticated = true;
                    respond(&id, json!({}));
                } else {
                    fail(&id, -32602, "unexpected auth method");
                }
            }
            "session/new" | "session/resume" if !state.authenticated => {
                fail(&id, -32000, "Authentication required")
            }
            "session/new" => respond(&id, json!({ "sessionId": SESSION_ID, "configOptions": state.config_options() })),
            "session/resume" => respond(&id, json!({ "configOptions": state.config_options() })),
            "session/set_config_option" => state.set_option(&id, &params),
            "session/set_mode" => fail(&id, -32601, "use the mode config option"),
            "session/close" => respond(&id, json!({})),
            "session/prompt" => {
                let text = params
                    .get("prompt")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(" ");
                state.prompt(id, &text);
            }
            "session/cancel" => {}
            _ if !id.is_null() => fail(&id, -32601, &format!("Method not found: {method}")),
            _ => {}
        }
    }
}
