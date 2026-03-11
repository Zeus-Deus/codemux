use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::TcpListener;
use std::process::Stdio;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout, Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

const DEFAULT_VIEWPORT_WIDTH: u32 = 1280;
const DEFAULT_VIEWPORT_HEIGHT: u32 = 720;
const MIN_VIEWPORT_WIDTH: u32 = 320;
const MIN_VIEWPORT_HEIGHT: u32 = 240;

static CDP_COMMAND_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct BrowserSession {
    browser_id: String,
    debug_port: u16,
    pid: Option<u32>,
    current_url: Option<String>,
    viewport_width: u32,
    viewport_height: u32,
    target_id: String,
    websocket_url: String,
    child: Child,
    _user_data_dir: TempDir,
}

#[derive(Debug, Clone)]
struct BrowserTarget {
    target_id: String,
    websocket_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSnapshot {
    pub title: String,
    pub url: String,
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct BrowserManager {
    sessions: tokio::sync::Mutex<HashMap<String, BrowserSession>>,
    next_port: AtomicU16,
}

impl Default for BrowserManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserManager {
    pub fn new() -> Self {
        Self {
            sessions: tokio::sync::Mutex::new(HashMap::new()),
            next_port: AtomicU16::new(9222),
        }
    }

    fn find_free_port(&self) -> u16 {
        for _ in 0..32 {
            let candidate = self.next_port.fetch_add(1, Ordering::Relaxed);
            if TcpListener::bind(("127.0.0.1", candidate)).is_ok() {
                return candidate;
            }
        }

        TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .map(|address| address.port())
            .unwrap_or(9222)
    }

    pub async fn spawn_browser(&self, browser_id: String) -> Result<(), String> {
        {
            let mut sessions = self.sessions.lock().await;
            if let Some(session) = sessions.get_mut(&browser_id) {
                match session.child.try_wait() {
                    Ok(None) => return Ok(()),
                    Ok(Some(_)) => {
                        sessions.remove(&browser_id);
                    }
                    Err(error) => {
                        sessions.remove(&browser_id);
                        return Err(format!("Failed to inspect browser process state: {error}"));
                    }
                }
            }
        }

        let browser_binary = find_browser_binary()?;
        let debug_port = self.find_free_port();
        let user_data_dir = tempfile::tempdir()
            .map_err(|error| format!("Failed to create temp dir: {error}"))?;

        let mut child = Command::new(browser_binary)
            .arg(format!("--remote-debugging-port={debug_port}"))
            .arg("--remote-debugging-address=127.0.0.1")
            .arg("--headless=new")
            .arg("--no-sandbox")
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .arg(format!("--user-data-dir={}", user_data_dir.path().display()))
            .arg("--disable-extensions")
            .arg("--disable-background-networking")
            .arg("--disable-default-apps")
            .arg("--disable-sync")
            .arg("--metrics-recording-only")
            .arg("--mute-audio")
            .arg(format!(
                "--window-size={DEFAULT_VIEWPORT_WIDTH},{DEFAULT_VIEWPORT_HEIGHT}"
            ))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("Failed to spawn browser: {error}"))?;

        let pid = child.id();
        let target = match wait_for_target(debug_port).await {
            Ok(target) => target,
            Err(error) => {
                let _ = child.kill().await;
                return Err(error);
            }
        };

        let session = BrowserSession {
            browser_id: browser_id.clone(),
            debug_port,
            pid,
            current_url: Some("about:blank".to_string()),
            viewport_width: DEFAULT_VIEWPORT_WIDTH,
            viewport_height: DEFAULT_VIEWPORT_HEIGHT,
            target_id: target.target_id,
            websocket_url: target.websocket_url,
            child,
            _user_data_dir: user_data_dir,
        };

        eprintln!(
            "[BROWSER] Spawned browser {} on port {}",
            session.browser_id, session.debug_port
        );

        self.sessions.lock().await.insert(browser_id, session);
        Ok(())
    }

    pub async fn navigate(&self, browser_id: &str, url: &str) -> Result<String, String> {
        let websocket_url = self.session_websocket_url(browser_id).await?;
        let mut ws = connect_target_websocket(&websocket_url).await?;
        let target_url = normalize_browser_url(url);

        send_cdp_command(&mut ws, "Page.enable", json!({}), Duration::from_secs(5)).await?;
        send_cdp_command(
            &mut ws,
            "Page.setLifecycleEventsEnabled",
            json!({ "enabled": true }),
            Duration::from_secs(5),
        )
        .await?;

        let result = send_cdp_command(
            &mut ws,
            "Page.navigate",
            json!({ "url": target_url }),
            Duration::from_secs(10),
        )
        .await?;

        if let Some(error_text) = result.get("errorText").and_then(Value::as_str) {
            if !error_text.trim().is_empty() {
                eprintln!(
                    "[BROWSER] Navigation failed for {} -> {}: {}",
                    browser_id, target_url, error_text
                );
                return Err(error_text.to_string());
            }
        }

        let _ = wait_for_any_event(
            &mut ws,
            &["Page.loadEventFired", "Page.frameStoppedLoading"],
            Duration::from_secs(12),
        )
        .await;
        sleep(Duration::from_millis(200)).await;

        if let Some(session) = self.sessions.lock().await.get_mut(browser_id) {
            session.current_url = Some(target_url.clone());
        }

        eprintln!("[BROWSER] Navigated {} -> {}", browser_id, target_url);
        Ok(format!("Navigated to {target_url}"))
    }

    pub async fn screenshot(&self, browser_id: &str) -> Result<String, String> {
        let websocket_url = self.session_websocket_url(browser_id).await?;
        let mut ws = connect_target_websocket(&websocket_url).await?;
        let (viewport_width, viewport_height) = self.session_viewport(browser_id).await?;

        set_viewport(&mut ws, viewport_width, viewport_height).await?;

        let result = send_cdp_command(
            &mut ws,
            "Page.captureScreenshot",
            json!({
                "format": "png",
                "fromSurface": true,
                "captureBeyondViewport": false,
                "optimizeForSpeed": true
            }),
            Duration::from_secs(10),
        )
        .await?;

        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("No screenshot data in response: {result:?}"))?;

        Ok(format!("data:image/png;base64,{data}"))
    }

    pub async fn click(&self, browser_id: &str, x: f64, y: f64) -> Result<String, String> {
        let websocket_url = self.session_websocket_url(browser_id).await?;
        let mut ws = connect_target_websocket(&websocket_url).await?;
        let (viewport_width, viewport_height) = self.session_viewport(browser_id).await?;

        set_viewport(&mut ws, viewport_width, viewport_height).await?;

        send_cdp_command(
            &mut ws,
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved",
                "x": x,
                "y": y,
                "button": "none",
                "buttons": 0,
                "pointerType": "mouse"
            }),
            Duration::from_secs(2),
        )
        .await?;

        send_cdp_command(
            &mut ws,
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": x,
                "y": y,
                "button": "left",
                "buttons": 1,
                "clickCount": 1,
                "pointerType": "mouse"
            }),
            Duration::from_secs(2),
        )
        .await?;

        send_cdp_command(
            &mut ws,
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": x,
                "y": y,
                "button": "left",
                "buttons": 0,
                "clickCount": 1,
                "pointerType": "mouse"
            }),
            Duration::from_secs(2),
        )
        .await?;

        Ok("Clicked".to_string())
    }

    pub async fn type_text(&self, browser_id: &str, text: &str) -> Result<String, String> {
        let websocket_url = self.session_websocket_url(browser_id).await?;
        let mut ws = connect_target_websocket(&websocket_url).await?;

        send_cdp_command(
            &mut ws,
            "Input.insertText",
            json!({ "text": text }),
            Duration::from_secs(3),
        )
        .await?;

        Ok(format!("Typed: {text}"))
    }

    pub async fn get_snapshot(&self, browser_id: &str) -> Result<BrowserSnapshot, String> {
        let (debug_port, target_id) = {
            let sessions = self.sessions.lock().await;
            let session = sessions
                .get(browser_id)
                .ok_or_else(|| "Browser session not found".to_string())?;
            (session.debug_port, session.target_id.clone())
        };

        get_browser_snapshot(debug_port, Some(&target_id)).await
    }

    pub async fn close_browser(&self, browser_id: &str) -> Result<(), String> {
        let mut session = {
            let mut sessions = self.sessions.lock().await;
            match sessions.remove(browser_id) {
                Some(session) => session,
                None => return Ok(()),
            }
        };

        if let Some(pid) = session.pid {
            eprintln!("[BROWSER] Closing browser {} pid {}", browser_id, pid);
        }

        match session.child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => session
                .child
                .kill()
                .await
                .map_err(|error| format!("Failed to kill browser process: {error}")),
            Err(error) => Err(format!("Failed to inspect browser process state: {error}")),
        }
    }

    async fn session_websocket_url(&self, browser_id: &str) -> Result<String, String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(browser_id)
            .ok_or_else(|| "Browser session not found".to_string())?;
        Ok(session.websocket_url.clone())
    }

    async fn session_viewport(&self, browser_id: &str) -> Result<(u32, u32), String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(browser_id)
            .ok_or_else(|| "Browser session not found".to_string())?;
        Ok((session.viewport_width, session.viewport_height))
    }

    pub async fn resize_viewport(
        &self,
        browser_id: &str,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let viewport_width = width.max(MIN_VIEWPORT_WIDTH);
        let viewport_height = height.max(MIN_VIEWPORT_HEIGHT);

        let websocket_url = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .get_mut(browser_id)
                .ok_or_else(|| "Browser session not found".to_string())?;
            session.viewport_width = viewport_width;
            session.viewport_height = viewport_height;
            session.websocket_url.clone()
        };

        let mut ws = connect_target_websocket(&websocket_url).await?;
        set_viewport(&mut ws, viewport_width, viewport_height).await?;
        Ok(())
    }
}

fn normalize_browser_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return "about:blank".to_string();
    }

    if trimmed.eq_ignore_ascii_case("about:blank") {
        return "about:blank".to_string();
    }

    if trimmed.contains("://") {
        return trimmed.to_string();
    }

    format!("https://{trimmed}")
}

fn find_browser_binary() -> Result<&'static str, String> {
    const CANDIDATES: &[&str] = &[
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
        "google-chrome",
        "brave-browser",
    ];

    for candidate in CANDIDATES {
        let result = std::process::Command::new(candidate)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if result.is_ok() {
            return Ok(candidate);
        }
    }

    Err(
        "Could not find a Chromium-compatible browser in PATH (tried chromium, chromium-browser, google-chrome-stable, google-chrome, brave-browser)"
            .to_string(),
    )
}

async fn wait_for_target(debug_port: u16) -> Result<BrowserTarget, String> {
    let deadline = Instant::now() + Duration::from_secs(15);

    loop {
        match discover_target(debug_port).await {
            Ok(target) => return Ok(target),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "Browser failed to expose a page target on port {debug_port}: {error}"
                    ));
                }
            }
        }

        sleep(Duration::from_millis(250)).await;
    }
}

async fn discover_target(debug_port: u16) -> Result<BrowserTarget, String> {
    let response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{debug_port}/json/list"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map_err(|error| format!("Failed to get browser targets: {error}"))?;

    let targets: Vec<Value> = response
        .json()
        .await
        .map_err(|error| format!("Failed to parse browser targets: {error}"))?;

    let target = targets
        .iter()
        .find(|entry| entry.get("type").and_then(Value::as_str) == Some("page"))
        .ok_or_else(|| "No page target found".to_string())?;

    let target_id = target
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "No target id found".to_string())?
        .to_string();
    let websocket_url = target
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| "No WebSocket URL found".to_string())?
        .to_string();

    Ok(BrowserTarget {
        target_id,
        websocket_url,
    })
}

async fn get_browser_snapshot(
    debug_port: u16,
    preferred_target_id: Option<&str>,
) -> Result<BrowserSnapshot, String> {
    let response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{debug_port}/json/list"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|error| format!("Failed to get browser targets: {error}"))?;

    let targets: Vec<Value> = response
        .json()
        .await
        .map_err(|error| format!("Failed to parse browser targets: {error}"))?;

    let target = preferred_target_id
        .and_then(|target_id| {
            targets.iter().find(|entry| {
                entry.get("id").and_then(Value::as_str) == Some(target_id)
                    && entry.get("type").and_then(Value::as_str) == Some("page")
            })
        })
        .or_else(|| {
            targets
                .iter()
                .find(|entry| entry.get("type").and_then(Value::as_str) == Some("page"))
        })
        .ok_or_else(|| "No page target found".to_string())?;

    let title = target
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled")
        .to_string();
    let url = target
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    Ok(BrowserSnapshot { title, url })
}

async fn connect_target_websocket(websocket_url: &str) -> Result<WsStream, String> {
    let (ws, _) = connect_async(websocket_url)
        .await
        .map_err(|error| format!("WebSocket connect failed: {error}"))?;
    Ok(ws)
}

async fn set_viewport(ws: &mut WsStream, width: u32, height: u32) -> Result<(), String> {
    send_cdp_command(
        ws,
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": width,
            "height": height,
            "deviceScaleFactor": 1,
            "mobile": false
        }),
        Duration::from_secs(3),
    )
    .await?;

    Ok(())
}

async fn send_cdp_command(
    ws: &mut WsStream,
    method: &str,
    params: Value,
    response_timeout: Duration,
) -> Result<Value, String> {
    let id = CDP_COMMAND_COUNTER.fetch_add(1, Ordering::Relaxed);
    let payload = json!({
        "id": id,
        "method": method,
        "params": params,
    });

    ws.send(Message::Text(payload.to_string().into()))
        .await
        .map_err(|error| format!("Failed to send {method}: {error}"))?;

    read_until_response(ws, id, response_timeout).await
}

async fn read_until_response(
    ws: &mut WsStream,
    expected_id: u64,
    response_timeout: Duration,
) -> Result<Value, String> {
    let deadline = Instant::now() + response_timeout;

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| format!("Timed out waiting for CDP response {expected_id}"))?;

        let next_message = timeout(remaining, ws.next())
            .await
            .map_err(|_| format!("Timed out waiting for CDP response {expected_id}"))?;

        let Some(message) = next_message else {
            return Err("Browser WebSocket closed unexpectedly".to_string());
        };

        match message.map_err(|error| format!("WebSocket error: {error}"))? {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text)
                    .map_err(|error| format!("Failed to parse CDP response: {error}"))?;

                if value.get("id").and_then(Value::as_u64) != Some(expected_id) {
                    continue;
                }

                if let Some(error) = value.get("error") {
                    return Err(format!("CDP command failed: {error}"));
                }

                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            Message::Binary(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("Failed to parse CDP response: {error}"))?;

                if value.get("id").and_then(Value::as_u64) != Some(expected_id) {
                    continue;
                }

                if let Some(error) = value.get("error") {
                    return Err(format!("CDP command failed: {error}"));
                }

                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            Message::Ping(payload) => {
                ws.send(Message::Pong(payload))
                    .await
                    .map_err(|error| format!("Failed to reply to WebSocket ping: {error}"))?;
            }
            Message::Pong(_) => {}
            Message::Close(frame) => {
                return Err(format!("Browser WebSocket closed: {frame:?}"));
            }
            Message::Frame(_) => {}
        }
    }
}

async fn wait_for_any_event(
    ws: &mut WsStream,
    expected_methods: &[&str],
    event_timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + event_timeout;

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| "Timed out waiting for page event".to_string())?;

        let next_message = timeout(remaining, ws.next())
            .await
            .map_err(|_| "Timed out waiting for page event".to_string())?;

        let Some(message) = next_message else {
            return Err("Browser WebSocket closed unexpectedly".to_string());
        };

        match message.map_err(|error| format!("WebSocket error: {error}"))? {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text)
                    .map_err(|error| format!("Failed to parse CDP event: {error}"))?;

                if let Some(method) = value.get("method").and_then(Value::as_str) {
                    if expected_methods.iter().any(|expected| expected == &method) {
                        return Ok(());
                    }
                }
            }
            Message::Binary(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("Failed to parse CDP event: {error}"))?;

                if let Some(method) = value.get("method").and_then(Value::as_str) {
                    if expected_methods.iter().any(|expected| expected == &method) {
                        return Ok(());
                    }
                }
            }
            Message::Ping(payload) => {
                ws.send(Message::Pong(payload))
                    .await
                    .map_err(|error| format!("Failed to reply to WebSocket ping: {error}"))?;
            }
            Message::Pong(_) => {}
            Message::Close(frame) => {
                return Err(format!("Browser WebSocket closed: {frame:?}"));
            }
            Message::Frame(_) => {}
        }
    }
}
