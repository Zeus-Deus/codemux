//! Native OpenCode 2.x wire boundary. Legacy SDK DTOs are not native-v2 DTOs.
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};

/// Stable updates consumed by the runtime bridge; no v1 wire-defaulting.
#[derive(Debug, Clone)]
pub enum Update {
    Content {
        thinking: bool,
        complete: bool,
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        content: Value,
        error: bool,
    },
    Usage(Value),
    Started,
    Terminal {
        outcome: String,
        error: Option<Value>,
    },
    Child {
        id: String,
        status: String,
        detail: Value,
    },
    Permission(Value),
    Form(Value),
    Resolved {
        id: String,
        answer: Option<Value>,
    },
    Compacted,
}

#[derive(Debug, Default)]
pub struct State {
    running: HashSet<String>,
    settled: HashSet<String>,
    recovered_idle: HashMap<String, Value>,
    seen: HashSet<String>,
    recent: VecDeque<String>,
    content: HashMap<(String, String, bool, u64), (String, bool)>,
    tools: HashMap<(String, String, String), Tool>,
    models: HashMap<String, Value>,
}

#[derive(Debug, Default)]
struct Tool {
    name: String,
    input: Value,
    raw: String,
    metadata: Value,
    called: bool,
    ended: bool,
}

impl State {
    pub fn mark_running(&mut self, session: &str) {
        self.running.insert(session.into());
    }
    /// Claim a durable descendant terminal once per idle boundary. A live
    /// terminal already observed before the first snapshot must not repeat.
    pub fn recover_terminal(&mut self, session: &str, boundary: Value) -> bool {
        let previous = self.recovered_idle.insert(session.into(), boundary.clone());
        self.running.contains(session)
            || previous.as_ref().is_some_and(|old| old != &boundary)
            || (previous.is_none() && !self.settled.contains(session))
    }
    pub fn reduce(&mut self, event: &Event) -> Result<Vec<Update>, String> {
        // Snapshot identities may recur with a newer partial value. Their
        // per-item state/high-water guards, not event IDs, reconcile them.
        if !event.id.starts_with("snapshot:") {
            if !self.seen.insert(event.id.clone()) {
                return Ok(vec![]);
            }
            self.recent.push_back(event.id.clone());
            if self.recent.len() > 2048 {
                if let Some(id) = self.recent.pop_front() {
                    self.seen.remove(&id);
                }
            }
        }
        let d = &event.data;
        let Some(session) = event.session_id() else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        match event.kind.as_str() {
            "session.execution.started" => {
                if self.running.insert(session.into()) {
                    self.content
                        .retain(|(s, _, _, _), (_, ended)| s != session || !*ended);
                    self.tools
                        .retain(|(s, _, _), tool| s != session || !tool.ended);
                }
                out.push(Update::Started);
            }
            "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted" => {
                if self.running.remove(session) {
                    self.settled.insert(session.into());
                    out.push(Update::Terminal {
                        outcome: event.kind.rsplit('.').next().unwrap().into(),
                        error: d.get("error").cloned(),
                    });
                }
            }
            "session.step.started" => {
                self.models.insert(session.into(), d["model"].clone());
            }
            "session.step.ended" | "session.step.failed" => {
                if d.get("tokens").is_some() {
                    out.push(Update::Usage(json!({"id": required_str(d,"assistantMessageID")?, "sessionID":session,"role":"assistant", "tokens":d["tokens"],"cost":d["cost"],"providerID":self.models.get(session).and_then(|m|m.get("providerID")),"modelID":self.models.get(session).and_then(|m|m.get("id"))})));
                }
            }
            kind if kind.starts_with("session.text.") || kind.starts_with("session.reasoning.") => {
                let thinking = kind.starts_with("session.reasoning.");
                let message = required_str(d, "assistantMessageID")?;
                let ordinal = d["ordinal"].as_u64().ok_or("missing content ordinal")?;
                let content = self
                    .content
                    .entry((session.into(), message.into(), thinking, ordinal))
                    .or_default();
                if content.1 {
                    return Ok(vec![]);
                }
                if kind.ends_with(".delta") {
                    let text = required_str(d, "delta")?;
                    content.0.push_str(text);
                    out.push(Update::Content {
                        thinking,
                        complete: false,
                        text: text.into(),
                    });
                } else if kind.ends_with(".snapshot") {
                    let full = required_str(d, "text")?;
                    let missing = full
                        .strip_prefix(&content.0)
                        .ok_or("native content snapshot is not an append of the streamed value")?;
                    if !missing.is_empty() {
                        out.push(Update::Content {
                            thinking,
                            complete: false,
                            text: missing.into(),
                        });
                    }
                    content.0 = full.into();
                } else if kind.ends_with(".ended") {
                    content.0 = required_str(d, "text")?.into();
                    content.1 = true;
                    out.push(Update::Content {
                        thinking,
                        complete: true,
                        text: content.0.clone(),
                    });
                }
            }
            kind if kind.starts_with("session.tool.") => {
                let message = required_str(d, "assistantMessageID")?;
                let id = required_str(d, "id")?;
                let tool = self
                    .tools
                    .entry((session.into(), message.into(), id.into()))
                    .or_default();
                if tool.ended {
                    return Ok(vec![]);
                }
                match kind {
                    "session.tool.input.started" => {
                        tool.name = required_str(d, "name")?.into();
                    }
                    "session.tool.input.delta" => {
                        tool.raw.push_str(required_str(d, "delta")?);
                    }
                    "session.tool.input.ended" => {
                        tool.raw = required_str(d, "text")?.into();
                        tool.input = serde_json::from_str(&tool.raw).unwrap_or(Value::Null);
                    }
                    "session.tool.called" => {
                        tool.input = d["input"].clone();
                        if !tool.called {
                            tool.called = true;
                            out.push(Update::ToolUse {
                                id: id.into(),
                                name: tool.name.clone(),
                                input: tool.input.clone(),
                            });
                        }
                    }
                    "session.tool.progress" => {
                        tool.metadata = d["metadata"].clone();
                    }
                    "session.tool.success" | "session.tool.failed" => {
                        if !tool.called {
                            out.push(Update::ToolUse {
                                id: id.into(),
                                name: tool.name.clone(),
                                input: tool.input.clone(),
                            });
                            tool.called = true;
                        }
                        tool.ended = true;
                        out.push(Update::ToolResult {
                            id: id.into(),
                            content: d
                                .get("content")
                                .filter(|v| !v.is_null() || kind == "session.tool.success")
                                .cloned()
                                .unwrap_or_else(
                                    || json!([{"type":"text","text":d["error"]["message"]}]),
                                ),
                            error: kind.ends_with(".failed"),
                        });
                    }
                    _ => {}
                }
                // Tool completion is not child completion: background subagents
                // can outlive both this call and the parent's execution.
                if tool.name == "subagent" {
                    let metadata = d.get("metadata").unwrap_or(&tool.metadata);
                    if let Some(child) = metadata.get("sessionID").and_then(Value::as_str) {
                        out.push(Update::Child {
                            id: child.into(),
                            status: metadata["status"].as_str().unwrap_or("running").into(),
                            detail: json!({"id":id,"input":tool.input,"metadata":metadata}),
                        });
                    }
                }
            }
            "permission.asked" => out.push(Update::Permission(d.clone())),
            "permission.replied" => out.push(Update::Resolved {
                id: required_str(d, "requestID")?.into(),
                answer: Some(d["reply"].clone()),
            }),
            "form.created" => out.push(Update::Form(d["form"].clone())),
            "form.replied" | "form.cancelled" => out.push(Update::Resolved {
                id: required_str(d, "id")?.into(),
                answer: d.get("answer").cloned(),
            }),
            "session.compaction.ended" => out.push(Update::Compacted),
            // Busy/idle, step completion, and deprecated session.idle never
            // settle the user turn. Execution terminals are authoritative.
            _ => {}
        }
        Ok(out)
    }
}

fn required_str<'a>(data: &'a Value, key: &str) -> Result<&'a str, String> {
    data.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing native v2 {key}"))
}

pub fn model_ref(model: &str, variant: Option<&str>) -> Result<Value, String> {
    let (provider, id) = model
        .split_once('/')
        .filter(|(p, m)| !p.is_empty() && !m.is_empty())
        .ok_or("invalid_model_id: expected providerID/modelID")?;
    let mut model = json!({"providerID":provider,"id":id});
    if let Some(variant) = variant.filter(|v| !v.is_empty()) {
        model["variant"] = json!(variant);
    }
    Ok(model)
}
pub fn create_body(
    directory: &str,
    model: Option<&str>,
    variant: Option<&str>,
) -> Result<Value, String> {
    let mut body = json!({"location":{"directory":directory}});
    if let Some(model) = model {
        body["model"] = model_ref(model, variant)?;
    }
    Ok(body)
}
pub fn prompt_body(text: &str, files: Vec<Value>, steer: bool) -> Value {
    let mut body = json!({"text":text,"delivery":if steer {"steer"} else {"queue"},"resume":true});
    if !files.is_empty() {
        body["files"] = json!(files);
    }
    body
}
/// Only native question-tool forms fit the existing question UI. Do not
/// project auth, MCP-global, hidden, or conditional forms into chat inputs.
pub fn question_payload(form: &Value) -> Result<Value, String> {
    if form["metadata"]["kind"] != "question" || form["sessionID"] == "global" {
        return Err(
            "unsupported OpenCode form (only ordinary question-tool forms are supported)".into(),
        );
    }
    let fields = form["fields"]
        .as_array()
        .filter(|f| !f.is_empty())
        .ok_or("empty question form")?;
    let mut questions = Vec::new();
    for field in fields {
        let kind = required_str(field, "type")?;
        if !matches!(kind, "string" | "multiselect")
            || field["hidden"] == true
            || field.get("when").is_some()
        {
            return Err("unsupported question field".into());
        }
        let question = field
            .get("description")
            .and_then(Value::as_str)
            .or_else(|| field.get("title").and_then(Value::as_str))
            .unwrap_or(required_str(field, "key")?);
        // The current UI returns labels, so accepting differing option values
        // would silently answer with the wrong token.
        let options = field
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if options.iter().any(|o| o["value"] != o["label"]) {
            return Err("question option label/value mismatch".into());
        }
        questions.push(json!({"id":field["key"],"question":question,"header":field["title"].as_str().unwrap_or(""),"multiSelect":kind=="multiselect","allowOther":field["custom"]==true,"options":options}));
    }
    // The UI keys answers by question text. Preserve exact native keys in the
    // outbound map and deny an unrepresentable duplicate instead of guessing.
    let unique: HashSet<_> = questions
        .iter()
        .map(|q| q["question"].as_str().unwrap())
        .collect();
    if unique.len() != questions.len() {
        return Err("duplicate question text is not representable by the question UI".into());
    }
    Ok(json!({"questions":questions}))
}

pub fn question_answer(form: &Value, input: &Value) -> Result<Value, String> {
    let payload = question_payload(form)?;
    let answers = input
        .get("answers")
        .unwrap_or(input)
        .as_object()
        .ok_or("missing question answers")?;
    let mut answer = serde_json::Map::new();
    for (field, question) in form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .zip(payload["questions"].as_array().unwrap())
    {
        let key = required_str(field, "key")?;
        let value = answers
            .get(key)
            .or_else(|| answers.get(question["question"].as_str().unwrap()))
            .ok_or_else(|| format!("missing answer for {key}"))?;
        let options = field
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let values: Vec<String> = if field["type"] == "multiselect" {
            if let Some(array) = value.as_array() {
                array
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| "expected string selection".into())
                    })
                    .collect::<Result<_, String>>()?
            } else {
                let text = value.as_str().ok_or("expected multiselect answer")?;
                if options.iter().any(|o| o["value"] == text) {
                    vec![text.into()]
                } else {
                    text.split(", ")
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                }
            }
        } else {
            vec![value.as_str().ok_or("expected string answer")?.into()]
        };
        if values.is_empty() || values.iter().any(|v| v.is_empty()) {
            return Err(format!("missing answer for {key}"));
        }
        if field["custom"] != true
            && values
                .iter()
                .any(|v| !options.iter().any(|o| o["value"] == v.as_str()))
        {
            return Err(format!("invalid option for {key}"));
        }
        answer.insert(
            key.into(),
            if field["type"] == "multiselect" {
                json!(values)
            } else {
                json!(values[0])
            },
        );
    }
    Ok(Value::Object(answer))
}

/// Normalize the history boundary only; live native events use State. Native
/// messages omit sessionID, so the client must install the requested session
/// ID on info/parts after decoding a page (never infer it from process cwd).
pub fn decode_session_messages(
    value: Value,
) -> Result<Vec<super::protocol::SessionMessage>, String> {
    let messages = value["data"]
        .as_array()
        .ok_or("missing native v2 message data array")?;
    let mut out = Vec::new();
    for message in messages {
        let kind = required_str(message, "type")?;
        if !matches!(kind, "user" | "assistant") {
            continue;
        }
        let id = required_str(message, "id")?;
        let session = message["sessionID"].as_str().unwrap_or("");
        let error = message
            .get("error")
            .map(|e| json!({"name":e["type"],"data":{"message":e["message"]}}));
        let info = json!({"id":id,"sessionID":session,"role":kind,"time":message["time"],"tokens":message["tokens"],"error":error,"providerID":message["model"]["providerID"],"modelID":message["model"]["id"]});
        let mut parts = Vec::new();
        let content = if kind == "user" {
            vec![json!({"type":"text","text":message["text"]})]
        } else {
            message["content"]
                .as_array()
                .ok_or("missing assistant content")?
                .clone()
        };
        for (ordinal, item) in content.iter().enumerate() {
            let mut part = json!({"id":format!("{id}:{ordinal}"),"sessionID":session,"messageID":id,"type":item["type"],"text":item["text"]});
            if item["type"] == "tool" {
                let state = &item["state"];
                let status = state["status"].as_str().ok_or("missing tool state")?;
                let projected_status = if status == "streaming" {
                    "pending"
                } else {
                    status
                };
                let output = state["content"]
                    .as_array()
                    .map(|c| {
                        c.iter()
                            .filter_map(|c| c["text"].as_str().or_else(|| c["uri"].as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                part["callID"] = item["id"].clone();
                part["tool"] = item["name"].clone();
                part["state"] = json!({"status":projected_status,"input":state["input"],"metadata":state["metadata"],"output":output,"error":state["error"]["message"].as_str().unwrap_or(""),"time":{"start":item["time"]["ran"],"end":item["time"]["completed"]}});
            }
            parts.push(part);
        }
        out.push(
            serde_json::from_value(json!({"info":info,"parts":parts}))
                .map_err(|e| format!("native message decode failed: {e}"))?,
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(id: &str, kind: &str, data: Value) -> Event {
        Event {
            id: id.into(),
            kind: kind.into(),
            data,
        }
    }
    #[test]
    fn reconnect_snapshot_appends_only_missing_partial_content_then_accepts_live_deltas() {
        let mut state = State::default();
        let base = json!({"sessionID":"s","assistantMessageID":"m","ordinal":0});
        let mut d = base.clone();
        d["delta"] = json!("hel");
        state
            .reduce(&event("first", "session.text.delta", d))
            .unwrap();
        let mut d = base.clone();
        d["text"] = json!("hello");
        assert!(
            matches!(&state.reduce(&event("snapshot:content:m:0","session.text.snapshot",d)).unwrap()[..],[Update::Content{complete:false,text,..}] if text=="lo")
        );
        let mut d = base.clone();
        d["delta"] = json!(" world");
        assert!(
            matches!(&state.reduce(&event("live","session.text.delta",d)).unwrap()[..],[Update::Content{complete:false,text,..}] if text==" world")
        );
        let mut d = base;
        d["text"] = json!("hello world!");
        assert!(
            matches!(&state.reduce(&event("snapshot:content:m:0","session.text.snapshot",d)).unwrap()[..],[Update::Content{complete:false,text,..}] if text=="!")
        );
    }

    #[test]
    fn question_roundtrip_preserves_native_keys_and_rejects_general_forms() {
        let form = json!({"id":"f","sessionID":"child","metadata":{"kind":"question"},"fields":[
          {"key":"q0","type":"string","title":"Storage","description":"Which storage?","custom":true,"options":[{"value":"A","label":"A"}]},
          {"key":"q1","type":"multiselect","description":"Features?","custom":true,"options":[{"value":"with, comma","label":"with, comma"},{"value":"B","label":"B"}]}]});
        let payload = question_payload(&form).unwrap();
        assert_eq!(payload["questions"][0]["question"], "Which storage?");
        assert_eq!(
            question_answer(
                &form,
                &json!({"answers":{"Which storage?":"custom","Features?":"with, comma"}})
            )
            .unwrap(),
            json!({"q0":"custom","q1":["with, comma"]})
        );
        assert!(
            question_payload(&json!({"metadata":{"kind":"mcp-elicitation"},"fields":[]})).is_err()
        );
        assert!(question_answer(&form, &json!({"answers":{}})).is_err());
    }

    #[test]
    fn history_page_decodes_native_user_assistant_and_tool_without_v1_defaults() {
        let page = json!({"data":[{"id":"u","type":"user","text":"hi","time":{"created":1}},
          {"id":"a","type":"assistant","agent":"build","model":{"providerID":"p","id":"m"},"time":{"created":2,"completed":3},"tokens":{"input":4,"output":2},"content":[{"type":"reasoning","text":"thought"},{"type":"text","text":"answer"},{"type":"tool","id":"t","name":"read","time":{"created":2,"completed":3},"state":{"status":"completed","input":{"path":"x"},"content":[{"type":"text","text":"ok"}]}}]},
          {"id":"i","type":"idle","outcome":"succeeded"}],"cursor":{"next":"opaque"}});
        let messages = decode_session_messages(page).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1].info.qualified_model_id().as_deref(),
            Some("p/m")
        );
        assert!(
            matches!(&messages[1].parts[0],super::super::protocol::PartPayload::Reasoning(p) if p.text=="thought")
        );
        assert!(
            matches!(&messages[1].parts[2],super::super::protocol::PartPayload::Tool(p) if p.call_id=="t")
        );
        assert!(decode_session_messages(json!({"wrong":[]})).is_err());
    }

    #[test]
    fn create_and_durable_prompt_use_native_fields_and_opaque_model_variant() {
        assert_eq!(
            model_ref("p/model/slash", Some("high")).unwrap(),
            json!({"providerID":"p","id":"model/slash","variant":"high"})
        );
        assert_eq!(
            create_body("/project", Some("p/m"), Some("high")).unwrap(),
            json!({"location":{"directory":"/project"},"model":{"providerID":"p","id":"m","variant":"high"}})
        );
        let prompt = prompt_body(
            "hi",
            vec![json!({"uri":"data:image/png;base64,YQ=="})],
            false,
        );
        assert_eq!(
            prompt,
            json!({"text":"hi","files":[{"uri":"data:image/png;base64,YQ=="}],"delivery":"queue","resume":true})
        );
        assert_eq!(prompt_body("steer", vec![], true)["delivery"], "steer");
        assert!(model_ref("invalid", None).is_err());
    }

    #[test]
    fn native_turn_stream_keeps_thinking_tools_usage_and_one_execution_terminal() {
        let mut state = State::default();
        let data = json!({"sessionID":"s", "assistantMessageID":"m", "ordinal":0});
        state
            .reduce(&event("1", "session.execution.started", data.clone()))
            .unwrap();
        let mut delta = data.clone();
        delta["delta"] = json!("thinking");
        let updates = state
            .reduce(&event("2", "session.reasoning.delta", delta))
            .unwrap();
        assert!(
            matches!(&updates[..], [Update::Content{thinking:true,complete:false,text}] if text == "thinking")
        );
        let mut ended = data.clone();
        ended["text"] = json!("thinking done");
        let updates = state
            .reduce(&event("3", "session.reasoning.ended", ended.clone()))
            .unwrap();
        assert!(
            matches!(&updates[..], [Update::Content{thinking:true,complete:true,text}] if text == "thinking done")
        );
        assert!(state
            .reduce(&event("4", "session.reasoning.ended", ended))
            .unwrap()
            .is_empty());
        let tool = json!({"sessionID":"s","assistantMessageID":"m","id":"call","name":"read"});
        state
            .reduce(&event("5", "session.tool.input.started", tool))
            .unwrap();
        let called = json!({"sessionID":"s","assistantMessageID":"m","id":"call","input":{"path":"x"},"executed":true});
        assert!(
            matches!(&state.reduce(&event("6","session.tool.called",called)).unwrap()[..], [Update::ToolUse{name,input,..}] if name=="read" && input["path"]=="x")
        );
        let result = json!({"sessionID":"s","assistantMessageID":"m","id":"call","content":[{"type":"text","text":"ok"},{"type":"file","uri":"file:///x","mime":"text/plain"}],"executed":true});
        assert!(
            matches!(&state.reduce(&event("7","session.tool.success",result.clone())).unwrap()[..], [Update::ToolResult{content,error:false,..}] if content.as_array().unwrap().len()==2)
        );
        assert!(state
            .reduce(&event("8", "session.tool.success", result))
            .unwrap()
            .is_empty());
        let usage = json!({"sessionID":"s","assistantMessageID":"m","tokens":{"input":10,"output":3,"reasoning":2,"cache":{"read":1,"write":0}},"cost":0.02});
        assert!(matches!(
            &state
                .reduce(&event("9", "session.step.ended", usage))
                .unwrap()[..],
            [Update::Usage(_)]
        ));
        assert!(state
            .reduce(&event("10", "session.idle", data.clone()))
            .unwrap()
            .is_empty());
        assert!(
            matches!(&state.reduce(&event("11","session.execution.succeeded",data.clone())).unwrap()[..], [Update::Terminal{outcome,..}] if outcome=="succeeded")
        );
        assert!(state
            .reduce(&event("12", "session.execution.succeeded", data))
            .unwrap()
            .is_empty());
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
}

impl Event {
    pub fn session_id(&self) -> Option<&str> {
        if self.kind.starts_with("form.") && self.kind == "form.created" {
            self.data.get("form")?.get("sessionID")?.as_str()
        } else {
            self.data.get("sessionID")?.as_str()
        }
    }
}
