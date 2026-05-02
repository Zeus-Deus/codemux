use crate::agent_browser::BrowserAutomationResult;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub async fn handle_os_action(
    _action: &str,
    _params: serde_json::Value,
    _browser_id: &str,
) -> Result<BrowserAutomationResult, String> {
    Err("OS input not supported on this platform".to_string())
}

#[cfg(target_os = "linux")]
pub use linux_impl::handle_os_action;

#[cfg(target_os = "windows")]
pub use windows_impl::handle_os_action;

#[cfg(target_os = "linux")]
mod linux_impl {
use super::BrowserAutomationResult;
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

} // mod linux_impl

#[cfg(target_os = "windows")]
mod windows_impl {
    //! Tier-3 OS-level input injection on Windows.
    //!
    //! Mirrors the Linux Tier-3 implementation but driven by Win32 APIs
    //! instead of ydotool + hyprctl. The protocol is identical: receive a
    //! viewport-relative coordinate, locate the headed Chromium window via
    //! `EnumWindows` filtered by process image name, translate to screen
    //! coordinates accounting for the toolbar offset, run a randomized
    //! Bezier-path mouse approach, click via `SendInput`, optionally
    //! follow with synthesized keyboard input.
    //!
    //! No daemon dependency (`SendInput` is in user32 directly), no UAC
    //! elevation required for non-elevated targets — Windows lets a normal
    //! GUI process inject into other normal GUI processes by default.
    //! Inputs targeting an elevated window will silently no-op (UIPI),
    //! same trade-off ydotool has on Linux.
    use super::BrowserAutomationResult;
    use serde_json::{json, Value};
    use tokio::time::{sleep, Duration};
    use windows_sys::Win32::Foundation::{
        CloseHandle, BOOL, HANDLE, HWND, LPARAM, RECT, TRUE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
        KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEINPUT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SetCursorPos,
    };

    /// Browser chrome offset (toolbar height) for viewport→screen.
    /// Headless = 0; headed Chromium ≈ 88px on Windows (~13px more than
    /// the Linux number because Windows' default DPI + thicker title bar).
    /// Matches the Linux file's tunable; refine later if needed.
    const HEADED_CHROME_OFFSET_Y: f64 = 88.0;

    /// Window we found via EnumWindows.
    struct WindowGeometry {
        x: i32,
        y: i32,
    }

    /// Process names that count as a Chromium-based browser. Lowercase
    /// for case-insensitive matching against the basename of the process
    /// image path returned by `QueryFullProcessImageNameW`.
    const BROWSER_EXE_NAMES: &[&str] = &[
        "chrome.exe",
        "msedge.exe",
        "brave.exe",
        "vivaldi.exe",
        "opera.exe",
        "chromium.exe",
    ];

    /// EnumWindows callback context: we accumulate all candidate HWNDs
    /// then pick the topmost visible one outside the callback (the
    /// callback runs in a tight loop; we keep it cheap).
    struct EnumCtx {
        hwnds: Vec<HWND>,
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam as *mut EnumCtx);
        ctx.hwnds.push(hwnd);
        TRUE
    }

    /// Returns the lowercased basename of the EXE backing the process
    /// that owns `hwnd`, or None on lookup failure.
    fn process_image_name(hwnd: HWND) -> Option<String> {
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 {
            return None;
        }
        let handle: HANDLE =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let mut buf: [u16; 1024] = [0; 1024];
        let mut size: u32 = buf.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size)
        };
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        path.rsplit('\\').next().map(|s| s.to_lowercase())
    }

    /// Find a visible top-level window whose owning process is one of
    /// our known Chromium-based browsers. Equivalent to the Linux side's
    /// `find_browser_window` via `hyprctl clients -j`.
    fn find_browser_window() -> Result<(HWND, WindowGeometry), String> {
        let mut ctx = EnumCtx { hwnds: Vec::new() };
        let ok = unsafe {
            EnumWindows(
                Some(enum_windows_proc),
                &mut ctx as *mut EnumCtx as LPARAM,
            )
        };
        if ok == 0 && ctx.hwnds.is_empty() {
            return Err("EnumWindows returned no windows".into());
        }

        for hwnd in ctx.hwnds {
            if unsafe { IsWindowVisible(hwnd) } == 0 {
                continue;
            }
            let exe = match process_image_name(hwnd) {
                Some(name) => name,
                None => continue,
            };
            if !BROWSER_EXE_NAMES.iter().any(|n| *n == exe) {
                continue;
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
                continue;
            }
            // Skip zero-size windows (some hidden / system).
            if rect.right - rect.left <= 1 || rect.bottom - rect.top <= 1 {
                continue;
            }
            return Ok((
                hwnd,
                WindowGeometry {
                    x: rect.left,
                    y: rect.top,
                },
            ));
        }
        Err(
            "No Chromium-based browser window found. Open the browser in headed mode (not \
             headless) before requesting OS-level input."
                .to_string(),
        )
    }

    /// Move the system cursor to absolute screen coordinates. SetCursorPos
    /// is sufficient — no SendInput/MOUSEEVENTF_MOVE needed because
    /// Windows treats SetCursorPos as a real cursor move event for input
    /// purposes (Chromium's mouse-event handling sees it).
    fn cursor_move(x: i32, y: i32) -> Result<(), String> {
        if unsafe { SetCursorPos(x, y) } == 0 {
            return Err(format!("SetCursorPos({x}, {y}) failed"));
        }
        Ok(())
    }

    /// Synthesize a left-button click via SendInput (down + up).
    fn click_left() -> Result<(), String> {
        let mut inputs: [INPUT; 2] = unsafe { std::mem::zeroed() };
        inputs[0].r#type = INPUT_MOUSE;
        inputs[0].Anonymous = INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_LEFTDOWN,
                time: 0,
                dwExtraInfo: 0,
            },
        };
        inputs[1].r#type = INPUT_MOUSE;
        inputs[1].Anonymous = INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_LEFTUP,
                time: 0,
                dwExtraInfo: 0,
            },
        };
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
        if sent != inputs.len() as u32 {
            return Err(format!(
                "SendInput sent {sent}/{} mouse events (likely UIPI block)",
                inputs.len()
            ));
        }
        Ok(())
    }

    /// Type a string via SendInput with KEYEVENTF_UNICODE so we don't have
    /// to map code points to virtual-key codes ourselves. Each char is a
    /// single 16-bit code unit (codepoints above the BMP are emitted as a
    /// surrogate pair, which Windows handles natively for KEYEVENTF_UNICODE).
    async fn type_text(text: &str, key_delay_ms: u64) -> Result<(), String> {
        for ch in text.encode_utf16() {
            let mut inputs: [INPUT; 2] = unsafe { std::mem::zeroed() };
            // KEYDOWN (Unicode)
            inputs[0].r#type = INPUT_KEYBOARD;
            inputs[0].Anonymous = INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: ch,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            };
            // KEYUP (Unicode)
            inputs[1].r#type = INPUT_KEYBOARD;
            inputs[1].Anonymous = INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: ch,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            };
            let sent = unsafe {
                SendInput(
                    inputs.len() as u32,
                    inputs.as_ptr(),
                    std::mem::size_of::<INPUT>() as i32,
                )
            };
            if sent != inputs.len() as u32 {
                return Err(format!(
                    "SendInput sent {sent}/2 key events (likely UIPI block)"
                ));
            }
            if key_delay_ms > 0 {
                sleep(Duration::from_millis(key_delay_ms)).await;
            }
        }
        Ok(())
    }

    /// Higher-level click: viewport→screen coord conversion + Bezier mouse
    /// path with randomized delays. Mirrors `linux_impl::os_click`.
    async fn os_click(viewport_x: f64, viewport_y: f64) -> Result<String, String> {
        let (_hwnd, geom) = find_browser_window()?;

        let screen_x = geom.x as f64 + viewport_x;
        let screen_y = geom.y as f64 + HEADED_CHROME_OFFSET_Y + viewport_y;

        // Pre-compute random values before .await (thread_rng is !Send).
        let (points, delays, pause) = {
            let rx = rand::random::<f64>();
            let ry = rand::random::<f64>();
            let start_x = (screen_x - 120.0 + rx * 60.0).max(0.0);
            let start_y = (screen_y - 90.0 + ry * 40.0).max(0.0);
            let pts = crate::stream_input::generate_bezier_points(
                (start_x, start_y),
                (screen_x, screen_y),
                5,
            );
            let dls: Vec<u64> = (0..pts.len())
                .map(|_| 15 + rand::random::<u64>() % 20)
                .collect();
            let p = 50 + rand::random::<u64>() % 100;
            (pts, dls, p)
        };

        for (i, (px, py)) in points.iter().enumerate() {
            cursor_move(*px as i32, *py as i32)?;
            sleep(Duration::from_millis(delays[i])).await;
        }

        cursor_move(screen_x as i32, screen_y as i32)?;
        sleep(Duration::from_millis(pause)).await;

        click_left()?;

        Ok(format!(
            "OS click at viewport ({viewport_x}, {viewport_y}) → screen ({}, {})",
            screen_x as i32, screen_y as i32
        ))
    }

    async fn os_type(text: &str, x: Option<f64>, y: Option<f64>) -> Result<String, String> {
        if let (Some(vx), Some(vy)) = (x, y) {
            os_click(vx, vy).await?;
            sleep(Duration::from_millis(150)).await;
        }
        type_text(text, 50).await?;
        Ok(format!("OS typed {} characters", text.len()))
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

    pub async fn handle_os_action(
        action: &str,
        params: Value,
        browser_id: &str,
    ) -> Result<BrowserAutomationResult, String> {
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
} // mod windows_impl
