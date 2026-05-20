//! Service-manager unit generation for the `codemux-remote scheduler`.
//!
//! For an automation host to run automations reliably the scheduler
//! must survive reboots and logout — so it is registered as a
//! persistent **user** service. This module generates the unit
//! definitions (pure, unit-tested).
//!
//! The SSH wiring that writes these to a host and enables them belongs
//! in `ssh/bootstrap.rs`; it is wired once the account API is deployed
//! and the host bootstrap can also provision a scheduler token — see
//! `docs/plans/automations-sync.md` Phase E.

/// A systemd **user** service unit for the scheduler.
///
/// `exec_path` is the absolute path to the `codemux-remote` binary on
/// the host (bootstrap installs it at `~/.local/bin/codemux-remote`).
/// Installed at `~/.config/systemd/user/codemux-scheduler.service`,
/// then `systemctl --user enable --now codemux-scheduler`.
pub fn systemd_unit(exec_path: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Codemux automation scheduler\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec_path} scheduler\n\
         Restart=always\n\
         RestartSec=10\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// A launchd `LaunchAgent` plist for macOS hosts. `exec_path` is the
/// absolute `codemux-remote` path; `label` is the service label (e.g.
/// `org.codemux.scheduler`). Installed under
/// `~/Library/LaunchAgents/<label>.plist`.
pub fn launchd_plist(exec_path: &str, label: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n\
         \x20   <string>{exec_path}</string>\n\
         \x20   <string>scheduler</string>\n\
         \x20 </array>\n\
         \x20 <key>RunAtLoad</key>\n\
         \x20 <true/>\n\
         \x20 <key>KeepAlive</key>\n\
         \x20 <true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// Default systemd unit file name for the scheduler service.
pub const SYSTEMD_UNIT_NAME: &str = "codemux-scheduler.service";

/// Default launchd label for the scheduler service.
pub const LAUNCHD_LABEL: &str = "org.codemux.scheduler";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_unit_runs_the_scheduler_subcommand() {
        let unit = systemd_unit("/home/u/.local/bin/codemux-remote");
        assert!(unit.contains("ExecStart=/home/u/.local/bin/codemux-remote scheduler"));
        // Survives crashes and reboots.
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("WantedBy=default.target"));
        // Waits for the network — the scheduler polls the account API.
        assert!(unit.contains("Wants=network-online.target"));
    }

    #[test]
    fn launchd_plist_runs_the_scheduler_and_keeps_it_alive() {
        let plist = launchd_plist("/usr/local/bin/codemux-remote", LAUNCHD_LABEL);
        assert!(plist.contains("<string>org.codemux.scheduler</string>"));
        assert!(plist.contains("<string>/usr/local/bin/codemux-remote</string>"));
        assert!(plist.contains("<string>scheduler</string>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        // A well-formed plist document.
        assert!(plist.trim_start().starts_with("<?xml"));
        assert!(plist.contains("</plist>"));
    }
}
