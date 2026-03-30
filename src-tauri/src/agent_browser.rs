use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Stealth Chromium flags that reduce bot detection fingerprinting.
/// Passed via the AGENT_BROWSER_ARGS env var (comma-separated).
const STEALTH_CHROMIUM_ARGS: &str = "\
--disable-blink-features=AutomationControlled,\
--disable-features=AutomationControlled,\
--disable-infobars,\
--no-first-run,\
--no-default-browser-check,\
--disable-background-timer-throttling,\
--disable-backgrounding-occluded-windows,\
--disable-renderer-backgrounding,\
--disable-component-update,\
--disable-hang-monitor,\
--disable-prompt-on-repost,\
--metrics-recording-only,\
--password-store=basic";

/// Detect installed Chrome/Chromium version and return a realistic user-agent string.
fn stealth_user_agent() -> String {
    let candidates = ["chromium", "chromium-browser", "google-chrome-stable", "google-chrome"];
    for bin in candidates {
        if let Ok(output) = std::process::Command::new(bin).arg("--version").output() {
            if output.status.success() {
                let version_str = String::from_utf8_lossy(&output.stdout);
                // Parse version like "Chromium 131.0.6778.204" or "Google Chrome 131.0.6778.204"
                if let Some(ver) = version_str.split_whitespace().last() {
                    return format!(
                        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{} Safari/537.36",
                        ver.trim()
                    );
                }
            }
        }
    }
    // Fallback to a reasonable default
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string()
}

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

/// Default stream port for the browser screencast WebSocket.
pub const DEFAULT_STREAM_PORT: u16 = 9223;

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

/// JavaScript that queries the DOM for all interactive elements.
/// Used as a fallback when the ARIA snapshot returns nothing useful.
pub const DOM_SNAPSHOT_SCRIPT: &str = r###"(() => {
  const sel = "a[href], button, input, select, textarea, [role='button'], [role='link'], [role='tab'], [role='checkbox'], [role='radio'], [role='combobox'], [role='menuitem'], [role='searchbox'], [tabindex]:not([tabindex='-1'])";
  const els = document.querySelectorAll(sel);
  const results = [];
  const seen = new Set();
  for (const el of els) {
    if (el.offsetParent === null && el.tagName !== "INPUT" && el.getAttribute("type") !== "hidden") continue;
    try { if (getComputedStyle(el).display === "none" || getComputedStyle(el).visibility === "hidden") continue; } catch(e) { continue; }
    const tag = el.tagName.toLowerCase();
    const role = el.getAttribute("role") || tag;
    const text = (el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || el.getAttribute("value") || el.getAttribute("title") || "").substring(0, 80).trim().replace(/\s+/g, " ");
    let selector = "";
    if (el.id) selector = "#" + el.id;
    else if (el.getAttribute("name")) selector = tag + "[name='" + el.getAttribute("name") + "']";
    else if (el.getAttribute("aria-label")) selector = tag + "[aria-label='" + el.getAttribute("aria-label") + "']";
    else if (tag === "a" && el.getAttribute("href")) {
      const href = el.getAttribute("href");
      if (href.length < 60) selector = "a[href='" + href + "']";
    }
    if (!selector) {
      let cur = el; const parts = [];
      while (cur && cur !== document.body && cur !== document.documentElement) {
        let seg = cur.tagName.toLowerCase();
        const parent = cur.parentElement;
        if (parent) {
          const siblings = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
          if (siblings.length > 1) seg += ":nth-of-type(" + (siblings.indexOf(cur) + 1) + ")";
        }
        parts.unshift(seg);
        cur = parent;
        if (parts.length > 4) break;
      }
      selector = parts.join(" > ");
    }
    const key = role + "|" + text + "|" + selector;
    if (seen.has(key)) continue;
    seen.add(key);
    const entry = text ? "- [" + role + "] \"" + text + "\" \u2192 " + selector : "- [" + role + "] " + selector;
    results.push(entry);
  }
  return results.length > 0 ? results.join("\n") : "(no elements found)";
})()"###;

fn needs_dom_fallback(stdout: &str) -> bool {
    let trimmed = stdout.trim();
    trimmed.contains("(no interactive elements)")
        || trimmed == "- document"
        || trimmed.is_empty()
        || trimmed == "(empty)"
}

fn extract_eval_result(stdout: &str) -> String {
    // agent-browser eval returns JSON: {"success":true,"data":{"result":"...","origin":"..."}}
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(stdout) {
        if let Some(result) = json.pointer("/data/result").and_then(|v| v.as_str()) {
            return result.to_string();
        }
    }
    // Native binary may wrap eval string results in quotes — strip them
    let trimmed = stdout.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() > 1 {
        // Unescape the JSON string
        if let Ok(serde_json::Value::String(s)) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return s;
        }
    }
    trimmed.to_string()
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
        "viewport" => {
            let w = params.get("width").and_then(|v| v.as_u64()).unwrap_or(1280);
            let h = params.get("height").and_then(|v| v.as_u64()).unwrap_or(720);
            format!("npx agent-browser viewport {} {} --session {}", w, h, session)
        }
        _ => return Err(format!("Unknown action: {}", action)),
    };

    Ok(command)
}

fn make_request_id() -> String {
    format!(
        "req-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    )
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

    // Debug logging for snapshot commands
    if action == "snapshot" || action == "accessibility_snapshot" {
        eprintln!("[codemux::browser] Snapshot stdout ({} bytes): {}", stdout.len(), &stdout[..stdout.len().min(200)]);
        if !stderr.is_empty() {
            eprintln!("[codemux::browser] Snapshot stderr: {}", &stderr[..stderr.len().min(200)]);
        }
    }

    if !output.status.success() && !stdout.contains("✓") && !stdout.contains("{") && !stdout.contains("- ") {
        return Err(format!("agent-browser failed: {} {}", stdout, stderr));
    }

    // For snapshot: detect useless ARIA result and fall back to DOM-based query
    if action == "snapshot" || action == "accessibility_snapshot" {
        eprintln!("[codemux::browser] Snapshot raw stdout for fallback check: {:?}", &stdout[..stdout.len().min(300)]);
    }
    if (action == "snapshot" || action == "accessibility_snapshot") && needs_dom_fallback(&stdout) {
        eprintln!("[codemux::browser] ARIA snapshot empty, falling back to DOM query");
        let dom_params = serde_json::json!({ "script": DOM_SNAPSHOT_SCRIPT });
        let dom_cmd = build_agent_browser_command(session, "eval", &dom_params)?;
        if let Ok(dom_output) = std::process::Command::new("sh").args(["-c", &dom_cmd]).output() {
            let dom_stdout = String::from_utf8_lossy(&dom_output.stdout).to_string();
            let dom_tree = extract_eval_result(&dom_stdout);
            if !dom_tree.is_empty() && dom_tree != "(no elements found)" {
                let combined = format!(
                    "{}\n\n--- Interactive Elements (DOM) ---\n{}",
                    stdout.trim(),
                    dom_tree
                );
                return Ok(BrowserAutomationResult {
                    request_id: make_request_id(),
                    browser_id: browser_id.to_string(),
                    data: serde_json::json!({ "tree": combined }),
                    message: None,
                });
            }
        }
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
        // For eval results, the native binary may wrap string values in quotes — strip them
        let result_str = if (action == "evaluate" || action == "eval") && stdout.trim().starts_with('"') {
            extract_eval_result(&stdout)
        } else {
            stdout.clone()
        };
        serde_json::json!({ "result": result_str, "success": output.status.success() })
    };

    Ok(BrowserAutomationResult {
        request_id: make_request_id(),
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
            stream_port: DEFAULT_STREAM_PORT,
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
            .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
            .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
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

    pub async fn start_stream(&self, browser_id: &str) -> Result<String, String> {
        let port = self.stream_port;
        let session = session_name(browser_id);

        let mut running = self.running.lock().await;
        if *running {
            return Ok(format!("ws://localhost:{}", port));
        }

        // Check if a daemon is already listening on the stream port (e.g., started
        // by a preceding agent-browser CLI command). If so, adopt it instead of
        // killing and restarting — that would destroy the agent's active session.
        if let Ok(output) = std::process::Command::new("sh")
            .args(["-c", &format!("fuser {}/tcp 2>/dev/null", port)])
            .output()
        {
            if output.status.success() && !output.stdout.is_empty() {
                *running = true;
                return Ok(format!("ws://localhost:{}", port));
            }
        }

        // Kill any stale daemon on this port to avoid EADDRINUSE
        let _ = std::process::Command::new("sh")
            .args(["-c", &format!("fuser -k {}/tcp 2>/dev/null", port)])
            .output();
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Start daemon + open a page in one Node.js script.
        // The daemon's startDaemon() runs forever (blocks), so we call it without await.
        // After 3s, we connect to the daemon's Unix socket and send a "navigate" command
        // to trigger the browser auto-launch. The CLI binary can't do this because it's
        // a separate process that doesn't talk to the daemon.
        let script = format!(
            "const m = await import('agent-browser'); \
             m.setSession('{}'); \
             m.startDaemon({{ streamPort: {} }}); \
             await new Promise(r => setTimeout(r, 3000)); \
             const net = await import('node:net'); \
             const sock = net.createConnection(m.getSocketPath()); \
             sock.write(JSON.stringify({{id:'init',action:'navigate',url:'about:blank'}}) + '\\n'); \
             sock.on('data', () => {{}});",
            session, port
        );
        eprintln!("[codemux::browser] Starting daemon+browser on port {} session={}", port, session);
        let daemon_cmd = format!(
            "node --input-type=module -e {} >/tmp/codemux-browser-daemon.log 2>&1",
            shell_quote(&script)
        );
        std::process::Command::new("sh")
            .args(["-c", &daemon_cmd])
            .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
            .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
            .stdin(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start browser stream: {}", e))?;

        // Wait for daemon + stream server + browser launch + first screencast frames
        eprintln!("[codemux::browser] Waiting 8s for daemon + browser...");
        std::thread::sleep(std::time::Duration::from_millis(8000));
        eprintln!("[codemux::browser] Stream ready at ws://localhost:{}", port);
        *running = true;

        Ok(format!("ws://localhost:{}", port))
    }
}
