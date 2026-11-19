//! Native Windows foreground handoff on an interactive desktop. The Tauri
//! fixture uses a private instance identifier and no Codemux profile or webview.

#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::ptr::null_mut;
use std::time::{Duration, Instant};
use tauri::Manager;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DestroyWindow, DispatchMessageW,
    GetForegroundWindow, PeekMessageW, SetForegroundWindow, ShowWindow, TranslateMessage,
    CW_USEDEFAULT, MSG, PM_REMOVE, SW_SHOW, WS_OVERLAPPEDWINDOW,
};

const TEST: &str = "duplicate_desktop_launch_transfers_foreground_permission";
const IDENTIFIER: &str = "CODEMUX_TEST_WINDOWS_INSTANCE_ID";
const ROLE: &str = "CODEMUX_TEST_WINDOWS_LAUNCH_ROLE";
const FOCUS_PID: &str = "CODEMUX_TEST_WINDOWS_FOCUS_PID";

struct Process(Child);

impl Process {
    fn start(root: &Path, identifier: &str, role: &str) -> Self {
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--ignored", "--nocapture"])
                .current_dir(root)
                .env(IDENTIFIER, identifier)
                .env(ROLE, role)
                .env(FOCUS_PID, std::process::id().to_string())
                .stdin(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .unwrap(),
        )
    }

    fn status(&mut self) -> Option<ExitStatus> {
        self.0.try_wait().unwrap()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct NativeWindow(HWND);

impl NativeWindow {
    fn new(title: &str) -> Self {
        let class: Vec<u16> = "STATIC".encode_utf16().chain([0]).collect();
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        // The built-in class needs no registration or application profile.
        // The strings are NUL-terminated and valid throughout creation.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                320,
                200,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        assert!(!window.is_null(), "could not create a fixture window");
        Self(window)
    }

    fn show_and_focus(&self) {
        // This HWND belongs to the calling thread and lives through both calls.
        unsafe {
            ShowWindow(self.0, SW_SHOW);
            SetForegroundWindow(self.0);
        }
    }

    fn is_foreground(&self) -> bool {
        // GetForegroundWindow only reads the current foreground handle.
        unsafe { GetForegroundWindow() == self.0 }
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        // The creating thread owns destruction of this HWND.
        unsafe { DestroyWindow(self.0) };
    }
}

fn eventually<T>(reason: &str, mut condition: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        // Service the test thread's native window while polling it. PeekMessage
        // fills each MSG before it is passed to Translate/DispatchMessage.
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if let Some(value) = condition() {
            return value;
        }
        assert!(Instant::now() < deadline, "{reason}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

// Tauri's managed state is thread-safe. The owning process retains the native
// window for the duration of its event loop, and the callback uses it on that thread.
struct PrimaryWindow(usize);

fn run_instance(role: String) {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().identifier = std::env::var(IDENTIFIER).unwrap();
    let focus_pid = std::env::var(FOCUS_PID).unwrap().parse::<u32>().unwrap();
    let app = tauri::Builder::default()
        .any_thread()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let window = app.state::<PrimaryWindow>().inner().0 as HWND;
            // Exercise the native foreground transfer directly, without a
            // window toolkit's simulated keyboard input fallback.
            unsafe { SetForegroundWindow(window) };
        }))
        .setup(move |app| {
            assert_eq!(role, "primary", "duplicate reached desktop setup");
            let window = NativeWindow::new("Codemux primary launch fixture");
            window.show_and_focus();
            eventually("primary could not take foreground", || {
                window.is_foreground().then_some(())
            });
            // Let the controller bring back its independently owned window.
            // The controller becomes the foreground process before creating
            // the duplicate, as when a user launches the app a second time.
            assert_ne!(unsafe { AllowSetForegroundWindow(focus_pid) }, 0);
            let handle = window.0 as usize;
            app.manage(PrimaryWindow(handle));
            std::mem::forget(window);
            std::fs::write("primary-window", handle.to_string())?;

            let app = app.handle().clone();
            std::thread::spawn(move || {
                eventually("foreground probe was not requested", || {
                    Path::new("probe-foreground").is_file().then_some(())
                });
                app.run_on_main_thread(move || {
                    // Check that the foreground lock is active on this desktop.
                    // Otherwise an old plugin can pass without transferring it.
                    let allowed = unsafe { SetForegroundWindow(handle as HWND) } != 0;
                    std::fs::write(
                        "foreground-probe",
                        if allowed { "allowed" } else { "denied" },
                    )
                    .unwrap();
                })
                .unwrap();
            });
            Ok(())
        })
        .build(context)
        .unwrap();
    app.run(|_, _| {});
}

#[test]
#[ignore = "requires an unlocked, idle Windows desktop and takes foreground focus"]
fn duplicate_desktop_launch_transfers_foreground_permission() {
    if let Ok(role) = std::env::var(ROLE) {
        run_instance(role);
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let identifier = format!("com.codemux.focus-fixture.{}", uuid::Uuid::new_v4());
    let other = NativeWindow::new("Independent foreground fixture");
    other.show_and_focus();
    eventually("an unlocked interactive desktop is required", || {
        other.is_foreground().then_some(())
    });

    let mut primary = Process::start(root.path(), &identifier, "primary");
    let primary_window = eventually("primary window did not initialize", || {
        assert!(primary.status().is_none(), "primary exited before setup");
        let value = std::fs::read_to_string(root.path().join("primary-window")).ok()?;
        Some(value.parse::<usize>().ok()? as HWND)
    });

    other.show_and_focus();
    eventually("independent window could not regain foreground", || {
        other.is_foreground().then_some(())
    });
    std::fs::write(root.path().join("probe-foreground"), "").unwrap();
    let probe = eventually("primary did not check the foreground policy", || {
        let status = std::fs::read_to_string(root.path().join("foreground-probe")).ok()?;
        matches!(status.as_str(), "allowed" | "denied").then_some(status)
    });
    assert_eq!(probe, "denied", "desktop allowed unassisted focus");
    assert!(
        other.is_foreground(),
        "fixture lost foreground before launch"
    );

    let mut duplicate = Process::start(root.path(), &identifier, "duplicate");
    let status = eventually("duplicate did not complete native forwarding", || {
        duplicate.status()
    });
    assert!(status.success(), "duplicate exited with {status}");
    eventually("duplicate did not focus the primary window", || {
        // The primary process remains alive and owns this handle.
        assert!(
            primary.status().is_none(),
            "primary exited during forwarding"
        );
        (unsafe { GetForegroundWindow() } == primary_window).then_some(())
    });
}
