//! Private stream-local socket paths for the two ends of an SSH tunnel.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use super::exec::{sh_quote, validate_ssh_target};

/// Leave room for the NUL on platforms with a 104-byte sun_path.
const MAX_SOCKET_PATH_BYTES: usize = 103;
pub(super) const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_DISCOVERY_OUTPUT: u64 = 16 * 1024;
const PATH_MARKER: &str = "codemux:pty-path:v1 ";
const SOCKET_ROOT_UNAVAILABLE: &str =
    "The device has no usable private workspace socket directory or startup path. Check its socket, PID, and log entries under the SSH home, runtime, and temporary directories.";
const LOCAL_SOCKET_FILE: &str = "pty.sock";

#[derive(Clone, Copy)]
enum SocketDiscovery {
    Prepare,
    Probe,
}

/// OpenSSH splits forwarding arguments at ':' and expands '%' and
/// environment tokens. Use a path that is passed through literally.
pub(super) fn forward_socket_path(path: &Path) -> bool {
    let Some(path) = path.to_str() else {
        return false;
    };
    path.starts_with('/')
        && path.len() <= MAX_SOCKET_PATH_BYTES
        && path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/._-".contains(&c))
        && !path.split('/').any(|part| part == "." || part == "..")
}

pub(super) fn is_private_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o777 == 0o700
    })
}

fn has_trusted_ancestors(path: &Path) -> bool {
    let uid = unsafe { libc::geteuid() };
    path.ancestors().all(|dir| {
        fs::symlink_metadata(dir).is_ok_and(|metadata| {
            let mode = metadata.mode();
            metadata.is_dir()
                && (metadata.uid() == 0 || metadata.uid() == uid)
                && (mode & 0o022 == 0 || mode & 0o1000 != 0)
        })
    })
}

pub(super) struct LocalSocket {
    // Replacing a supervisor cannot unlink the new supervisor's socket.
    _directory: TempDir,
    pub path: PathBuf,
}

impl LocalSocket {
    pub(super) fn allocate() -> Result<Self, String> {
        Self::allocate_under(
            std::env::var_os("XDG_RUNTIME_DIR")
                .as_deref()
                .map(Path::new),
        )
    }

    fn allocate_under(runtime: Option<&Path>) -> Result<Self, String> {
        let private = runtime
            .filter(|path| path.is_absolute() && is_private_directory(path))
            .and_then(|path| fs::canonicalize(path).ok())
            .filter(|path| has_trusted_ancestors(path));
        // The system temp root keeps the fallback short even when TMPDIR
        // or the user's home path would exceed the Unix socket limit.
        let system_tmp = fs::canonicalize("/tmp")
            .ok()
            .filter(|path| has_trusted_ancestors(path));
        for base in private.iter().chain(system_tmp.iter()) {
            if !forward_socket_path(&base.join("cmux.01234567").join(LOCAL_SOCKET_FILE)) {
                continue;
            }
            let Ok(directory) = tempfile::Builder::new()
                .prefix("cmux.")
                .rand_bytes(8)
                .permissions(fs::Permissions::from_mode(0o700))
                .tempdir_in(base)
            else {
                continue;
            };
            let path = directory.path().join(LOCAL_SOCKET_FILE);
            if is_private_directory(directory.path()) && forward_socket_path(&path) {
                return Ok(Self {
                    _directory: directory,
                    path,
                });
            }
        }
        Err("Could not allocate a short, private SSH tunnel socket".into())
    }
}

fn workspace_socket_key(workspace_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    workspace_id.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub(super) fn verified_ssh_args() -> [&'static str; 9] {
    [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ControlPath=none",
    ]
}

/// Discovery and tunnel setup recheck directory ownership independently.
pub(super) const DIRECTORY_CHECKS: &str = r#"
mode() { stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1" 2>/dev/null; }
owner() { stat -c %u "$1" 2>/dev/null || stat -f %u "$1" 2>/dev/null; }
absolute_dir() {
    case "$1" in /*) (CDPATH='' cd -P "$1" && pwd -P);; *) return 1;; esac
}
trusted_directory() {
    [ "$(absolute_dir "$1" 2>/dev/null)" = "$1" ] || return 1
    candidate=$1
    while :; do
        candidate_owner=$(owner "$candidate") || return 1
        case "$candidate_owner" in 0|"$UID_NUMBER") ;; *) return 1;; esac
        candidate_mode=$(mode "$candidate") || return 1
        case "$candidate_mode" in
            [1357][0-7][0-7][0-7]) ;;
            *[2367][0-7]|*[0-7][2367]) return 1;;
            *) ;;
        esac
        [ "$candidate" = / ] && break
        candidate=${candidate%/*}
        [ -n "$candidate" ] || candidate=/
    done
}
private_directory() {
    [ -d "$1" ] && [ -O "$1" ] && [ "$(mode "$1")" = 700 ] && trusted_directory "$1"
}
daemon_alive() {
    [ -S "$1" ] && [ ! -L "$1" ] && [ -O "$1" ] && [ "$(mode "$1")" = 600 ] || return 1
    [ -f "$1.pid" ] && [ ! -L "$1.pid" ] && [ -O "$1.pid" ] || return 1
    case "$(mode "$1.pid")" in 600|640|644) ;; *) return 1 ;; esac
    pid=$(cat "$1.pid") || return 1
    case "$pid" in ''|0|*[!0-9]*) return 1 ;; esac
    kill -0 "$pid" 2>/dev/null
}
usable_socket_file() {
    [ ! -L "$1" ] && { [ ! -e "$1" ] || { [ -f "$1" ] && [ -O "$1" ] && [ -w "$1" ]; }; }
}
usable_socket_start_paths() {
    # rm -f can unlink a symlink itself, including a symlink to a directory.
    { [ ! -d "$1" ] || [ -L "$1" ]; } &&
        { [ ! -d "$1.pid" ] || [ -L "$1.pid" ]; } &&
        usable_socket_file "$1.log"
}
lock_socket_path() {
    # This file lives in an already verified private directory and is never
    # unlinked. The kernel releases its lock when the last descriptor closes.
    usable_socket_file "$1" || return 1
    exec 9>>"$1" || return 1
    if command -v flock >/dev/null 2>&1; then
        flock -x -w 3 9
    elif command -v perl >/dev/null 2>&1; then
        # macOS provides Perl's flock even without a flock executable.
        perl -MFcntl=:flock -e '
            open(my $lock, "<&=9") or exit 1;
            alarm 3;
            flock($lock, LOCK_EX) or exit 1;
        '
    else
        printf '%s\n' 'Codemux SSH tunnels need flock or Perl on the remote host' >&2
        return 1
    fi
}
unlock_socket_path() {
    exec 9>&-
}
"#;

fn discovery_command(key: &str, system_tmp: &Path, discovery: SocketDiscovery) -> String {
    let script = format!(
        "umask 077\nexport LC_ALL=C\nUID_NUMBER=$(id -u) || exit 1\nKEY={}\nTMP_ROOT={}\nPROBE_ONLY={}\n{}{}",
        sh_quote(key),
        sh_quote(&system_tmp.to_string_lossy()),
        u8::from(matches!(discovery, SocketDiscovery::Probe)),
        DIRECTORY_CHECKS,
        r#"
HOME_ROOT=$(absolute_dir "$HOME" 2>/dev/null) || HOME_ROOT=
RUNTIME=$(absolute_dir "${XDG_RUNTIME_DIR:-}" 2>/dev/null) || RUNTIME=
TMP_ROOT=$(absolute_dir "$TMP_ROOT" 2>/dev/null) || TMP_ROOT=
if ! trusted_directory "$TMP_ROOT"; then TMP_ROOT=; fi
report_socket() {
    printf 'codemux:pty-path:v1 %s\n' "$1"
}
unusable_root() {
    report_socket unavailable
    exit 0
}
state_candidate() {
    STATE="$1/.cmux-tunnels"
    if [ ! -e "$STATE" ] && [ ! -L "$STATE" ] && [ -w "$1" ]; then
        # In probe mode verify the parent without creating its cold state.
        [ "$PROBE_ONLY" = 0 ] || return 0
        mkdir -m 700 "$STATE" 2>/dev/null || :
    fi
    private_directory "$STATE" && [ -w "$STATE" ]
}
select_state() {
    if [ -n "$HOME_ROOT" ] && [ -O "$HOME_ROOT" ] && trusted_directory "$HOME_ROOT" &&
        state_candidate "$HOME_ROOT"; then
        return 0
    fi
    [ -n "$RUNTIME" ] && private_directory "$RUNTIME" && state_candidate "$RUNTIME"
}
socket_fits() {
    case "$1/$KEY.sock" in /*[!A-Za-z0-9_./-]*) return 1;; /*) ;; *) return 1;; esac
    [ "${#1}" -le "$((103 - ${#KEY} - 6))" ]
}
select_state || unusable_root
MARKER="$STATE/$KEY.root"
LOCK="$STATE/$KEY.lock.fd"
ROOT=
if [ "$PROBE_ONLY" = 1 ]; then
    usable_socket_file "$LOCK" || unusable_root
    command -v flock >/dev/null 2>&1 || command -v perl >/dev/null 2>&1 || unusable_root
else
    lock_socket_path "$LOCK" || exit 1
fi
if [ -f "$MARKER" ] && [ ! -L "$MARKER" ]; then
    ROOT=$(cat "$MARKER") || ROOT=
fi
# An active legacy listener must be renamed on the same filesystem.
OLD=
if [ -n "$TMP_ROOT" ]; then
    OLD="$TMP_ROOT/codemux-ptyd-$KEY.d/pty.sock"
    if ! private_directory "$(dirname "$OLD")" || ! daemon_alive "$OLD"; then
        OLD="$TMP_ROOT/codemux-ptyd-$KEY.sock"
        if ! daemon_alive "$OLD"; then OLD=; fi
    fi
fi
if [ -z "$ROOT" ] || ! private_directory "$ROOT" || [ ! -w "$ROOT" ] || ! socket_fits "$ROOT"; then
    if [ -n "$OLD" ] || ! socket_fits "$STATE"; then
        [ -n "$TMP_ROOT" ] && [ -w "$TMP_ROOT" ] &&
            socket_fits "$TMP_ROOT/cmux.XXXXXXXX" || unusable_root
        if [ "$PROBE_ONLY" = 1 ]; then
            report_socket "${OLD:-absent}"
            exit 0
        fi
        ROOT=$(mktemp -d "$TMP_ROOT/cmux.XXXXXXXX") || exit 1
    else
        # A detached daemon may outlast an SSH login session.
        ROOT=$STATE
    fi
    if [ "$PROBE_ONLY" = 0 ]; then
        private_directory "$ROOT" && socket_fits "$ROOT" || exit 1
        # Recording the root before renaming lets the next attempt recover if
        # interrupted. With a live legacy socket, discovery retries migration.
        NEXT=$(mktemp "$STATE/.root.XXXXXXXX") || exit 1
        printf '%s\n' "$ROOT" > "$NEXT" && mv -f "$NEXT" "$MARKER" || exit 1
    fi
fi
SOCKET="$ROOT/$KEY.sock"
usable_socket_file "$SOCKET.start.fd" || unusable_root
case "$ROOT" in
    "$TMP_ROOT"/cmux.*)
        if [ -n "$OLD" ] && ! daemon_alive "$SOCKET"; then
            [ ! -e "$SOCKET" ] && [ ! -L "$SOCKET" ] || unusable_root
            usable_socket_file "$SOCKET.pid" || unusable_root
            if [ "$PROBE_ONLY" = 1 ]; then
                report_socket "$OLD"
                exit 0
            fi
            cp "$OLD.pid" "$SOCKET.pid" && mv "$OLD" "$SOCKET" || exit 1
        fi
        ;;
esac
if ! daemon_alive "$SOCKET"; then
    usable_socket_start_paths "$SOCKET" || unusable_root
    if [ "$PROBE_ONLY" = 1 ]; then
        report_socket absent
        exit 0
    fi
fi
if [ "$PROBE_ONLY" = 0 ]; then
    unlock_socket_path || exit 1
fi
report_socket "$SOCKET"
"#,
    );
    format!("exec sh -c {}", sh_quote(&script))
}

fn parse_discovered_socket(
    output: &[u8],
    key: &str,
    discovery: SocketDiscovery,
) -> Result<Option<String>, String> {
    let output = std::str::from_utf8(output).map_err(|_| "Invalid SSH discovery output")?;
    let mut paths = output
        .lines()
        .filter_map(|line| line.strip_prefix(PATH_MARKER));
    let path = paths
        .next()
        .ok_or("SSH did not report a private socket path")?;
    if paths.next().is_some() {
        return Err("SSH reported ambiguous socket paths".into());
    }
    if path == "unavailable" {
        return Err(SOCKET_ROOT_UNAVAILABLE.into());
    }
    let probing = matches!(discovery, SocketDiscovery::Probe);
    if probing && path == "absent" {
        return Ok(None);
    }
    let socket = Path::new(path);
    let current = socket.file_name().and_then(|name| name.to_str())
        == Some(format!("{key}.sock").as_str());
    let legacy = probing
        && (socket.ends_with(format!("codemux-ptyd-{key}.sock"))
            || socket.ends_with(format!("codemux-ptyd-{key}.d/pty.sock")));
    if !forward_socket_path(socket) || !(current || legacy) {
        return Err("SSH reported an invalid private socket path".into());
    }
    Ok(Some(path.to_owned()))
}

pub(super) async fn discover_remote_socket(
    ssh_target: &str,
    workspace_id: &str,
) -> Result<String, String> {
    find_remote_socket(ssh_target, workspace_id, SocketDiscovery::Prepare)
        .await?
        .ok_or_else(|| "SSH did not prepare a private socket path".into())
}

pub(super) async fn probe_remote_socket(
    ssh_target: &str,
    workspace_id: &str,
) -> Result<Option<String>, String> {
    find_remote_socket(ssh_target, workspace_id, SocketDiscovery::Probe).await
}

async fn find_remote_socket(
    ssh_target: &str,
    workspace_id: &str,
    discovery: SocketDiscovery,
) -> Result<Option<String>, String> {
    validate_ssh_target(ssh_target)?;
    let key = workspace_socket_key(workspace_id);
    let mut cmd = Command::new("ssh");
    cmd.args(verified_ssh_args())
        .args([
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "ConnectTimeout=6",
            "--",
            ssh_target,
        ])
        .arg(discovery_command(&key, Path::new("/tmp"), discovery))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    crate::execution::sanitize_appimage_env_tokio(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("Could not run SSH socket discovery: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("Missing SSH discovery output pipe")?
        .take(MAX_DISCOVERY_OUTPUT + 1);
    let mut output = Vec::new();
    let status = timeout(DISCOVERY_TIMEOUT, async {
        stdout.read_to_end(&mut output).await?;
        if output.len() as u64 > MAX_DISCOVERY_OUTPUT {
            return Err(std::io::Error::other("output limit exceeded"));
        }
        child.wait().await
    })
    .await
    .map_err(|_| "SSH socket discovery timed out")?
    .map_err(|error| format!("SSH socket discovery failed: {error}"))?;
    if !status.success() {
        return Err(format!(
            "SSH could not discover a private remote socket ({status})"
        ));
    }
    parse_discovered_socket(&output, &key, discovery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::{UnixListener, UnixStream};

    const WORKSPACE: &str = "abc123";

    fn private_temp() -> TempDir {
        tempfile::Builder::new()
            .prefix("cmux-path-test.")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap()
    }

    #[test]
    fn local_allocations_prefer_a_verified_runtime_root_and_are_unique() {
        let runtime = private_temp();
        let canonical = runtime.path().canonicalize().unwrap();
        let first = LocalSocket::allocate_under(Some(runtime.path())).unwrap();
        let second = LocalSocket::allocate_under(Some(runtime.path())).unwrap();
        assert!(first.path.starts_with(&canonical));
        assert!(second.path.starts_with(&canonical));
        assert_ne!(first.path, second.path);
        assert!(is_private_directory(first.path.parent().unwrap()));
    }

    #[test]
    fn local_fallback_avoids_an_insecure_or_overlong_runtime_path() {
        let runtime = private_temp();
        let long = runtime.path().join("x".repeat(100));
        fs::create_dir(&long).unwrap();
        fs::set_permissions(&long, fs::Permissions::from_mode(0o700)).unwrap();
        let overlong = LocalSocket::allocate_under(Some(&long)).unwrap();
        assert!(!overlong.path.starts_with(&long));
        assert!(forward_socket_path(&overlong.path));

        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let insecure = LocalSocket::allocate_under(Some(runtime.path())).unwrap();
        assert!(!insecure
            .path
            .starts_with(runtime.path().canonicalize().unwrap()));
        assert!(is_private_directory(insecure.path.parent().unwrap()));
    }

    fn run_discovery(home: &Path, runtime: Option<&Path>, tmp: &Path) -> Result<PathBuf, String> {
        run_socket_search(home, runtime, tmp, SocketDiscovery::Prepare)?
            .ok_or_else(|| "Fixture reported no socket path".into())
    }

    fn run_socket_search(
        home: &Path,
        runtime: Option<&Path>,
        tmp: &Path,
        discovery: SocketDiscovery,
    ) -> Result<Option<PathBuf>, String> {
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &discovery_command(WORKSPACE, tmp, discovery)])
            .env("HOME", home)
            .env("XDG_RUNTIME_DIR", runtime.unwrap_or_else(|| Path::new("")))
            .output()
            .unwrap();
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        parse_discovered_socket(&output.stdout, WORKSPACE, discovery)
            .map(|path| path.map(PathBuf::from))
    }

    #[test]
    fn remote_home_socket_survives_runtime_disappearance_and_ignores_claimed_tmp_path() {
        let home = private_temp();
        let runtime = private_temp();
        let tmp = private_temp();
        let claimed = tmp.path().join(format!("codemux-ptyd-{WORKSPACE}.d"));
        fs::create_dir(&claimed).unwrap();
        fs::set_permissions(&claimed, fs::Permissions::from_mode(0o777)).unwrap();
        let before = run_discovery(home.path(), Some(runtime.path()), tmp.path()).unwrap();
        let after = run_discovery(home.path(), None, tmp.path()).unwrap();
        assert_eq!(before, after);
        assert!(after.starts_with(home.path().canonicalize().unwrap().join(".cmux-tunnels")));
        assert_eq!(fs::metadata(claimed).unwrap().mode() & 0o777, 0o777);
    }

    #[test]
    fn remote_long_home_reuses_a_short_private_fallback() {
        let home = private_temp();
        let long = home.path().join("x".repeat(100));
        fs::create_dir(&long).unwrap();
        let tmp = private_temp();
        let first = run_discovery(&long, None, tmp.path()).unwrap();
        let second = run_discovery(&long, None, tmp.path()).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(tmp.path().canonicalize().unwrap()));
        assert!(is_private_directory(first.parent().unwrap()));
        assert!(forward_socket_path(&first));
    }

    #[test]
    fn cold_probe_rejects_unusable_state_or_uses_the_same_runtime_fallback_as_prepare() {
        let home = private_temp();
        let runtime = private_temp();
        let tmp = private_temp();
        let home_state = home.path().join(".cmux-tunnels");
        let runtime_state = runtime.path().join(".cmux-tunnels");
        fs::DirBuilder::new().create(&home_state).unwrap();
        fs::set_permissions(&home_state, fs::Permissions::from_mode(0o755)).unwrap();

        for discovery in [SocketDiscovery::Probe, SocketDiscovery::Prepare] {
            let error = run_socket_search(home.path(), None, tmp.path(), discovery).unwrap_err();
            assert_eq!(error, SOCKET_ROOT_UNAVAILABLE);
        }
        assert_eq!(
            run_socket_search(home.path(), Some(runtime.path()), tmp.path(), SocketDiscovery::Probe)
                .unwrap(),
            None,
        );
        assert!(!runtime_state.exists(), "a cold probe must not create state");
        let socket = run_discovery(home.path(), Some(runtime.path()), tmp.path()).unwrap();
        assert_eq!(socket.parent(), Some(runtime_state.canonicalize().unwrap().as_path()));
        assert_eq!(fs::metadata(home_state).unwrap().mode() & 0o777, 0o755);

        fs::set_permissions(&runtime_state, fs::Permissions::from_mode(0o755)).unwrap();
        for discovery in [SocketDiscovery::Probe, SocketDiscovery::Prepare] {
            let error = run_socket_search(
                home.path(), Some(runtime.path()), tmp.path(), discovery,
            ).unwrap_err();
            assert_eq!(error, SOCKET_ROOT_UNAVAILABLE);
        }
    }

    #[test]
    fn cold_probe_requires_a_short_usable_root_for_long_state_paths() {
        let fixture = private_temp();
        let long_home = fixture.path().join("h".repeat(100));
        let long_tmp = fixture.path().join("t".repeat(100));
        fs::create_dir(&long_home).unwrap();
        fs::create_dir(&long_tmp).unwrap();
        assert_eq!(
            run_socket_search(&long_home, None, &long_tmp, SocketDiscovery::Probe).unwrap_err(),
            SOCKET_ROOT_UNAVAILABLE,
        );
        assert!(!long_home.join(".cmux-tunnels").exists());
        assert_eq!(
            run_discovery(&long_home, None, &long_tmp).unwrap_err(),
            SOCKET_ROOT_UNAVAILABLE,
        );
        assert_eq!(fs::read_dir(long_tmp).unwrap().count(), 0);
    }

    #[test]
    fn discovery_rejects_blocked_socket_start_paths() {
        let home = private_temp();
        let tmp = private_temp();
        let state = home.path().join(".cmux-tunnels");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();

        for suffix in ["", ".pid", ".log"] {
            let blocker = state.join(format!("{WORKSPACE}.sock{suffix}"));
            fs::create_dir(&blocker).unwrap();
            for discovery in [SocketDiscovery::Probe, SocketDiscovery::Prepare] {
                assert_eq!(
                    run_socket_search(home.path(), None, tmp.path(), discovery).unwrap_err(),
                    SOCKET_ROOT_UNAVAILABLE,
                );
            }
            assert!(blocker.is_dir());
            fs::remove_dir(blocker).unwrap();
        }
    }

    #[test]
    fn remote_discovery_retries_a_failed_legacy_relocation_before_returning_a_path() {
        let home = private_temp();
        let tmp = private_temp();
        let legacy = tmp.path().join(format!("codemux-ptyd-{WORKSPACE}.sock"));
        let _listener = UnixListener::bind(&legacy).unwrap();
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o600)).unwrap();
        let pid_path = legacy.with_extension("sock.pid");
        fs::write(&pid_path, std::process::id().to_string()).unwrap();
        fs::set_permissions(&pid_path, fs::Permissions::from_mode(0o600)).unwrap();

        // Resume the state that exists if a discovery run is interrupted
        // after recording the new root, but before moving the legacy socket.
        let state = home.path().join(".cmux-tunnels");
        fs::DirBuilder::new().create(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let next = tmp.path().join("cmux.resume");
        fs::create_dir(&next).unwrap();
        fs::set_permissions(&next, fs::Permissions::from_mode(0o700)).unwrap();
        let next = next.canonicalize().unwrap();
        fs::write(
            state.join(format!("{WORKSPACE}.root")),
            next.to_str().unwrap(),
        )
        .unwrap();
        let socket = next.join(format!("{WORKSPACE}.sock"));
        fs::create_dir(&socket).unwrap();
        assert!(run_discovery(home.path(), None, tmp.path()).is_err());
        assert!(UnixStream::connect(&legacy).is_ok());

        fs::remove_dir(&socket).unwrap();
        assert_eq!(
            run_discovery(home.path(), None, tmp.path()).unwrap(),
            socket
        );
        assert!(!legacy.exists());
        assert!(UnixStream::connect(&socket).is_ok());
        assert_eq!(
            run_discovery(home.path(), None, tmp.path()).unwrap(),
            socket
        );
    }

    #[test]
    fn discovery_rejects_ambiguous_or_nonliteral_forward_paths() {
        assert_eq!(
            parse_discovered_socket(
                b"banner\ncodemux:pty-path:v1 /tmp/abc123.sock\n",
                WORKSPACE,
                SocketDiscovery::Prepare,
            )
            .unwrap()
            .as_deref(),
            Some("/tmp/abc123.sock"),
        );
        for output in [
            "codemux:pty-path:v1 relative/abc123.sock\n",
            "codemux:pty-path:v1 /tmp/:escape/abc123.sock\n",
            "codemux:pty-path:v1 /tmp/%u/abc123.sock\n",
            "codemux:pty-path:v1 /tmp/../abc123.sock\n",
            "codemux:pty-path:v1 /tmp/another.sock\n",
            "codemux:pty-path:v1 /tmp/abc123.sock\ncodemux:pty-path:v1 /other/abc123.sock\n",
        ] {
            assert!(
                parse_discovered_socket(output.as_bytes(), WORKSPACE, SocketDiscovery::Prepare)
                    .is_err()
            );
        }
    }

    #[test]
    fn checking_for_an_existing_socket_does_not_allocate_or_relocate_it() {
        let home = private_temp();
        let tmp = private_temp();
        let probe = || run_socket_search(home.path(), None, tmp.path(), SocketDiscovery::Probe).unwrap();
        assert_eq!(probe(), None);
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);

        let tmp_path = tmp.path().canonicalize().unwrap();
        let legacy = tmp_path.join(format!("codemux-ptyd-{WORKSPACE}.sock"));
        let _listener = UnixListener::bind(&legacy).unwrap();
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o600)).unwrap();
        let pid = legacy.with_extension("sock.pid");
        fs::write(&pid, std::process::id().to_string()).unwrap();
        fs::set_permissions(&pid, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(probe().as_deref(), Some(legacy.as_path()));
        assert!(UnixStream::connect(&legacy).is_ok());
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);

        let current = run_discovery(home.path(), None, &tmp_path).unwrap();
        assert_eq!(probe().as_deref(), Some(current.as_path()));
        assert!(UnixStream::connect(current).is_ok());
    }

    #[test]
    fn advisory_lock_releases_after_the_holder_exits() {
        // Both implementations must leave the lock on the parent shell's
        // descriptor after the utility exits, and release it upon shell exit.
        for utility in ["flock", "perl"] {
            let Some(binary) = std::env::var_os("PATH")
                .into_iter()
                .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                .map(|path| path.join(utility))
                .find(|path| path.is_file())
            else {
                continue;
            };
            let fixture = private_temp();
            std::os::unix::fs::symlink(binary, fixture.path().join(utility)).unwrap();
            let lock = fixture.path().join("lock.fd");
            let script = format!(
                "{DIRECTORY_CHECKS}\nlock_socket_path \"$1\" || exit 1\nprintf 'locked\\n'\nexec /bin/sleep 30"
            );
            let mut holder = std::process::Command::new("/bin/sh")
                .args(["-c", &script, "fixture"])
                .arg(&lock)
                .env("PATH", fixture.path())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut signal = String::new();
            BufReader::new(holder.stdout.take().unwrap())
                .read_line(&mut signal)
                .unwrap();
            let contender = || {
                std::process::Command::new("/bin/sh")
                    .args([
                        "-c",
                        &format!("{DIRECTORY_CHECKS}\nlock_socket_path \"$1\""),
                        "fixture",
                    ])
                    .arg(&lock)
                    .env("PATH", fixture.path())
                    .output()
                    .unwrap()
                    .status
                    .success()
            };
            let concurrent_acquired = contender();
            let _ = holder.kill();
            holder.wait().unwrap();
            assert_eq!(signal, "locked\n", "{utility} acquired the lock");
            assert!(
                !concurrent_acquired,
                "{utility} excluded a concurrent process"
            );
            assert!(contender(), "{utility} released the lock on process exit");
            assert!(lock.is_file(), "the lock file remains stable");
        }
    }

    #[test]
    fn concurrent_discovery_reuses_one_fallback_with_an_old_pid_symlink_present() {
        let home = private_temp();
        let long_home = home.path().join("h".repeat(100));
        let state = long_home.join(".cmux-tunnels");
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink("2147483647", state.join(format!("{WORKSPACE}.lock"))).unwrap();
        let tmp = private_temp();

        let (first, second) = std::thread::scope(|scope| {
            let discover = || run_discovery(&long_home, None, tmp.path()).unwrap();
            let thread = scope.spawn(discover);
            let second = discover();
            (thread.join().unwrap(), second)
        });
        assert_eq!(first, second);
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}
