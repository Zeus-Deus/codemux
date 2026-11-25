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

/// Git and rsync take a shell command string. Only fixed policy arguments
/// belong here; each caller supplies the destination separately.
#[cfg(unix)]
pub(crate) fn verified_shell_command() -> String {
    std::iter::once("ssh")
        .chain(verified_command_args())
        .collect::<Vec<_>>()
        .join(" ")
}
