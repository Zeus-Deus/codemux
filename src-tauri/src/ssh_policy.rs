//! OpenSSH policy for unattended, application-managed connections.

// A reused multiplex connection would skip host-key verification for this
// invocation. Leave OpenSSH's identity, host trust, and jump-host config in use.
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

// OpenSSH starts its ProxyJump clients without the application's command
// flags. Ask OpenSSH for the actual effective configuration of each implicit
// hop, including the user, port, forwarding target, and earlier jump chain.
// Keep the original argument list intact for the eventual exec.
const CHECKED_SSH: &str = r#"
cr=$(printf "\r")
ssh_setting() {
    while IFS= read -r setting; do
        setting=${setting%"$cr"}
        case $setting in "$1 "*) printf "%s\n" "${setting#* }"; return;; esac
    done <<CODEMUX_SSH_CONFIG
$config
CODEMUX_SSH_CONFIG
}
verify_proxy_jumps() {
    config=$(ssh -G "$@" </dev/null) || return 1
    count=0
    while :; do
        jumps=$(ssh_setting proxyjump)
        case $jumps in ""|none) return 0;; esac
        count=$((count + 1))
        if [ "$count" -gt 16 ]; then
            echo "Codemux: too many SSH jump hops or a jump loop" >&2
            return 1
        fi
        host=$(ssh_setting hostname)
        port=$(ssh_setting port)
        hop=${jumps##*,}
        case $jumps in *,*) set -- -J "${jumps%,*}";; *) set --;; esac
        address=${hop#ssh://}
        case $address in
            *@*) set -- "$@" -l "${address%@*}"; address=${address##*@};;
        esac
        case $address in
            \[*\]:*) set -- "$@" -p "${address##*]:}"; address=${address%%]*}; address=${address#\[};;
            \[*\]) address=${address#\[}; address=${address%\]};;
            *:*) set -- "$@" -p "${address##*:}"; address=${address%:*};;
        esac
        config=$(ssh -G "$@" -W "[$host]:$port" -- "$address" </dev/null) || return 1
        strict=$(ssh_setting stricthostkeychecking)
        control=$(ssh_setting controlpath)
        if { [ "$strict" != true ] && [ "$strict" != yes ]; } ||
            [ "$(ssh_setting batchmode)" != yes ] ||
            [ "$(ssh_setting forwardagent)" != no ] ||
            { [ -n "$control" ] && [ "$control" != none ]; }; then
            printf "Codemux: SSH jump %s requires StrictHostKeyChecking yes, BatchMode yes, ForwardAgent no, and ControlPath none\n" "$hop" >&2
            return 1
        fi
    done
}
verify_proxy_jumps "$@" || exit 255
exec ssh "$@"
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
    return tokio::process::Command::new("ssh");
}

/// Used by native Windows clients and dispatches that must check the route
/// before the guarded SSH spawn.
pub(crate) async fn check_native_jumps(command: &tokio::process::Command) -> std::io::Result<()> {
    use std::ffi::{OsStr, OsString};
    use std::io::{Error, ErrorKind};
    use std::process::Stdio;

    async fn config(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> std::io::Result<String> {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::process::Command::new("ssh")
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
        String::from_utf8(output.stdout).map_err(Error::other)
    }

    fn setting<'a>(config: &'a str, key: &str) -> &'a str {
        config
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(' ')?;
                (name == key).then_some(value)
            })
            .unwrap_or_default()
    }

    let args: Vec<OsString> = command.as_std().get_args().map(OsStr::to_owned).collect();
    let mut effective = config(args).await?;
    for depth in 0..=16 {
        let jumps = setting(&effective, "proxyjump");
        if matches!(jumps, "" | "none") {
            return Ok(());
        }
        if depth == 16 {
            break;
        }
        let forward = format!(
            "[{}]:{}",
            setting(&effective, "hostname"),
            setting(&effective, "port")
        );
        let (prior, hop) = jumps.rsplit_once(',').unwrap_or(("", jumps));
        let address = hop.strip_prefix("ssh://").unwrap_or(hop);
        let mut args = Vec::new();
        if !prior.is_empty() {
            args.extend(["-J", prior]);
        }
        let address = match address.rsplit_once('@') {
            Some((user, address)) => {
                args.extend(["-l", user]);
                address
            }
            None => address,
        };
        let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
            let (host, suffix) = bracketed
                .split_once(']')
                .ok_or_else(|| Error::other("Invalid SSH jump host"))?;
            (host, suffix.strip_prefix(':'))
        } else {
            address
                .rsplit_once(':')
                .map_or((address, None), |(host, port)| (host, Some(port)))
        };
        if let Some(port) = port {
            args.extend(["-p", port]);
        }
        args.extend(["-W", &forward, "--", host]);
        let hop_config = config(args).await?;
        if !matches!(
            setting(&hop_config, "stricthostkeychecking"),
            "true" | "yes"
        ) || setting(&hop_config, "batchmode") != "yes"
            || setting(&hop_config, "forwardagent") != "no"
            || !matches!(setting(&hop_config, "controlpath"), "" | "none")
        {
            return Err(Error::other(format!(
                "SSH jump {hop} requires StrictHostKeyChecking yes, BatchMode yes, ForwardAgent no, and ControlPath none"
            )));
        }
        effective = hop_config;
    }
    Err(Error::other("Too many SSH jump hops or a jump loop"))
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

fn git_ssh_wrapper() -> String {
    format!(
        "#!/bin/sh\nset -- {} \"$@\"\n{CHECKED_SSH}",
        verified_command_args().collect::<Vec<_>>().join(" ")
    )
}

/// Set the unattended Git default and keep its executable alive through the
/// Git operation. Git reads `GIT_SSH` after `core.sshCommand` and
/// `GIT_SSH_COMMAND`, so checkout-specific overrides also work during clone.
pub(crate) fn unattended_git(
    command: &mut std::process::Command,
) -> std::io::Result<Option<tempfile::TempDir>> {
    use std::io::Write;

    command.env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH").is_some() || std::env::var_os("GIT_SSH_COMMAND").is_some() {
        return Ok(None);
    }
    let temp_root = std::path::absolute(std::env::temp_dir())?;
    let mut builder = tempfile::Builder::new();
    builder.prefix("codemux-git-ssh-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir_in(temp_root)?;
    // The filename tells Git this is OpenSSH, including on Git for Windows,
    // which launches an extensionless shebang script through its own shell.
    let wrapper = directory.path().join("ssh");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    options
        .open(&wrapper)?
        .write_all(git_ssh_wrapper().as_bytes())?;
    command.env("GIT_SSH", wrapper);
    Ok(Some(directory))
}

/// Apply the same Git default to a probe on the device that will run the
/// automation. Owner configuration on that device retains precedence.
pub(crate) fn unattended_git_script(command: &str) -> String {
    let wrapper = git_ssh_wrapper();
    format!(
        r#"export GIT_TERMINAL_PROMPT=0
if [ "${{GIT_SSH+x}}" != x ] && [ "${{GIT_SSH_COMMAND+x}}" != x ]; then
    ssh_dir=$(mktemp -d "${{TMPDIR:-/tmp}}/codemux-git-ssh.XXXXXXXX") || exit 1
    trap 'rm -f "$ssh_dir/ssh"; rmdir "$ssh_dir"' EXIT
    trap 'exit 1' HUP INT TERM
    (umask 077; cat >"$ssh_dir/ssh" <<'CODEMUX_GIT_SSH'
{wrapper}CODEMUX_GIT_SSH
    ) && chmod 700 "$ssh_dir/ssh" || exit 1
    export GIT_SSH="$ssh_dir/ssh"
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
    fn checked_ssh_requires_strict_config_on_each_jump() {
        let root = tempfile::tempdir().unwrap();
        let ssh = root.path().join("ssh");
        fs::write(
            &ssh,
            r#"#!/bin/sh
if [ "$1" != -G ]; then
    printf 'transfer started\n'
    exit 0
fi
case "$*" in
    *2001:db8::2)
        case "$*" in *'-J jump-one -l alice -p 2222 -W [destination.test]:22 '*) ;; *) exit 2;; esac
        printf 'hostname 2001:db8::2\nport 2222\nproxyjump jump-one\n'
        ;;
    *'-- jump-one')
        case "$*" in *'-W [2001:db8::2]:2222 '*) ;; *) exit 2;; esac
        if [ "${WEAK_JUMP:-}" = yes ]; then
            printf 'stricthostkeychecking accept-new\n'
            exit 0
        fi
        ;;
    *)
        printf 'hostname destination.test\nport 22\nproxyjump jump-one,alice@[2001:db8::2]:2222\n'
        exit 0
        ;;
esac
printf 'stricthostkeychecking true\nbatchmode yes\nforwardagent no\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(root.path().to_owned())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let run = |weak| {
            checked_ssh()
                .env("PATH", &path)
                .env("WEAK_JUMP", weak)
                .args(verified_command_args())
                .args(["--", "destination", "fixture"])
                .output()
                .unwrap()
        };
        let allowed = run("no");
        assert!(allowed.status.success(), "{allowed:?}");
        assert_eq!(allowed.stdout, b"transfer started\n");
        let denied = run("yes");
        assert_eq!(denied.status.code(), Some(255), "{denied:?}");
        assert!(denied.stdout.is_empty());
        assert!(String::from_utf8_lossy(&denied.stderr).contains("requires StrictHostKeyChecking"));
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
        let _wrapper = unattended_git(&mut git).unwrap();
        let output = git.output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("default "), "{stderr}");
        assert!(stderr.contains("StrictHostKeyChecking=yes"), "{stderr}");

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
        let _wrapper = unattended_git(&mut git).unwrap();
        let output = git.output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("scoped "), "{stderr}");
        assert!(!stderr.contains("default "), "{stderr}");
    }
}
