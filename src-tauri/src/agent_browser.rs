use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
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
/// Maximum stream port (inclusive). Ports 9223–9299 are reserved.
const MAX_STREAM_PORT: u16 = 9299;

/// Kill all agent-browser daemon processes.
/// Called on app shutdown to prevent stale daemons across restarts.
pub fn kill_stream_daemons() {
    let _ = std::process::Command::new("sh")
        .args(["-c", "pkill -f 'agent-browser.*daemon' 2>/dev/null; pkill -f 'agent-browser.*--session' 2>/dev/null"])
        .output();
}

/// Per-session stream state.
struct StreamSession {
    port: u16,
    running: bool,
}

pub struct AgentBrowserManager {
    /// Atomic counter for the next port to try allocating.
    next_port: AtomicU16,
    /// Per-session state keyed by session identifier (workspace_id or cli_session_name).
    sessions: Mutex<HashMap<String, StreamSession>>,
    /// Serializes start_stream calls to prevent concurrent daemon launches
    /// from racing (e.g., React StrictMode double-mount, pane remount + agent action).
    start_lock: Mutex<()>,
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

/// Resolve the path to the agent-browser native binary.
///
/// Search order (first match wins):
/// 1. System PATH — covers AUR `agent-browser` package and manual installs
/// 2. Tauri sidecar — bundled next to the executable in AppImage/deb/rpm
/// 3. node_modules — dev mode (`npm run tauri dev`)
/// 4. npx fallback — always works if Node.js + npm are present
fn resolve_binary() -> String {
    // 1. System PATH (AUR/system package, cargo install, manual install)
    if let Ok(output) = std::process::Command::new("which")
        .arg("agent-browser")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // Skip the node_modules/.bin shim — we want the native binary directly
            if !path.is_empty() && !path.contains("node_modules/.bin") {
                return path;
            }
        }
    }

    // Determine the platform-specific sidecar name
    let target_triple = if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "x86_64-apple-darwin"
    } else {
        return "npx agent-browser".to_string();
    };

    let npm_binary = if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        "agent-browser-linux-x64"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        "agent-browser-linux-arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "agent-browser-darwin-arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "agent-browser-darwin-x64"
    } else {
        return "npx agent-browser".to_string();
    };

    let sidecar_name = format!("agent-browser-{target_triple}");

    // 2. Tauri sidecar — next to the executable (AppImage/deb/rpm installs)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(&sidecar_name);
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    // 3. node_modules — dev mode (npm run tauri dev)
    // Tauri runs from src-tauri/ but node_modules is at the project root.
    // Check cwd, parent of cwd, and parent of exe for the npm binary.
    let mut search_dirs = vec![
        std::env::current_dir().unwrap_or_default(),
    ];
    // Parent of cwd (project root when cwd is src-tauri/)
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(parent) = cwd.parent() {
            search_dirs.push(parent.to_path_buf());
        }
    }
    // Directory containing the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            search_dirs.push(dir.to_path_buf());
        }
    }
    for base in &search_dirs {
        let candidate = base.join("node_modules/agent-browser/bin").join(npm_binary);
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }

    // 4. npx fallback — spawns Node.js shim, requires npm + Node.js
    "npx agent-browser".to_string()
}

fn build_agent_browser_command(session: &str, action: &str, params: &serde_json::Value) -> Result<String, String> {
    let bin = resolve_binary();
    let command = match action {
        "open_url" | "open" => {
            let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("about:blank");
            format!(
                "{bin} open {url} --session {s} && {bin} wait --load load --session {s}",
                bin = bin,
                url = shell_quote(url),
                s = session,
            )
        }
        "screenshot" => format!("{} screenshot --session {}", bin, session),
        "snapshot" | "accessibility_snapshot" => {
            format!("{} snapshot -i --session {}", bin, session)
        }
        "click" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("{} click {} --session {}", bin, shell_quote(selector), session)
        }
        "fill" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
            format!(
                "{} fill {} {} --session {}",
                bin,
                shell_quote(selector),
                shell_quote(value),
                session
            )
        }
        "type_text" => {
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            format!("{} type body {} --session {}", bin, shell_quote(text), session)
        }
        "console_logs" | "console" => format!("{} console --session {}", bin, session),
        "evaluate" | "eval" => {
            let script = params.get("script").and_then(|v| v.as_str()).unwrap_or("");
            format!("{} eval {} --session {}", bin, shell_quote(script), session)
        }
        "back" => format!("{} back --session {}", bin, session),
        "forward" => format!("{} forward --session {}", bin, session),
        "reload" => format!("{} reload --session {}", bin, session),
        "viewport" => {
            let w = params.get("width").and_then(|v| v.as_u64()).unwrap_or(1280);
            let h = params.get("height").and_then(|v| v.as_u64()).unwrap_or(720);
            format!("{} set viewport {} {} --session {}", bin, w, h, session)
        }
        // New v0.24.0 commands
        "get_styles" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("{} get styles {} --json --session {}", bin, shell_quote(selector), session)
        }
        "wait" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            let text = params.get("text").and_then(|v| v.as_str());
            if let Some(text) = text {
                format!("{} wait --text {} --session {}", bin, shell_quote(text), session)
            } else {
                format!("{} wait {} --session {}", bin, shell_quote(selector), session)
            }
        }
        "get_text" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("{} get text {} --session {}", bin, shell_quote(selector), session)
        }
        "get_box" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("{} get box {} --json --session {}", bin, shell_quote(selector), session)
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

fn execute_agent_browser_action(browser_id: &str, action: &str, params: serde_json::Value, stream_port: u16) -> Result<BrowserAutomationResult, String> {
    let session = session_name(browser_id);
    let shell_cmd = build_agent_browser_command(session, action, &params)?;
    let output = std::process::Command::new("sh")
        .args(["-c", &shell_cmd])
        .env("AGENT_BROWSER_STREAM_PORT", stream_port.to_string())
        .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
        .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
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

pub fn run_cli_action(browser_id: &str, action: &str, params: serde_json::Value, stream_port: u16) -> Result<BrowserAutomationResult, String> {
    execute_agent_browser_action(browser_id, action, params, stream_port)
}

impl Default for AgentBrowserManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBrowserManager {
    pub fn new() -> Self {
        Self {
            next_port: AtomicU16::new(DEFAULT_STREAM_PORT),
            sessions: Mutex::new(HashMap::new()),
            start_lock: Mutex::new(()),
        }
    }

    /// Create a new manager and kill stale daemons from previous app runs.
    /// Uses both process-name matching and port-based cleanup to handle
    /// daemons that survive pkill (shared daemon, different session name, etc.).
    pub fn new_with_cleanup() -> Self {
        kill_stream_daemons();
        // Also kill by port — pkill may miss daemons with unexpected command lines.
        for port in DEFAULT_STREAM_PORT..=DEFAULT_STREAM_PORT + 10 {
            let _ = std::process::Command::new("sh")
                .args(["-c", &format!("fuser -k {}/tcp 2>/dev/null", port)])
                .output();
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        Self::new()
    }

    /// Allocate a unique stream port for the given session key.
    /// Returns the existing port if already allocated, or assigns the next
    /// available port in the range [DEFAULT_STREAM_PORT, MAX_STREAM_PORT].
    ///
    /// Does NOT check if the port is free — start_stream uses fuser -k to
    /// reclaim ports occupied by stale daemons. A try-bind check here would
    /// skip our own stale daemon's port, causing the agent-browser CLI to
    /// reuse the stale daemon while BrowserPane connects to the wrong port.
    pub async fn allocate_port(&self, session_key: &str) -> Result<u16, String> {
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get(session_key) {

            return Ok(s.port);
        }

        // Assign the next port from the counter, skipping ports owned by other sessions.
        let range_size = MAX_STREAM_PORT - DEFAULT_STREAM_PORT + 1;
        for _ in 0..range_size {
            let port = self.next_port.fetch_add(1, Ordering::Relaxed);
            // Wrap around if we've gone past the range
            if port > MAX_STREAM_PORT {
                self.next_port.store(DEFAULT_STREAM_PORT, Ordering::Relaxed);
                continue;
            }
            // Check no other session already owns this port
            if sessions.values().any(|s| s.port == port) {
                continue;
            }

            sessions.insert(session_key.to_string(), StreamSession {
                port,
                running: false,
            });
            return Ok(port);
        }
        Err("All browser stream ports (9223-9299) are in use".to_string())
    }

    /// Return the port allocated for a session, if any.
    pub async fn get_port(&self, session_key: &str) -> Option<u16> {
        self.sessions.lock().await.get(session_key).map(|s| s.port)
    }

    /// Register an alias key for an already-allocated port.
    /// Used so that both workspace_id and cli_session_name map to the same port.
    pub async fn ensure_port(&self, session_key: &str, port: u16) {

        let mut sessions = self.sessions.lock().await;
        sessions.entry(session_key.to_string()).or_insert(StreamSession {
            port,
            running: false,
        });
    }

    pub async fn spawn(&self, browser_id: &str) -> Result<(), String> {
        let session = session_name(browser_id);
        let bin = resolve_binary();
        let port = self.allocate_port(browser_id).await?;

        {
            let sessions = self.sessions.lock().await;
            if sessions.get(browser_id).map_or(false, |s| s.running) {
                return Ok(());
            }
        }

        let output = std::process::Command::new("sh")
            .args(["-c", &format!("{} open about:blank --headless --session {}", bin, session)])
            .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
            .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
            .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
            .output()
            .map_err(|e| format!("Failed to start agent-browser: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to start browser: {}", stderr));
        }

        self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|s| s.running = true);
        Ok(())
    }

    pub async fn run_command(&self, browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
        let port = self.allocate_port(browser_id).await?;
        // Mark running BEFORE the blocking CLI call. The CLI auto-starts the
        // daemon on first invocation, so it's effectively "running" as soon as
        // we call it. Setting this early prevents a concurrent start_stream
        // (from BrowserPane mounting) from killing the daemon mid-command.
        self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|s| s.running = true);
        execute_agent_browser_action(browser_id, action, params, port)
    }

    pub async fn get_screenshot(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id);
        let bin = resolve_binary();
        let port = self.allocate_port(browser_id).await?;

        let output = std::process::Command::new("sh")
            .args(["-c", &format!("{} screenshot --session {}", bin, session)])
            .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
            .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
            .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
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
        let bin = resolve_binary();

        // Kill the daemon on this session's port before closing
        if let Some(s) = self.sessions.lock().await.remove(browser_id) {
            let _ = std::process::Command::new("sh")
                .args(["-c", &format!("fuser -k {}/tcp 2>/dev/null", s.port)])
                .output();
        }

        let _ = std::process::Command::new("sh")
            .args(["-c", &format!("{} close --session {}", bin, session)])
            .output();
        Ok(())
    }

    /// Start the browser session and return the WebSocket stream URL.
    ///
    /// With agent-browser v0.24.0+, the Rust daemon auto-starts on first
    /// command and streaming is enabled by default. We just need to:
    /// 1. Allocate a unique port for this session
    /// 2. Set AGENT_BROWSER_STREAM_PORT so the daemon binds to our port
    /// 3. Run any command to trigger daemon + browser launch
    /// 4. Return the WebSocket URL
    pub async fn start_stream(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id);
        let bin = resolve_binary();

        // Serialize start_stream calls. The old code held a single Mutex<bool>
        // across the entire operation (including sleeps). Without this, concurrent
        // callers (React StrictMode double-mount, pane remount + agent action)
        // both see running=false and race to start/kill the daemon.
        let _start_guard = self.start_lock.lock().await;

        // Re-check running under the start_lock — a prior call may have finished.
        if let Some(port) = self.get_port(browser_id).await {
            let sessions = self.sessions.lock().await;
            if sessions.get(browser_id).map_or(false, |s| s.running) {

                return Ok(format!("ws://localhost:{}", port));
            }
        }

        // Close any stale daemon for this session name (from a previous app run)
        // BEFORE allocating a port. Otherwise allocate_port's try-bind sees the
        // stale daemon's port as occupied, skips it, and allocates a different port.
        // The agent-browser CLI would then reuse the stale daemon (by session name)
        // while BrowserPane connects to the newly allocated (empty) port.

        let _ = std::process::Command::new("sh")
            .args(["-c", &format!("{} close --session {} 2>/dev/null", bin, session)])
            .output();

        let port = self.allocate_port(browser_id).await?;

        // Kill any other process on the allocated port (non-agent-browser services).
        let _ = std::process::Command::new("sh")
            .args(["-c", &format!("fuser -k {}/tcp 2>/dev/null", port)])
            .output();
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Launch browser via CLI. The v0.24.0 Rust daemon auto-starts and
        // streaming is enabled by default when AGENT_BROWSER_STREAM_PORT is set.
        eprintln!("[codemux::browser] Starting browser session={} port={}", session, port);
        let launch_cmd = format!(
            "{} open about:blank --headless --session {}",
            bin, session
        );
        let _ = std::process::Command::new("sh")
            .args(["-c", &launch_cmd])
            .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
            .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
            .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
            .output();

        // Give the daemon a moment to start the WebSocket stream server.
        std::thread::sleep(std::time::Duration::from_millis(1000));
        self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|s| s.running = true);

        Ok(format!("ws://localhost:{}", port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_binary_returns_existing_path_or_npx_fallback() {
        let result = resolve_binary();
        if result.starts_with("npx ") {
            // Fallback is acceptable (e.g., binary not in cwd/node_modules)
            assert_eq!(result, "npx agent-browser");
        } else {
            // Must be a real path that exists and is a file
            let path = std::path::Path::new(&result);
            assert!(path.exists(), "resolve_binary returned non-existent path: {}", result);
            assert!(path.is_file(), "resolve_binary returned non-file: {}", result);
        }
    }

    #[test]
    fn resolve_binary_finds_native_binary_from_project_root() {
        // Run from the project root where node_modules exists
        let result = resolve_binary();
        // On this machine, the native binary should be found in node_modules
        if !result.starts_with("npx ") {
            assert!(
                result.contains("agent-browser-linux-x64") || result.contains("agent-browser-darwin"),
                "Expected platform-specific binary name, got: {}",
                result
            );
            // Verify it's executable
            let output = std::process::Command::new(&result)
                .arg("--version")
                .output();
            assert!(output.is_ok(), "Binary at {} is not executable", result);
            let out = output.unwrap();
            let version = String::from_utf8_lossy(&out.stdout);
            assert!(
                version.contains("agent-browser") || out.status.success(),
                "Binary didn't respond to --version: {}",
                version
            );
        }
    }

    #[test]
    fn build_command_open_chains_wait_load() {
        let cmd = build_agent_browser_command("test-session", "open", &serde_json::json!({"url": "https://example.com"})).unwrap();
        let bin = resolve_binary();
        assert!(cmd.starts_with(&bin), "Command should start with resolved binary: {}", cmd);
        assert!(cmd.contains("--session test-session"));
        assert!(cmd.contains("https://example.com"));
        assert!(cmd.contains("wait --load load"), "Should wait for load event: {}", cmd);
        assert!(!cmd.contains("stream disable"), "Should NOT restart stream: {}", cmd);
    }

    #[test]
    fn build_command_viewport_uses_set_viewport() {
        let cmd = build_agent_browser_command("s", "viewport", &serde_json::json!({"width": 800, "height": 600})).unwrap();
        assert!(cmd.contains("set viewport 800 600"), "v0.24.0 uses 'set viewport', got: {}", cmd);
    }

    #[test]
    fn build_command_unknown_action_returns_error() {
        let result = build_agent_browser_command("s", "nonexistent_action", &serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown action"));
    }

    #[tokio::test]
    async fn two_sessions_get_different_ports() {
        let mgr = AgentBrowserManager::new();
        let p1 = mgr.allocate_port("workspace-a").await.unwrap();
        let p2 = mgr.allocate_port("workspace-b").await.unwrap();
        assert_ne!(p1, p2, "Two workspaces must get different ports");
        assert!(p1 >= DEFAULT_STREAM_PORT && p1 <= MAX_STREAM_PORT);
        assert!(p2 >= DEFAULT_STREAM_PORT && p2 <= MAX_STREAM_PORT);
    }

    #[tokio::test]
    async fn allocate_port_is_idempotent() {
        let mgr = AgentBrowserManager::new();
        let p1 = mgr.allocate_port("workspace-x").await.unwrap();
        let p2 = mgr.allocate_port("workspace-x").await.unwrap();
        assert_eq!(p1, p2, "Same session key must return same port");
    }

    #[tokio::test]
    async fn ports_never_collide() {
        let mgr = AgentBrowserManager::new();
        let mut ports = std::collections::HashSet::new();
        // Allocate 10 sessions — all should get unique ports
        for i in 0..10 {
            let port = mgr.allocate_port(&format!("ws-{}", i)).await.unwrap();
            assert!(
                ports.insert(port),
                "Port {} was already assigned to another session",
                port,
            );
        }
    }

    #[tokio::test]
    async fn close_releases_session() {
        let mgr = AgentBrowserManager::new();
        let p = mgr.allocate_port("ws-close-test").await.unwrap();
        assert!(mgr.get_port("ws-close-test").await.is_some());
        let _ = mgr.close("ws-close-test").await;
        assert!(mgr.get_port("ws-close-test").await.is_none(), "Session should be removed after close");
        // Port range still works after release
        let p2 = mgr.allocate_port("ws-close-test-2").await.unwrap();
        assert!(p2 >= DEFAULT_STREAM_PORT && p2 <= MAX_STREAM_PORT);
        assert_ne!(p, p2, "New allocation should skip the (possibly still in-use) released port");
    }
}
