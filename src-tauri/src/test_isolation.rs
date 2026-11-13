//! Test helpers that keep tests away from the developer's own Codemux state.
use std::path::{Path, PathBuf};
use std::process::Command;

/// A test fixture binary (`fake_cursor_acp`, …) built next to the crate's
/// own binaries. Unit-test executables used to sit one level below them in
/// `target/<profile>/deps`; newer Cargo puts them deeper in the build
/// directory, so look for the fixture in each ancestor directory. Falls
/// back to the old location, so a missing fixture still names a path.
pub(crate) fn fixture_bin(name: impl AsRef<Path>) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join(name.as_ref()))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| exe.parent().unwrap().parent().unwrap().join(name.as_ref()))
}

/// Set only in the child process [`isolated`] starts: its private root.
const ROOT_ENV: &str = "CODEMUX_TEST_ISOLATED_ROOT";

/// The private root of an [`isolated`] child; `None` in a normal test run.
pub(crate) fn isolated_root() -> Option<PathBuf> {
    std::env::var_os(ROOT_ENV).map(PathBuf::from)
}

/// Re-runs the test at `test_path` (`module::tests::name`) in a new process
/// whose process-wide paths live in a fresh temporary root: worktree home and
/// layout file (test-only overrides that also work on Windows, where Known
/// Folders ignore HOME), HOME, the XDG directories, the temp dir and git's
/// global config. The paths are set only on a new process, because changing
/// the environment of a running test binary races its other threads.
///
/// Returns true in the parent once the child passed, so the test returns at
/// once; false in the child, which runs the body.
pub(crate) fn isolated(test_path: &str) -> bool {
    if let Some(root) = isolated_root() {
        assert_eq!(
            Path::new(&std::env::var_os("CODEMUX_TEST_WORKTREE_HOME").unwrap()),
            Path::new(&root)
        );
        assert!(
            Path::new(&std::env::var_os("CODEMUX_TEST_LAYOUT_PATH").unwrap()).starts_with(&root)
        );
        return false;
    }
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile");
    std::fs::create_dir(&profile).unwrap();
    // A private runtime dir: a child that dials the control socket must never
    // fall back to the developer's running app.
    let runtime = root.path().join("run");
    std::fs::create_dir(&runtime).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .current_dir(root.path())
        .args(["--exact", test_path, "--nocapture"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        // Keep HOME different so Unix runs exercise the explicit override too.
        .env("HOME", &profile)
        .env("USERPROFILE", &profile)
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("TMPDIR", root.path())
        .env("TEMP", root.path())
        .env("TMP", root.path())
        .env(ROOT_ENV, root.path())
        .env("CODEMUX_TEST_WORKTREE_HOME", root.path())
        .env("CODEMUX_TEST_LAYOUT_PATH", root.path().join("layout.json"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.path().join("gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Codemux test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Codemux test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
    if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
        command.env("LD_LIBRARY_PATH", path);
    }
    #[cfg(windows)]
    for key in ["SystemRoot", "WINDIR", "ComSpec", "PATHEXT"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "isolated {test_path}: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
