use crate::agent_browser::BrowserAutomationResult;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::{json, Value};
use tokio::time::{sleep, timeout, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------------------
// Key mapping
// ---------------------------------------------------------------------------

struct KeyInfo {
    key: &'static str,
    code: &'static str,
    key_code: u32,
}

fn lookup_special_key(name: &str) -> Option<KeyInfo> {
    match name {
        "Enter" => Some(KeyInfo { key: "Enter", code: "Enter", key_code: 13 }),
        "Tab" => Some(KeyInfo { key: "Tab", code: "Tab", key_code: 9 }),
        "Escape" => Some(KeyInfo { key: "Escape", code: "Escape", key_code: 27 }),
        "Backspace" => Some(KeyInfo { key: "Backspace", code: "Backspace", key_code: 8 }),
        "Delete" => Some(KeyInfo { key: "Delete", code: "Delete", key_code: 46 }),
        "ArrowUp" => Some(KeyInfo { key: "ArrowUp", code: "ArrowUp", key_code: 38 }),
        "ArrowDown" => Some(KeyInfo { key: "ArrowDown", code: "ArrowDown", key_code: 40 }),
        "ArrowLeft" => Some(KeyInfo { key: "ArrowLeft", code: "ArrowLeft", key_code: 37 }),
        "ArrowRight" => Some(KeyInfo { key: "ArrowRight", code: "ArrowRight", key_code: 39 }),
        "Home" => Some(KeyInfo { key: "Home", code: "Home", key_code: 36 }),
        "End" => Some(KeyInfo { key: "End", code: "End", key_code: 35 }),
        "PageUp" => Some(KeyInfo { key: "PageUp", code: "PageUp", key_code: 33 }),
        "PageDown" => Some(KeyInfo { key: "PageDown", code: "PageDown", key_code: 34 }),
        "Space" | " " => Some(KeyInfo { key: " ", code: "Space", key_code: 32 }),
        _ => None,
    }
}

fn parse_key_combo(input: &str) -> (i32, String) {
    let parts: Vec<&str> = input.split('+').collect();
    if parts.len() == 1 {
        return (0, input.to_string());
    }
    let mut modifiers = 0i32;
    for part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "alt" => modifiers |= 1,
            "ctrl" | "control" => modifiers |= 2,
            "meta" | "cmd" | "command" => modifiers |= 4,
            "shift" => modifiers |= 8,
            _ => {}
        }
    }
    (modifiers, parts[parts.len() - 1].to_string())
}

// ---------------------------------------------------------------------------
// Bezier mouse movement
// ---------------------------------------------------------------------------

/// Generate points along a quadratic Bezier curve for human-like mouse movement.
/// Must be called from a sync context (uses thread_rng internally, which is !Send).
pub fn generate_bezier_points(start: (f64, f64), end: (f64, f64), n: usize) -> Vec<(f64, f64)> {
    let offset: f64 = 0.1 + rand::random::<f64>() * 0.2;
    let mid = ((start.0 + end.0) / 2.0, (start.1 + end.1) / 2.0);
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let ctrl = (mid.0 + (-dy) * offset, mid.1 + dx * offset);
    (1..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            (
                u * u * start.0 + 2.0 * u * t * ctrl.0 + t * t * end.0,
                u * u * start.1 + 2.0 * u * t * ctrl.1 + t * t * end.1,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// WebSocket helpers
// ---------------------------------------------------------------------------

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_stream(port: u16) -> Result<WsStream, String> {
    let url = format!("ws://127.0.0.1:{}", port);
    let (ws, _) = connect_async(&url)
        .await
        .map_err(|e| format!("Failed to connect to stream server at {}: {}. Is a browser pane open?", url, e))?;
    Ok(ws)
}

async fn send_msg(ws: &mut WsStream, msg: Value) -> Result<(), String> {
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .map_err(|e| format!("Failed to send stream message: {}", e))
}

// ---------------------------------------------------------------------------
// Message builders
// ---------------------------------------------------------------------------

fn mouse_event(event_type: &str, x: f64, y: f64, button: &str, click_count: i32, dx: f64, dy: f64, modifiers: i32) -> Value {
    json!({
        "type": "input_mouse",
        "eventType": event_type,
        "x": x, "y": y,
        "button": button,
        "clickCount": click_count,
        "deltaX": dx, "deltaY": dy,
        "modifiers": modifiers
    })
}

fn kb_event(event_type: &str, key: &str, code: &str, text: &str, modifiers: i32, vk: u32) -> Value {
    json!({
        "type": "input_keyboard",
        "eventType": event_type,
        "key": key, "code": code, "text": text,
        "modifiers": modifiers,
        "windowsVirtualKeyCode": vk
    })
}

// ---------------------------------------------------------------------------
// Public action functions
// ---------------------------------------------------------------------------

pub async fn click_at(port: u16, x: f64, y: f64, click_type: &str) -> Result<String, String> {
    // Pre-compute all random values before any .await (thread_rng is !Send)
    let (points, delays, pause, dbl_pause) = {
        let mut rng = rand::thread_rng();
        let start_x = (x - 80.0 + rng.gen::<f64>() * 40.0).max(0.0);
        let start_y = (y - 60.0 + rng.gen::<f64>() * 30.0).max(0.0);
        let pts = generate_bezier_points((start_x, start_y), (x, y), 5);
        let dls: Vec<u64> = (0..pts.len()).map(|_| 15 + rng.gen::<u64>() % 15).collect();
        let p = 30 + rng.gen::<u64>() % 50;
        let dp = 40 + rng.gen::<u64>() % 30;
        (pts, dls, p, dp)
    };

    let mut ws = connect_stream(port).await?;

    for (i, (px, py)) in points.iter().enumerate() {
        send_msg(&mut ws, mouse_event("mouseMoved", *px, *py, "none", 0, 0.0, 0.0, 0)).await?;
        sleep(Duration::from_millis(delays[i])).await;
    }

    send_msg(&mut ws, mouse_event("mouseMoved", x, y, "none", 0, 0.0, 0.0, 0)).await?;
    sleep(Duration::from_millis(pause)).await;

    let button = if click_type == "right" { "right" } else { "left" };
    send_msg(&mut ws, mouse_event("mousePressed", x, y, button, 1, 0.0, 0.0, 0)).await?;
    send_msg(&mut ws, mouse_event("mouseReleased", x, y, button, 1, 0.0, 0.0, 0)).await?;

    if click_type == "double" {
        sleep(Duration::from_millis(dbl_pause)).await;
        send_msg(&mut ws, mouse_event("mousePressed", x, y, "left", 2, 0.0, 0.0, 0)).await?;
        send_msg(&mut ws, mouse_event("mouseReleased", x, y, "left", 2, 0.0, 0.0, 0)).await?;
    }

    let _ = ws.close(None).await;
    Ok(format!("Clicked ({click_type}) at ({x}, {y})"))
}

pub async fn type_at(port: u16, text: &str, x: Option<f64>, y: Option<f64>) -> Result<String, String> {
    let mut ws = connect_stream(port).await?;

    if let (Some(cx), Some(cy)) = (x, y) {
        send_msg(&mut ws, mouse_event("mouseMoved", cx, cy, "none", 0, 0.0, 0.0, 0)).await?;
        sleep(Duration::from_millis(50)).await;
        send_msg(&mut ws, mouse_event("mousePressed", cx, cy, "left", 1, 0.0, 0.0, 0)).await?;
        send_msg(&mut ws, mouse_event("mouseReleased", cx, cy, "left", 1, 0.0, 0.0, 0)).await?;
        sleep(Duration::from_millis(100)).await;
    }

    for ch in text.chars() {
        if ch == '\n' {
            send_msg(&mut ws, kb_event("rawKeyDown", "Enter", "Enter", "\r", 0, 13)).await?;
            send_msg(&mut ws, kb_event("keyUp", "Enter", "Enter", "", 0, 13)).await?;
        } else if ch == '\t' {
            send_msg(&mut ws, kb_event("rawKeyDown", "Tab", "Tab", "", 0, 9)).await?;
            send_msg(&mut ws, kb_event("keyUp", "Tab", "Tab", "", 0, 9)).await?;
        } else {
            let s = ch.to_string();
            let vk = if ch.is_ascii_alphabetic() { ch.to_ascii_uppercase() as u32 } else { 0 };
            send_msg(&mut ws, kb_event("keyDown", &s, "", &s, 0, vk)).await?;
            send_msg(&mut ws, kb_event("keyUp", &s, "", "", 0, vk)).await?;
        }
        sleep(Duration::from_millis(10)).await;
    }

    let _ = ws.close(None).await;
    Ok(format!("Typed {} characters", text.len()))
}

pub async fn scroll_at(port: u16, x: f64, y: f64, direction: &str, amount: i32) -> Result<String, String> {
    let mut ws = connect_stream(port).await?;
    let ticks = amount.clamp(1, 10);
    let (dx, dy) = match direction {
        "up" => (0.0, -(ticks as f64) * 120.0),
        "down" => (0.0, (ticks as f64) * 120.0),
        "left" => (-(ticks as f64) * 120.0, 0.0),
        "right" => ((ticks as f64) * 120.0, 0.0),
        _ => (0.0, (ticks as f64) * 120.0),
    };
    send_msg(&mut ws, mouse_event("mouseWheel", x, y, "none", 0, dx, dy, 0)).await?;
    let _ = ws.close(None).await;
    Ok(format!("Scrolled {direction} by {ticks} ticks at ({x}, {y})"))
}

pub async fn key_press(port: u16, key: &str) -> Result<String, String> {
    let mut ws = connect_stream(port).await?;
    let (modifiers, base_key) = parse_key_combo(key);

    if let Some(info) = lookup_special_key(&base_key) {
        send_msg(&mut ws, kb_event("rawKeyDown", info.key, info.code, "", modifiers, info.key_code)).await?;
        send_msg(&mut ws, kb_event("keyUp", info.key, info.code, "", modifiers, info.key_code)).await?;
    } else if base_key.len() == 1 {
        let ch = base_key.chars().next().unwrap();
        let text = if modifiers == 0 { base_key.clone() } else { String::new() };
        let vk = if ch.is_ascii_alphabetic() { ch.to_ascii_uppercase() as u32 } else { 0 };
        let code = if ch.is_ascii_alphabetic() { format!("Key{}", ch.to_ascii_uppercase()) } else { String::new() };
        send_msg(&mut ws, kb_event("rawKeyDown", &base_key, &code, &text, modifiers, vk)).await?;
        send_msg(&mut ws, kb_event("keyUp", &base_key, &code, "", modifiers, vk)).await?;
    } else {
        return Err(format!("Unknown key: {}", base_key));
    }

    let _ = ws.close(None).await;
    Ok(format!("Pressed key: {key}"))
}

pub async fn drag(port: u16, sx: f64, sy: f64, ex: f64, ey: f64) -> Result<String, String> {
    let (points, delays) = {
        let mut rng = rand::thread_rng();
        let pts = generate_bezier_points((sx, sy), (ex, ey), 5);
        let dls: Vec<u64> = (0..pts.len()).map(|_| 15 + rng.gen::<u64>() % 15).collect();
        (pts, dls)
    };

    let mut ws = connect_stream(port).await?;
    send_msg(&mut ws, mouse_event("mouseMoved", sx, sy, "none", 0, 0.0, 0.0, 0)).await?;
    sleep(Duration::from_millis(50)).await;
    send_msg(&mut ws, mouse_event("mousePressed", sx, sy, "left", 1, 0.0, 0.0, 0)).await?;
    sleep(Duration::from_millis(50)).await;

    for (i, (px, py)) in points.iter().enumerate() {
        send_msg(&mut ws, mouse_event("mouseMoved", *px, *py, "left", 0, 0.0, 0.0, 0)).await?;
        sleep(Duration::from_millis(delays[i])).await;
    }

    send_msg(&mut ws, mouse_event("mouseReleased", ex, ey, "left", 1, 0.0, 0.0, 0)).await?;
    let _ = ws.close(None).await;
    Ok(format!("Dragged from ({sx}, {sy}) to ({ex}, {ey})"))
}

pub async fn get_viewport(port: u16) -> Result<(u32, u32), String> {
    let mut ws = connect_stream(port).await?;
    send_msg(&mut ws, json!({"type": "status"})).await?;

    let result = timeout(Duration::from_secs(3), async {
        while let Some(msg) = ws.next().await {
            if let Ok(Message::Text(text)) = msg {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if v.get("type").and_then(Value::as_str) == Some("status") {
                        let w = v.get("viewportWidth").and_then(Value::as_u64).unwrap_or(1280) as u32;
                        let h = v.get("viewportHeight").and_then(Value::as_u64).unwrap_or(720) as u32;
                        return Ok((w, h));
                    }
                }
            }
        }
        Err("Stream server closed without status".to_string())
    })
    .await
    .map_err(|_| "Timed out waiting for viewport status".to_string())?;

    let _ = ws.close(None).await;
    result
}

// ---------------------------------------------------------------------------
// Unified dispatcher
// ---------------------------------------------------------------------------

fn make_request_id() -> String {
    format!("req-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
}

pub async fn handle_vision_action(port: u16, action: &str, params: Value) -> Result<BrowserAutomationResult, String> {
    let text = match action {
        "click_at" => {
            let x = params.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = params.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let ct = params.get("click_type").and_then(Value::as_str).unwrap_or("left");
            click_at(port, x, y, ct).await?
        }
        "type_at" => {
            let t = params.get("text").and_then(Value::as_str).unwrap_or("");
            let x = params.get("x").and_then(Value::as_f64);
            let y = params.get("y").and_then(Value::as_f64);
            type_at(port, t, x, y).await?
        }
        "scroll_at" => {
            let x = params.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = params.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let dir = params.get("direction").and_then(Value::as_str).unwrap_or("down");
            let amt = params.get("amount").and_then(Value::as_i64).unwrap_or(3) as i32;
            scroll_at(port, x, y, dir, amt).await?
        }
        "key_press" => {
            let k = params.get("key").and_then(Value::as_str).unwrap_or("Enter");
            key_press(port, k).await?
        }
        "drag" => {
            let sx = params.get("start_x").and_then(Value::as_f64).unwrap_or(0.0);
            let sy = params.get("start_y").and_then(Value::as_f64).unwrap_or(0.0);
            let ex = params.get("end_x").and_then(Value::as_f64).unwrap_or(0.0);
            let ey = params.get("end_y").and_then(Value::as_f64).unwrap_or(0.0);
            drag(port, sx, sy, ex, ey).await?
        }
        _ => return Err(format!("Unknown vision action: {}", action)),
    };

    Ok(BrowserAutomationResult {
        request_id: make_request_id(),
        browser_id: "default".to_string(),
        data: json!({ "result": text, "success": true }),
        message: Some(text),
    })
}
