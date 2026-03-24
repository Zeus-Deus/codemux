use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        
        result.push(CHARS[b0 >> 2] as char);
        result.push(CHARS[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        
        if chunk.len() > 1 {
            result.push(CHARS[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        } else {
            result.push('=');
        }
        
        if chunk.len() > 2 {
            result.push(CHARS[b2 & 0x3f] as char);
        } else {
            result.push('=');
        }
    }
    
    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAutomationResult {
    pub request_id: String,
    pub browser_id: String,
    pub data: serde_json::Value,
    pub message: Option<String>,
}

pub struct AgentBrowserManager {
    pub running: Arc<Mutex<bool>>,
    pub stream_port: u16,
}

fn session_name(browser_id: &str) -> &str {
    if browser_id.is_empty() { "default" } else { browser_id }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn build_agent_browser_command(session: &str, action: &str, params: &serde_json::Value) -> Result<String, String> {
    let command = match action {
        "open_url" | "open" => {
            let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("about:blank");
            format!("npx agent-browser open {} --session {}", shell_quote(url), session)
        }
        "screenshot" => format!("npx agent-browser screenshot --session {}", session),
        "snapshot" | "accessibility_snapshot" => {
            format!("npx agent-browser snapshot -i --session {}", session)
        }
        "click" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("npx agent-browser click {} --session {}", shell_quote(selector), session)
        }
        "fill" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
            format!(
                "npx agent-browser fill {} {} --session {}",
                shell_quote(selector),
                shell_quote(value),
                session
            )
        }
        "type_text" => {
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            format!("npx agent-browser type body {} --session {}", shell_quote(text), session)
        }
        "console_logs" | "console" => format!("npx agent-browser console --session {}", session),
        "evaluate" | "eval" => {
            let script = params.get("script").and_then(|v| v.as_str()).unwrap_or("");
            format!("npx agent-browser eval {} --session {}", shell_quote(script), session)
        }
        "back" => format!("npx agent-browser back --session {}", session),
        "forward" => format!("npx agent-browser forward --session {}", session),
        "reload" => format!("npx agent-browser reload --session {}", session),
        _ => return Err(format!("Unknown action: {}", action)),
    };

    Ok(command)
}

fn execute_agent_browser_action(browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
    let session = session_name(browser_id);
    let shell_cmd = build_agent_browser_command(session, action, &params)?;
    let output = std::process::Command::new("sh")
        .args(["-c", &shell_cmd])
        .output()
        .map_err(|error| format!("Failed to run agent-browser: {}", error))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && !stdout.contains("✓") && !stdout.contains("{") && !stdout.contains("- ") {
        return Err(format!("agent-browser failed: {} {}", stdout, stderr));
    }

    let data: serde_json::Value = if stdout.contains("{") {
        serde_json::from_str(&stdout).unwrap_or(serde_json::json!({ "raw": stdout }))
    } else if action == "screenshot" {
        if stdout.trim().is_empty() {
            serde_json::json!({ "error": "No screenshot" })
        } else {
            serde_json::json!({ "raw": stdout })
        }
    } else if action == "snapshot" || action == "accessibility_snapshot" {
        serde_json::json!({ "tree": stdout })
    } else {
        serde_json::json!({ "result": stdout, "success": output.status.success() })
    };

    Ok(BrowserAutomationResult {
        request_id: format!(
            "req-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ),
        browser_id: browser_id.to_string(),
        data,
        message: None,
    })
}

pub fn run_cli_action(browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
    execute_agent_browser_action(browser_id, action, params)
}

impl Default for AgentBrowserManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBrowserManager {
    pub fn new() -> Self {
        Self {
            running: Arc::new(Mutex::new(false)),
            stream_port: 9223,
        }
    }

    pub async fn spawn(&self, browser_id: &str) -> Result<(), String> {
        let session = session_name(browser_id);
        
        let mut running = self.running.lock().await;
        if *running {
            return Ok(());
        }

        let output = std::process::Command::new("sh")
            .args(["-c", &format!("npx agent-browser open about:blank --headless --session {}", session)])
            .output()
            .map_err(|e| format!("Failed to start agent-browser: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to start browser: {}", stderr));
        }

        std::thread::sleep(std::time::Duration::from_millis(2000));
        *running = true;
        Ok(())
    }

    pub async fn run_command(&self, browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
        execute_agent_browser_action(browser_id, action, params)
    }

    pub async fn get_screenshot(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id);
        
        let output = std::process::Command::new("sh")
            .args(["-c", &format!("npx agent-browser screenshot --session {}", session)])
            .output()
            .map_err(|e| format!("Failed to get screenshot: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        
        // Parse the screenshot path from output like "Screenshot saved to /path/to/file.png"
        // Strip ANSI codes (escape sequences like \x1b[32m)
        let mut clean = String::new();
        let mut in_ansi = false;
        for c in stdout.chars() {
            if c == '\x1b' {
                in_ansi = true;
            } else if in_ansi && c == 'm' {
                in_ansi = false;
            } else if !in_ansi {
                clean.push(c);
            }
        }
        
        if let Some(path_start) = clean.find("Screenshot saved to ") {
            let path = clean[path_start + 19..].trim();
            
            // Read the file and convert to base64
            if let Ok(data) = std::fs::read(path) {
                let base64 = base64_encode(&data);
                return Ok(format!("data:image/png;base64,{}", base64));
            }
        }
        
        Ok(clean)
    }

    pub async fn close(&self, browser_id: &str) -> Result<(), String> {
        let session = session_name(browser_id);
        
        let mut running = self.running.lock().await;
        *running = false;
        
        let _ = std::process::Command::new("sh")
            .args(["-c", &format!("npx agent-browser close --session {}", session)])
            .output();
        Ok(())
    }

    pub fn get_stream_url(&self) -> String {
        format!("ws://localhost:{}", self.stream_port)
    }

    pub async fn start_stream(&self, _browser_id: &str) -> Result<String, String> {
        let port = self.stream_port;

        let mut running = self.running.lock().await;
        if *running {
            return Ok(format!("ws://localhost:{}", port));
        }

        // Start agent-browser with stream server via env var.
        // The daemon auto-starts the WebSocket stream server when
        // AGENT_BROWSER_STREAM_PORT is set.
        std::process::Command::new("sh")
            .args(["-c", "npx agent-browser open about:blank --headless"])
            .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start browser stream: {}", e))?;

        // Wait for daemon + stream server to be ready
        std::thread::sleep(std::time::Duration::from_millis(4000));
        *running = true;

        Ok(format!("ws://localhost:{}", port))
    }
}
