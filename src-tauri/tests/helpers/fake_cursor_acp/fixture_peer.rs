//! Opt-in native equivalent of `agent_control_acp_peer.py`.
//! Runs as the transport's direct owned child, not an interpreter/wrapper.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

struct Output {
    rig: PathBuf,
    // Main input handling and the delayed config rejection share one writer.
    // Keep each wire record + stdout line atomic, including flush.
    writer: Mutex<()>,
}
impl Output {
    fn record(&self, value: &Value) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.rig.join("peer-wire.jsonl"))
            .expect("open fixture wire log");
        serde_json::to_writer(&mut file, value).expect("record fixture wire");
        writeln!(file).expect("terminate fixture wire line");
    }
    fn incoming(&self, message: &Value) {
        let _writer = self.writer.lock().unwrap();
        self.record(&json!({"direction": "in", "message": message}));
    }
    fn send(&self, mut message: Value) {
        let _writer = self.writer.lock().unwrap();
        message["jsonrpc"] = json!("2.0");
        self.record(&json!({"direction": "out", "message": message}));
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        serde_json::to_writer(&mut stdout, &message).expect("write fixture response");
        writeln!(stdout).expect("terminate fixture response");
        stdout.flush().expect("flush fixture response");
    }
    fn update(&self, update: Value) {
        self.send(json!({"method": "session/update", "params": {
            "sessionId": "review-session", "update": update,
        }}));
    }
    fn marker(&self, name: &str, message: &Value) {
        std::fs::write(self.rig.join(name), serde_json::to_vec(message).unwrap())
            .expect("write fixture marker");
    }
}

fn options() -> Value {
    json!([
        {"id":"model", "category":"model", "type":"select", "currentValue":"fake-model",
         "options":[{"value":"fake-model","name":"Synthetic"},
                    {"value":"review-fatal","name":"Synthetic RPC failure"},
                    {"value":"stop-race-held","name":"Held actual config"},
                    {"value":"stop-race-reject","name":"Held config error"}]},
        {"id":"mode", "category":"mode", "type":"select", "currentValue":"ask",
         "options":[{"value":"ask","name":"Ask"},{"value":"agent","name":"Agent"}]}
    ])
}

// Match Python's str(id), preserving opaque server request IDs as strings.
fn id_text(id: &Value) -> String {
    id.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| id.to_string())
}

pub(super) fn run(rig: PathBuf) {
    std::fs::create_dir_all(&rig).expect("create fixture rig");
    std::fs::write(rig.join("peer-pid"), std::process::id().to_string())
        .expect("write direct child PID");
    let output = Arc::new(Output {
        rig,
        writer: Mutex::new(()),
    });
    // Preserve insertion order when cancelling all outstanding permissions.
    let mut held: Vec<(String, Value)> = Vec::new();
    let mut mode = String::new();
    let stdin = std::io::stdin();
    for line in BufReader::new(stdin.lock()).lines() {
        let message: Value =
            serde_json::from_str(&line.expect("read fixture input")).expect("parse fixture input");
        output.incoming(&message);
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            if let Some(index) = held
                .iter()
                .position(|(request, _)| *request == id_text(&id))
            {
                let (_, original) = held.remove(index);
                if mode == "before-terminal" {
                    std::process::exit(37);
                }
                let chosen = message
                    .pointer("/result/outcome/optionId")
                    .and_then(Value::as_str)
                    .unwrap_or("cancelled");
                output.update(
                    json!({"sessionUpdate":"tool_call_update", "toolCallId":"review-tool",
                    "status":"completed", "content":[{"type":"content", "content":{
                        "type":"text", "text":format!("visible result {chosen}")}}]}),
                );
                output.update(json!({"sessionUpdate":"agent_message_chunk",
                    "content":{"type":"text", "text":format!("wire-option:{chosen}")}}));
                output.send(json!({"id":original, "result":{"stopReason":"end_turn"}}));
            }
            continue;
        };
        match method {
            "initialize" | "authenticate" | "session/load" => {
                output.send(json!({"id":id, "result":{}}));
            }
            "session/new" => {
                output.send(json!({"id":id, "result":{
                    "sessionId":"review-session", "configOptions":options(),
                }}));
            }
            "session/set_config_option" => match params.get("value").and_then(Value::as_str) {
                Some("stop-race-reject") => {
                    output.marker("config-reject-held.json", &message);
                    let output = output.clone();
                    std::thread::spawn(move || {
                        while !output.rig.join("release-config-reject").is_file() {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        output.send(json!({"id":id, "error":{
                            "code":-32000, "message":"synthetic held configuration rejection",
                        }}));
                    });
                }
                Some("stop-race-held") => output.marker("config-held.json", &message),
                Some("review-fatal") => output.send(json!({"id":id, "error":{
                    "code":-32000, "message":"synthetic config transport failure",
                }})),
                _ => output.send(json!({"id":id, "result":{"configOptions":options()}})),
            },
            "session/prompt" => {
                let text = params
                    .get("prompt")
                    .and_then(Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .map(|block| block.get("text").and_then(Value::as_str).unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                if text.contains("review-hold")
                    || text.contains("review-approval")
                    || text.contains("fr1-hold")
                {
                    if text.contains("fr1-hold") {
                        mode = text.split_whitespace().last().unwrap_or("").into();
                        std::fs::write(
                            output.rig.join("fr1-peer-pid"),
                            std::process::id().to_string(),
                        )
                        .expect("write held direct child PID");
                    }
                    output.update(
                        json!({"sessionUpdate":"tool_call", "toolCallId":"review-tool",
                        "title":"Synthetic review tool", "kind":"execute", "status":"pending",
                        "rawInput":{"command":"synthetic"}}),
                    );
                    let request = format!("peer-{}", id_text(&id));
                    held.push((request.clone(), id));
                    let kinds: &[&str] = if text.contains("only-always") {
                        &["allow_always"]
                    } else {
                        &["allow_once", "allow_always", "reject_once", "reject_always"]
                    };
                    let options: Vec<_> = kinds
                        .iter()
                        .map(|kind| {
                            json!({
                                "optionId":format!("opaque-{kind}"), "kind":kind, "name":kind,
                            })
                        })
                        .collect();
                    output.send(json!({"id":request, "method":"session/request_permission", "params":{
                        "sessionId":"review-session", "toolCall":{"toolCallId":"review-tool"}, "options":options,
                    }}));
                } else if text == "review-runtime-fatal" {
                    output.send(json!({"id":id, "error":{
                        "code":-32000, "message":"synthetic runtime transport failure",
                    }}));
                } else {
                    output.update(json!({"sessionUpdate":"agent_message_chunk", "content":{
                        "type":"text", "text":text,
                    }}));
                    output.send(json!({"id":id, "result":{"stopReason":"end_turn"}}));
                }
            }
            "session/cancel" => {
                for (_, original) in held.drain(..) {
                    output.send(json!({"id":original, "result":{"stopReason":"cancelled"}}));
                }
            }
            _ if !id.is_null() => output.send(json!({"id":id, "result":{}})),
            _ => {}
        }
    }
}
