//! OpenSSH policy for unattended, application-managed connections.

// A reused multiplex connection would skip host-key verification for this
// invocation. Leave OpenSSH's identity and host trust configuration in use.
pub(crate) const VERIFIED_HOST_ARGS: [&str; 9] = [
    "-T",
    "-o",
    "BatchMode=yes",
    "-o",
    "StrictHostKeyChecking=yes",
    "-o",
    "ControlPath=none",
    "-o",
    "ForwardAgent=no",
];

pub(crate) fn verified_command_args() -> impl Iterator<Item = &'static str> {
    VERIFIED_HOST_ARGS
        .into_iter()
        .chain(["-o", "ClearAllForwardings=yes"])
}

// The live command must carry this flag. OpenSSH reads its configuration
// again at connect time and starts implicit jump clients without our flags.
const NO_PROXY_JUMP: [&str; 2] = ["-o", "ProxyJump=none"];
const PROXY_JUMP_ERROR: &str =
    "Codemux cannot verify implicit SSH ProxyJump clients. Configure a user-managed ProxyCommand instead.";

const CHECKED_SSH: &str = r#"
config=$(ssh -G "$@" </dev/null) || exit 255
cr=$(printf "\r")
while IFS= read -r setting; do
    setting=${setting%"$cr"}
    case $setting in
        "proxyjump none"|"proxyjump ") ;;
        "proxyjump "*)
            echo "Codemux cannot verify implicit SSH ProxyJump clients. Configure a user-managed ProxyCommand instead." >&2
            exit 255
            ;;
    esac
done <<CODEMUX_SSH_CONFIG
$config
CODEMUX_SSH_CONFIG
exec ssh -o ProxyJump=none "$@"
"#;

#[cfg(unix)]
pub(crate) fn checked_ssh_args() -> [&'static str; 3] {
    ["-c", CHECKED_SSH, "codemux-ssh"]
}

#[cfg(unix)]
pub(crate) fn checked_ssh() -> std::process::Command {
    let mut command = std::process::Command::new("sh");
    command.args(checked_ssh_args());
    command
}

pub(crate) fn checked_ssh_tokio() -> tokio::process::Command {
    #[cfg(unix)]
    return checked_ssh().into();
    #[cfg(windows)]
    return pinned_native_ssh();
}

/// Take the direct OpenSSH path used by native clients and guarded dispatch.
/// Preflight reports configured implicit routes, while the actual argv
/// independently forbids them even if configuration changes afterward.
pub(crate) fn pinned_native_ssh() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("ssh");
    command.args(NO_PROXY_JUMP);
    command
}

pub(crate) async fn check_native_jumps(command: &tokio::process::Command) -> std::io::Result<()> {
    use std::ffi::OsStr;
    use std::io::{Error, ErrorKind};
    use std::process::Stdio;

    let command = command.as_std();
    let args: Vec<_> = command.get_args().collect();
    let args = args
        .strip_prefix(&NO_PROXY_JUMP.map(OsStr::new))
        .ok_or_else(|| Error::other("SSH command must disable implicit jump clients"))?;
    let mut probe = tokio::process::Command::new(command.get_program());
    if let Some(directory) = command.get_current_dir() {
        probe.current_dir(directory);
    }
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => probe.env(key, value),
            None => probe.env_remove(key),
        };
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        probe
            .arg("-G")
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Error::new(ErrorKind::TimedOut, "SSH configuration check timed out"))??;
    if !output.status.success() {
        return Err(Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let config = String::from_utf8(output.stdout).map_err(Error::other)?;
    if config.lines().any(|line| {
        line.strip_prefix("proxyjump ")
            .is_some_and(|jump| !matches!(jump, "" | "none"))
    }) {
        return Err(Error::other(PROXY_JUMP_ERROR));
    }
    Ok(())
}

/// rsync parses its remote-shell string itself. It escapes quotes by
/// doubling them inside a quoted argument.
#[cfg(unix)]
pub(crate) fn verified_shell_command() -> String {
    format!(
        "sh -c '{}' codemux-ssh {}",
        CHECKED_SSH.replace('\'', "''"),
        verified_command_args().collect::<Vec<_>>().join(" ")
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn git_ssh_command() -> String {
    format!(
        "sh -c {} codemux-ssh {}",
        shell_quote(CHECKED_SSH),
        verified_command_args().collect::<Vec<_>>().join(" ")
    )
}

fn command_env(command: &std::process::Command, key: &str) -> Option<std::ffi::OsString> {
    if let Some((_, value)) = command.get_envs().find(|(name, _)| *name == key) {
        return value.map(std::ffi::OsStr::to_owned);
    }
    std::env::var_os(key)
}

fn config_command(command: &std::process::Command) -> std::process::Command {
    let mut git = crate::git::git_command();
    if let Some(dir) = command.get_current_dir() {
        git.current_dir(dir);
    }
    // Some callers select the checkout with a leading Git -C.
    let mut args = command.get_args();
    while args.next().is_some_and(|arg| arg == "-C") {
        if let Some(directory) = args.next() {
            git.arg("-C").arg(directory);
        }
    }
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                git.env(key, value);
            }
            None => {
                git.env_remove(key);
            }
        }
    }
    git.env_remove("GIT_CONFIG");
    git
}

fn git_config_error(output: &std::process::Output) -> std::io::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    std::io::Error::other(format!(
        "Git config exited with {}: {}",
        output.status,
        stderr.trim()
    ))
}

fn system_git_config(
    command: &std::process::Command,
) -> std::io::Result<Option<std::path::PathBuf>> {
    use std::io::{Error, ErrorKind};
    use std::path::PathBuf;

    if let Some(disabled) = command_env(command, "GIT_CONFIG_NOSYSTEM") {
        let disabled = disabled.to_string_lossy().to_ascii_lowercase();
        let skip = match disabled.as_str() {
            "" | "false" | "no" | "off" => false,
            "true" | "yes" | "on" => true,
            value => value.parse::<i64>().map(|value| value != 0).map_err(|_| {
                Error::new(
                    ErrorKind::InvalidInput,
                    "Invalid Git GIT_CONFIG_NOSYSTEM setting",
                )
            })?,
        };
        if skip {
            return Ok(None);
        }
    }

    // Ask Git which file it actually read. --no-includes keeps the first
    // origin at the system file itself, including on older Git versions.
    let output = config_command(command)
        .env("LC_ALL", "C")
        .args([
            "config",
            "--system",
            "--no-includes",
            "--show-origin",
            "--null",
            "--list",
        ])
        .output()?;
    if !output.status.success() {
        if String::from_utf8_lossy(&output.stderr).contains("No such file or directory") {
            return Ok(None);
        }
        return Err(git_config_error(&output));
    }
    if output.stdout.is_empty() {
        return Ok(None);
    }
    let Some(path) = output
        .stdout
        .split(|byte| *byte == 0)
        .next()
        .and_then(|origin| origin.strip_prefix(b"file:"))
    else {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Git did not report a system config file",
        ));
    };
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(path))
    };
    #[cfg(not(unix))]
    let path = PathBuf::from(std::str::from_utf8(path).map_err(Error::other)?);
    if path.has_root() {
        return Ok(Some(path));
    }
    let mut base = command
        .get_current_dir()
        .map_or_else(std::env::current_dir, |dir| std::path::absolute(dir))?;
    let mut args = command.get_args();
    while args.next().is_some_and(|arg| arg == "-C") {
        if let Some(directory) = args.next().filter(|directory| !directory.is_empty()) {
            base = base.join(directory);
        }
    }
    Ok(Some(base.join(path)))
}

/// Add a system-scope default before the user's system config. Git can
/// still resolve higher-priority config in the destination during clone.
/// Only the data file is temporary; Git invokes the policy through its shell.
pub(crate) fn unattended_git(
    command: &mut std::process::Command,
) -> std::io::Result<Option<tempfile::TempDir>> {
    command.env("GIT_TERMINAL_PROMPT", "0");
    if command_env(command, "GIT_SSH").is_some()
        || command_env(command, "GIT_SSH_COMMAND").is_some()
    {
        return Ok(None);
    }
    let system = system_git_config(command)?;
    let temp_root = std::path::absolute(std::env::temp_dir())?;
    let mut builder = tempfile::Builder::new();
    builder.prefix("codemux-git-config-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir_in(temp_root)?;
    let config = directory.path().join("config");
    let write_config = |name: &str, value: &std::ffi::OsStr| -> std::io::Result<()> {
        let output = config_command(command)
            .arg("config")
            .arg("--file")
            .arg(&config)
            .arg("--add")
            .arg(name)
            .arg(value)
            .output()?;
        if !output.status.success() {
            return Err(git_config_error(&output));
        }
        Ok(())
    };
    let transport = git_ssh_command();
    write_config("core.sshCommand", transport.as_ref())?;
    if let Some(system) = system {
        write_config("include.path", system.as_os_str())?;
    }
    // Older Git versions can ignore GIT_CONFIG_SYSTEM. Confirm the
    // fallback is active before starting an unattended operation.
    let check = config_command(command)
        .env("GIT_CONFIG_SYSTEM", &config)
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .args([
            "config",
            "--system",
            "--no-includes",
            "--get",
            "core.sshCommand",
        ])
        .output()?;
    if !check.status.success() || String::from_utf8_lossy(&check.stdout).trim_end() != transport {
        return Err(std::io::Error::other(
            "Git could not load the unattended SSH policy through GIT_CONFIG_SYSTEM",
        ));
    }
    command
        .env("GIT_CONFIG_SYSTEM", config)
        .env_remove("GIT_CONFIG_NOSYSTEM");
    Ok(Some(directory))
}

/// The probe runs in its current directory, so it can ask that Git
/// directly whether an owner-supplied command already applies there.
pub(crate) fn unattended_git_script(command: &str) -> String {
    let transport = shell_quote(&git_ssh_command());
    format!(
        r#"export GIT_TERMINAL_PROMPT=0
if [ "${{GIT_SSH+x}}" != x ] && [ "${{GIT_SSH_COMMAND+x}}" != x ]; then
    (unset GIT_CONFIG; git config --get core.sshCommand >/dev/null)
    case $? in
        0) ;;
        1) export GIT_SSH_COMMAND={transport} ;;
        *) exit 1 ;;
    esac
fi
{command}
"#
    )
}

#[cfg(all(test, unix))]
mod unattended_git_tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn implicit_jumps_are_rejected_and_disabled_in_the_live_client() {
        let root = tempfile::tempdir().unwrap();
        let ssh = root.path().join("ssh");
        fs::write(
            &ssh,
            r#"#!/bin/sh
if [ "$1" = -G ]; then
    if [ "$FIXTURE_ROUTE" = jump ]; then
        printf 'proxyjump relay-fixture\n'
    else
        printf 'proxycommand user-fixture\n'
    fi
    exit 0
fi
[ "$1" = -o ] && [ "$2" = ProxyJump=none ] || exit 2
printf 'transport started\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(root.path().to_owned())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let run = |route| {
            checked_ssh()
                .env("PATH", &path)
                .env("FIXTURE_ROUTE", route)
                .args(verified_command_args())
                .args(["--", "destination", "fixture"])
                .output()
                .unwrap()
        };
        let allowed = run("command");
        assert!(allowed.status.success(), "{allowed:?}");
        assert_eq!(allowed.stdout, b"transport started\n");
        let denied = run("jump");
        assert_eq!(denied.status.code(), Some(255), "{denied:?}");
        assert!(denied.stdout.is_empty());
        assert!(String::from_utf8_lossy(&denied.stderr).contains(PROXY_JUMP_ERROR));
    }

    #[tokio::test]
    async fn native_checks_the_route_before_using_a_pinned_command() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("ssh");
        fs::write(
            &fixture,
            r#"#!/bin/sh
if [ "$1" = -G ]; then
    shift
    for arg do
        [ "$arg" != ProxyJump=none ] || exit 2
    done
    [ "$FIXTURE_ROUTE" != jump ] || printf 'proxyjump relay-fixture\n'
    exit 0
fi
[ "$1" = -o ] && [ "$2" = ProxyJump=none ] || exit 2
printf 'transport started\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(root.path().to_owned())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let build = |route| {
            let mut command = pinned_native_ssh();
            command
                .env("PATH", &path)
                .env("FIXTURE_ROUTE", route)
                .args(verified_command_args())
                .args(["--", "destination", "fixture"]);
            command
        };
        let denied = check_native_jumps(&build("jump")).await.unwrap_err();
        assert_eq!(denied.to_string(), PROXY_JUMP_ERROR);

        let mut allowed = build("direct");
        check_native_jumps(&allowed).await.unwrap();
        let output = allowed.output().await.unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"transport started\n");
    }

    #[test]
    fn git_default_is_strict_and_clone_keeps_checkout_ssh_override() {
        if crate::test_isolation::isolated(
            "ssh_policy::unattended_git_tests::git_default_is_strict_and_clone_keeps_checkout_ssh_override",
        ) {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let path = std::env::join_paths(
            std::iter::once(root.path().to_owned())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let fake_ssh = |path: &Path, label: &str| {
            fs::write(
                path,
                format!("#!/bin/sh\n[ \"$1\" != -G ] || exit 0\nprintf '%s\\n' \"{label} $*\" >&2\nexit 1\n"),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        };
        fake_ssh(&root.path().join("ssh"), "default");
        let scoped_ssh = root.path().join("scoped-ssh");
        fake_ssh(&scoped_ssh, "scoped");

        let remote = "git@fixture.invalid:repo";
        let mut git = crate::git::git_command();
        git.current_dir(root.path())
            .env("PATH", &path)
            .args(["ls-remote", "--", remote]);
        let config = unattended_git(&mut git).unwrap().unwrap();
        assert_eq!(
            fs::metadata(config.path().join("config"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        let output = git.output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("default "), "{stderr}");
        assert!(stderr.contains("StrictHostKeyChecking=yes"), "{stderr}");
        assert!(stderr.contains("ProxyJump=none"), "{stderr}");

        let script = unattended_git_script(&format!("git ls-remote -- '{remote}'"));
        let output = Command::new("sh")
            .current_dir(root.path())
            .env("PATH", &path)
            .env("TMPDIR", root.path())
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("default "), "{stderr}");
        assert!(stderr.contains("StrictHostKeyChecking=yes"), "{stderr}");
        assert!(stderr.contains("ProxyJump=none"), "{stderr}");

        let scoped_config = root.path().join("scoped-config");
        let configure = |options: &[&str], key: &str, value: &str| {
            assert!(crate::git::git_command()
                .current_dir(root.path())
                .args(["config"])
                .args(options)
                .args([key, value])
                .status()
                .unwrap()
                .success());
        };
        configure(
            &["--file", scoped_config.to_str().unwrap()],
            "core.sshCommand",
            &crate::ssh::exec::sh_quote(scoped_ssh.to_str().unwrap()),
        );

        let mut git = crate::git::git_command();
        git.current_dir(root.path())
            .env("PATH", &path)
            .env("GIT_CONFIG_NOSYSTEM", "0")
            .env("GIT_CONFIG_SYSTEM", &scoped_config)
            .args(["ls-remote", "--", remote]);
        let _config = unattended_git(&mut git).unwrap().unwrap();
        let output = git.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("scoped "), "{stderr}");
        assert!(!stderr.contains("default "), "{stderr}");

        let output = Command::new("sh")
            .current_dir(root.path())
            .env("PATH", &path)
            .env("GIT_CONFIG_GLOBAL", &scoped_config)
            .args(["-c", &script])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("scoped "), "{stderr}");
        assert!(!stderr.contains("default "), "{stderr}");

        configure(
            &["--global"],
            "includeIf.gitdir:**/scoped-clone/.git.path",
            scoped_config.to_str().unwrap(),
        );
        let mut git = crate::git::git_command();
        git.current_dir(root.path()).env("PATH", &path).args([
            "-c",
            "ssh.variant=ssh",
            "clone",
            "--",
            remote,
            "scoped-clone",
        ]);
        let _config = unattended_git(&mut git).unwrap();
        let output = git.output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("scoped "), "{stderr}");
        assert!(!stderr.contains("default "), "{stderr}");
    }
}
