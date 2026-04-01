use crate::agent_browser::BrowserAutomationResult;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::{sleep, Duration};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

struct WindowGeometry {
    x: i32,
    y: i32,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
}

#[derive(Deserialize)]
struct HyprClient {
    address: String,
    at: [i32; 2],
    size: [u32; 2],
    #[allow(dead_code)]
    pid: u32,
    #[allow(dead_code)]
    title: String,
    class: String,
}

// ---------------------------------------------------------------------------
// ydotool availability
// ---------------------------------------------------------------------------

async fn is_ydotool_available() -> bool {
    let bin_ok = Command::new("which")
        .arg("ydotool")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !bin_ok {
        return false;
    }
    // Check if ydotoold daemon is running (try the service first, then a probe)
    let daemon_ok = Command::new("systemctl")
        .args(["--user", "is-active", "ydotool"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if daemon_ok {
        return true;
    }
    // Probe: try a no-op mouse move to see if ydotool works
    Command::new("ydotool")
        .args(["mousemove", "-a", "0", "0"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Window geometry via hyprctl
// ---------------------------------------------------------------------------

async fn find_browser_window() -> Result<(String, WindowGeometry), String> {
    let output = Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .await
        .map_err(|e| format!("Failed to run hyprctl: {}. Is Hyprland running?", e))?;

    if !output.status.success() {
        return Err("hyprctl clients failed".to_string());
    }

    let clients: Vec<HyprClient> = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse hyprctl output: {}", e))?;

    let client = clients
        .iter()
        .find(|c| {
            let cls = c.class.to_lowercase();
            cls.contains("chrom") || cls.contains("brave")
        })
        .ok_or_else(|| {
            "No browser window found via hyprctl. Is the browser running in headed mode (not headless)?".to_string()
        })?;

    Ok((
        client.address.clone(),
        WindowGeometry {
            x: client.at[0],
            y: client.at[1],
            width: client.size[0],
            height: client.size[1],
        },
    ))
}

// ---------------------------------------------------------------------------
// ydotool primitives
// ---------------------------------------------------------------------------

async fn ydotool_move(x: i64, y: i64) -> Result<(), String> {
    let out = Command::new("ydotool")
        .args(["mousemove", "-a", &x.to_string(), &y.to_string()])
        .output()
        .await
        .map_err(|e| format!("ydotool mousemove failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("ydotool mousemove error: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

async fn ydotool_click_left() -> Result<(), String> {
    // 0xC0 = click (press+release) button 0 (left)
    let out = Command::new("ydotool")
        .args(["click", "0xC0"])
        .output()
        .await
        .map_err(|e| format!("ydotool click failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("ydotool click error: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

async fn ydotool_type_text(text: &str, delay_ms: u64) -> Result<(), String> {
    let out = Command::new("ydotool")
        .args(["type", "--key-delay", &delay_ms.to_string(), "--", text])
        .output()
        .await
        .map_err(|e| format!("ydotool type failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("ydotool type error: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Kernel keycode mapping (linux/input-event-codes.h)
// ---------------------------------------------------------------------------

fn linux_keycode(name: &str) -> Result<u32, String> {
    Ok(match name.to_lowercase().as_str() {
        "a" => 30, "b" => 48, "c" => 46, "d" => 32, "e" => 18, "f" => 33,
        "g" => 34, "h" => 35, "i" => 23, "j" => 36, "k" => 37, "l" => 38,
        "m" => 50, "n" => 49, "o" => 24, "p" => 25, "q" => 16, "r" => 19,
        "s" => 31, "t" => 20, "u" => 22, "v" => 47, "w" => 17, "x" => 45,
        "y" => 21, "z" => 44,
        "0" => 11, "1" => 2, "2" => 3, "3" => 4, "4" => 5,
        "5" => 6, "6" => 7, "7" => 8, "8" => 9, "9" => 10,
        "return" | "enter" => 28,
        "escape" | "esc" => 1,
        "tab" => 15,
        "backspace" => 14,
        "space" | " " => 57,
        "delete" => 111,
        "home" => 102,
        "end" => 107,
        "pageup" => 104,
        "pagedown" => 109,
        "up" | "arrowup" => 103,
        "down" | "arrowdown" => 108,
        "left" | "arrowleft" => 105,
        "right" | "arrowright" => 106,
        "ctrl" | "control" => 29,
        "shift" => 42,
        "alt" => 56,
        "meta" | "super" | "cmd" => 125,
        _ => return Err(format!("Unknown key for ydotool: {}", name)),
    })
}

async fn ydotool_key(key: &str) -> Result<(), String> {
    let parts: Vec<&str> = key.split('+').collect();
    let mut args = Vec::new();

    if parts.len() == 1 {
        let code = linux_keycode(parts[0])?;
        args.push(format!("{}:1", code));
        args.push(format!("{}:0", code));
    } else {
        let modifiers = &parts[..parts.len() - 1];
        let main = parts[parts.len() - 1];
        for m in modifiers {
            args.push(format!("{}:1", linux_keycode(m)?));
        }
        let mc = linux_keycode(main)?;
        args.push(format!("{}:1", mc));
        args.push(format!("{}:0", mc));
        for m in modifiers.iter().rev() {
            args.push(format!("{}:0", linux_keycode(m)?));
        }
    }

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut cmd_args = vec!["key"];
    cmd_args.extend(arg_refs);

    let out = Command::new("ydotool")
        .args(&cmd_args)
        .output()
        .await
        .map_err(|e| format!("ydotool key failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("ydotool key error: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// High-level OS click with Bezier movement
// ---------------------------------------------------------------------------

/// Browser chrome offset (toolbar height) for viewport → screen coordinate conversion.
/// Headless = 0. Headed Chrome ≈ 75px. Can be refined later.
const HEADED_CHROME_OFFSET_Y: f64 = 75.0;

async fn os_click(viewport_x: f64, viewport_y: f64) -> Result<String, String> {
    let (_addr, geom) = find_browser_window().await?;

    let screen_x = geom.x as f64 + viewport_x;
    let screen_y = geom.y as f64 + HEADED_CHROME_OFFSET_Y + viewport_y;

    // Pre-compute random values before .await (thread_rng is !Send)
    let (points, delays, pause) = {
        let rx = rand::random::<f64>();
        let ry = rand::random::<f64>();
        let start_x = (screen_x - 120.0 + rx * 60.0).max(0.0);
        let start_y = (screen_y - 90.0 + ry * 40.0).max(0.0);
        let pts = crate::stream_input::generate_bezier_points((start_x, start_y), (screen_x, screen_y), 5);
        let dls: Vec<u64> = (0..pts.len()).map(|_| 15 + rand::random::<u64>() % 20).collect();
        let p = 50 + rand::random::<u64>() % 100;
        (pts, dls, p)
    };

    for (i, (px, py)) in points.iter().enumerate() {
        ydotool_move(*px as i64, *py as i64).await?;
        sleep(Duration::from_millis(delays[i])).await;
    }

    ydotool_move(screen_x as i64, screen_y as i64).await?;
    sleep(Duration::from_millis(pause)).await;

    ydotool_click_left().await?;

    Ok(format!(
        "OS click at viewport ({}, {}) → screen ({}, {})",
        viewport_x, viewport_y, screen_x as i64, screen_y as i64
    ))
}

async fn os_type(text: &str, x: Option<f64>, y: Option<f64>) -> Result<String, String> {
    // Click at position first if coordinates provided
    if let (Some(vx), Some(vy)) = (x, y) {
        os_click(vx, vy).await?;
        sleep(Duration::from_millis(150)).await;
    }
    ydotool_type_text(text, 50).await?;
    Ok(format!("OS typed {} characters", text.len()))
}

// ---------------------------------------------------------------------------
// Unified dispatcher
// ---------------------------------------------------------------------------

fn make_request_id() -> String {
    format!("req-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
}

pub async fn handle_os_action(action: &str, params: Value, browser_id: &str) -> Result<BrowserAutomationResult, String> {
    // Check ydotool availability
    if !is_ydotool_available().await {
        return Err(
            "OS-level input not available. Install ydotool with your system package manager \
             and ensure ydotoold is running (e.g. 'systemctl --user enable --now ydotool'). \
             The browser must also be running in headed mode (not headless)."
                .to_string(),
        );
    }

    let text = match action {
        "click_os" => {
            let x = params.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = params.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            os_click(x, y).await?
        }
        "type_os" => {
            let t = params.get("text").and_then(Value::as_str).unwrap_or("");
            let x = params.get("x").and_then(Value::as_f64);
            let y = params.get("y").and_then(Value::as_f64);
            os_type(t, x, y).await?
        }
        _ => return Err(format!("Unknown OS action: {}", action)),
    };

    Ok(BrowserAutomationResult {
        request_id: make_request_id(),
        browser_id: browser_id.to_string(),
        data: json!({ "result": text, "success": true }),
        message: Some(text),
    })
}
