//! `codemux-remote mcp` — stdio MCP server.
//!
//! Reads JSON-RPC 2.0 requests on stdin, writes responses on stdout.
//! The MCP protocol surface implemented here is the minimum the
//! Claude Code / Codex / Gemini CLIs need to discover and call our
//! tools:
//!
//! - `initialize` → returns server info + capabilities.
//! - `notifications/initialized` → no-op acknowledgement.
//! - `tools/list` → forwards to the daemon's `/tools/list`.
//! - `tools/call` → forwards to the daemon's `/tools/call`, wraps
//!                  the response in MCP's `content: [{type:"text"}]`
//!                  shape that all CLI agents expect.
//! - `ping` → returns empty result. Some clients sanity-check this.
//!
//! Everything else returns `-32601 method not found`. That's enough
//! for the CLI agents we care about; extending to resources/prompts
//! later is purely additive.
//!
//! The server reads the local manifest on boot to find the daemon's
//! endpoint + secret. If the daemon isn't running, we exit with a
//! clean error so the agent gets a meaningful message instead of a
//! cryptic stdio close.

use std::io::{BufRead, BufReader, Write};

use serde_json::{json, Value};

use super::manifest;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "codemux-remote";

pub fn run_stdio(state_dir: std::path::PathBuf) -> Result<(), String> {
    let manifest_path = super::config::manifest_path(&state_dir);
    let manifest = manifest::read(&manifest_path)
        .map_err(|e| format!("read manifest at {}: {e}", manifest_path.display()))?
        .ok_or_else(|| {
            format!(
                "no daemon manifest at {}. \
                 Start the daemon first with: codemux-remote serve",
                manifest_path.display()
            )
        })?;

    if !manifest::pid_alive(manifest.pid) {
        return Err(format!(
            "manifest at {} points to pid {} which is not running. \
             Start the daemon with: codemux-remote serve",
            manifest_path.display(),
            manifest.pid
        ));
    }

    // Blocking HTTP client — MCP stdio is request/response over a
    // single client, so the async runtime would be wasted ceremony
    // here. reqwest::blocking is already in deps.
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;

    let mut stdin = BufReader::new(std::io::stdin().lock());
    let stdout = std::io::stdout();
    let mut stdout_locked = stdout.lock();

    let mut line = String::new();
    loop {
        line.clear();
        let n = match stdin.read_line(&mut line) {
            Ok(0) => return Ok(()), // EOF — client disconnected, normal exit
            Ok(n) => n,
            Err(e) => return Err(format!("stdin read: {e}")),
        };
        if line.trim().is_empty() {
            continue;
        }
        let _ = n; // silence

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                // We can't reply with an id we don't know; per JSON-RPC
                // 2.0, send a Parse error with null id.
                write_response(
                    &mut stdout_locked,
                    error_response(Value::Null, -32700, &format!("parse error: {e}")),
                )?;
                continue;
            }
        };
        let response = handle_request(&request, &http, &manifest);
        // Notifications (no id) get no response.
        if let Some(resp) = response {
            write_response(&mut stdout_locked, resp)?;
        }
    }
}

/// Dispatch a single JSON-RPC request. Returns `None` for
/// notifications (no id), `Some(response)` otherwise.
fn handle_request(
    request: &Value,
    http: &reqwest::blocking::Client,
    manifest: &manifest::Manifest,
) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    // Notifications don't have an id; their handlers can't return
    // anything. We still pattern-match them so unknown notifications
    // don't bubble up as errors.
    let is_notification = id.is_none();

    let result_or_err: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(initialize_result()),
        "ping" => Ok(json!({})),
        "tools/list" => forward_list_tools(http, manifest),
        "tools/call" => forward_call_tool(&params, http, manifest),
        "notifications/initialized" | "notifications/cancelled" => {
            if is_notification {
                return None;
            }
            Ok(json!({}))
        }
        other => Err((-32601, format!("method not found: {other}"))),
    };

    if is_notification {
        return None;
    }

    Some(match result_or_err {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err((code, message)) => error_response(id.unwrap_or(Value::Null), code, &message),
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": { "listChanged": false }
        }
    })
}

fn forward_list_tools(
    http: &reqwest::blocking::Client,
    manifest: &manifest::Manifest,
) -> Result<Value, (i64, String)> {
    let url = format!("{}/tools/list", manifest.endpoint);
    let res = http
        .get(&url)
        .bearer_auth(&manifest.secret)
        .send()
        .map_err(|e| (-32000, format!("daemon /tools/list: {e}")))?;
    if !res.status().is_success() {
        return Err((
            -32000,
            format!("daemon /tools/list returned HTTP {}", res.status()),
        ));
    }
    let body: Value = res.json().map_err(|e| (-32000, format!("decode tools/list: {e}")))?;
    // Translate the catalog into MCP's expected shape: a top-level
    // "tools" array of { name, description, inputSchema }.
    let tools = body
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mapped: Vec<Value> = tools
        .into_iter()
        .map(|t| {
            json!({
                "name": t.get("name").cloned().unwrap_or(Value::Null),
                "description": t.get("description").cloned().unwrap_or(Value::Null),
                "inputSchema": t.get("input_schema").cloned().unwrap_or(json!({})),
            })
        })
        .collect();
    Ok(json!({ "tools": mapped }))
}

fn forward_call_tool(
    params: &Value,
    http: &reqwest::blocking::Client,
    manifest: &manifest::Manifest,
) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or((-32602, "missing tools/call.name".to_string()))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    let url = format!("{}/tools/call", manifest.endpoint);
    let res = http
        .post(&url)
        .bearer_auth(&manifest.secret)
        .json(&json!({ "name": name, "arguments": arguments }))
        .send()
        .map_err(|e| (-32000, format!("daemon /tools/call: {e}")))?;
    let status = res.status();
    let body: Value = res
        .json()
        .map_err(|e| (-32000, format!("decode tools/call: {e}")))?;

    // Map back into MCP's response shape:
    //   { content: [{type:"text", text:"<json>"}], isError: bool }
    // Agents render the text. Including the structured payload as
    // JSON inside the text field keeps the door open for richer
    // content (resources, images) without breaking the wire format.
    let is_error = !status.is_success() || body.get("ok") == Some(&json!(false));
    let payload = if is_error {
        body.get("error")
            .cloned()
            .unwrap_or_else(|| json!({ "kind": "internal", "message": "daemon error" }))
    } else {
        body.get("data").cloned().unwrap_or(json!({}))
    };
    let text = serde_json::to_string_pretty(&payload)
        .unwrap_or_else(|_| payload.to_string());

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    }))
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn write_response<W: Write>(w: &mut W, response: Value) -> Result<(), String> {
    let line = serde_json::to_string(&response)
        .map_err(|e| format!("serialise response: {e}"))?;
    writeln!(w, "{line}").map_err(|e| format!("stdout write: {e}"))?;
    w.flush().map_err(|e| format!("stdout flush: {e}"))
}
