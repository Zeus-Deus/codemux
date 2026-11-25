// A deliberately broken Chromium stand-in for the native pipe smoke gate.
// It advertises its PID, closes the inherited debugging pipes, then waits.
// The sidecar must fail the launch and reap this process.
use std::path::Path;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let profile = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--user-data-dir="))
        .expect("missing disposable profile");
    std::fs::write(Path::new(profile).join("fake-chrome.pid"), std::process::id().to_string())
        .expect("write fake Chrome PID");

    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn close(fd: i32) -> i32;
        }
        close(3);
        close(4);
    }

    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        let handles = args
            .iter()
            .find_map(|arg| arg.strip_prefix("--remote-debugging-io-pipes="))
            .expect("missing inherited pipe handles");
        for value in handles.split(',') {
            let handle = value.parse::<usize>().expect("numeric pipe handle");
            unsafe { CloseHandle(handle as *mut c_void) };
        }
    }

    std::thread::sleep(Duration::from_secs(30));
}
