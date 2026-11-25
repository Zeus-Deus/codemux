use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use codemux_lib::web_remote::connect::systemd_unit;

#[test]
fn systemd_keeps_percent_encoded_api_url_literal() {
    // systemd refuses --user --test as root. The unit string assertion in
    // the library still runs on hosts that cannot execute this parser test.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let Some(binary) = ["/usr/lib/systemd/systemd", "/lib/systemd/systemd"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
    else {
        return;
    };

    let url = "https://api.example.test/api%2Ftenant";
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("run");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        dir.path().join("codemux-api-env-probe.service"),
        systemd_unit("/usr/bin/env", Some(url)),
    )
    .unwrap();
    let output = Command::new(binary)
        .args(["--user", "--test", "--unit=codemux-api-env-probe.service"])
        .env("XDG_RUNTIME_DIR", runtime)
        .env("SYSTEMD_UNIT_PATH", format!("{}:", dir.path().display()))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "systemd could not parse the generated unit:\n{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == format!("Environment: CODEMUX_API_URL={url}")),
        "systemd changed the API URL while parsing the unit:\n{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}
