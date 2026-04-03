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

/// Resolve the path to the agent-browser native binary.
///
/// Search order (first match wins):
/// 1. System PATH — covers AUR `agent-browser` package and manual installs
/// 2. Tauri sidecar — bundled next to the executable in AppImage/deb/rpm
/// 3. node_modules — dev mode (`npm run tauri dev`)
/// 4. npx fallback — always works if Node.js + npm are present
fn resolve_binary() -> String {
    // 1. System PATH (AUR/system package, cargo install, manual install)
    if let Ok(output) = std::process::Command::new("which").arg("agent-browser").output() {
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
    for base in &[
        std::env::current_dir().unwrap_or_default(),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default(),
    ] {
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

fn execute_agent_browser_action(browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
    let session = session_name(browser_id);
    let shell_cmd = build_agent_browser_command(session, action, &params)?;
    let output = std::process::Command::new("sh")
        .args(["-c", &shell_cmd])
        .env("AGENT_BROWSER_STREAM_PORT", DEFAULT_STREAM_PORT.to_string())
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
        let bin = resolve_binary();

        let mut running = self.running.lock().await;
        if *running {
            return Ok(());
        }

        let output = std::process::Command::new("sh")
            .args(["-c", &format!("{} open about:blank --headless --session {}", bin, session)])
            .env("AGENT_BROWSER_STREAM_PORT", self.stream_port.to_string())
            .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
            .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
            .output()
            .map_err(|e| format!("Failed to start agent-browser: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to start browser: {}", stderr));
        }

        *running = true;
        Ok(())
    }

    pub async fn run_command(&self, browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
        execute_agent_browser_action(browser_id, action, params)
    }

    pub async fn get_screenshot(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id);
        let bin = resolve_binary();

        let output = std::process::Command::new("sh")
            .args(["-c", &format!("{} screenshot --session {}", bin, session)])
            .env("AGENT_BROWSER_STREAM_PORT", self.stream_port.to_string())
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

        let mut running = self.running.lock().await;
        *running = false;

        let _ = std::process::Command::new("sh")
            .args(["-c", &format!("{} close --session {}", bin, session)])
            .output();
        Ok(())
    }

    pub fn get_stream_url(&self) -> String {
        format!("ws://localhost:{}", self.stream_port)
    }

    /// Start the browser session and return the WebSocket stream URL.
    ///
    /// With agent-browser v0.24.0+, the Rust daemon auto-starts on first
    /// command and streaming is enabled by default. We just need to:
    /// 1. Set AGENT_BROWSER_STREAM_PORT so the daemon binds to our port
    /// 2. Run any command to trigger daemon + browser launch
    /// 3. Return the WebSocket URL
    pub async fn start_stream(&self, browser_id: &str) -> Result<String, String> {
        let port = self.stream_port;
        let session = session_name(browser_id);
        let bin = resolve_binary();
        let mut running = self.running.lock().await;

        // Check if a daemon is actually listening on the stream port.
        let port_alive = std::process::Command::new("sh")
            .args(["-c", &format!("fuser {}/tcp 2>/dev/null", port)])
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false);

        if *running && port_alive {
            return Ok(format!("ws://localhost:{}", port));
        }

        // Daemon is alive but we didn't know — adopt it.
        if port_alive && !*running {
            *running = true;
            return Ok(format!("ws://localhost:{}", port));
        }

        // Daemon died or never started — reset flag and (re)start.
        *running = false;

        // Kill any stale process on this port to avoid EADDRINUSE
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
        *running = true;

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
        // Must chain wait --load load after open
        assert!(cmd.contains("&& "), "Open command should chain wait: {}", cmd);
        assert!(cmd.contains("wait --load load"), "Open command should wait for load event: {}", cmd);
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
}
