//! The pull side of Hermes' slash-command vocabulary.
//!
//! Two things have to meet here and they do not naturally fit:
//!
//! * Hermes PUSHES its command list, once, as an
//!   `available_commands_update` notification that arrives unprompted
//!   immediately after `session/new`. It is never requested and there is
//!   no method that asks for it again.
//! * Codemux PULLS, lazily, keyed by provider + cwd, from
//!   `list_chat_slash_commands` — a command that has no session, may run
//!   with no session anywhere near it, and must answer at the speed of a
//!   popup opening.
//!
//! The bridge is this cache. The notification's job is to keep the cwd's
//! entry current — it is the invalidation, arriving with the replacement
//! already attached — and the pull just reads. Doing it the other way
//! round (spawn a child and run `session/new` when the popup opens) would
//! cost seconds and boot a whole agent to populate a menu, which is
//! exactly the trade the capability harvest already refuses to make.
//!
//! Keyed by cwd rather than by (profile, cwd) because the pull only knows
//! the cwd. In practice the vocabulary is the agent's built-in command
//! set plus whatever the workspace defines, so two profiles at one cwd
//! agree; last writer wins if they ever do not.
//!
//! A cwd nobody has run a session in yields an empty list, never an error
//! — the composer then shows Codemux's own built-ins, which is the same
//! thing every provider without a discovery surface does.

use std::collections::HashMap;
use std::path::Path;

use tokio::sync::Mutex;

use crate::agent_provider::claude::slash_commands::ProviderSlashCommand;

#[derive(Default)]
pub struct HermesSlashCommandCache {
    inner: Mutex<HashMap<String, Vec<ProviderSlashCommand>>>,
}

impl HermesSlashCommandCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store the vocabulary a live session just reported for `cwd`.
    pub async fn record(&self, cwd: &Path, commands: Vec<ProviderSlashCommand>) {
        self.inner.lock().await.insert(key(cwd), commands);
    }

    /// What is known for `cwd`. Empty rather than absent for an unknown
    /// cwd: the popup renders a list, and "no provider commands" is a
    /// legitimate answer it already knows how to draw.
    pub async fn get(&self, cwd: &str) -> Vec<ProviderSlashCommand> {
        self.inner
            .lock()
            .await
            .get(&key(Path::new(cwd)))
            .cloned()
            .unwrap_or_default()
    }

    /// Forget one cwd, or everything when given `None`. Used when a
    /// session ends, so a vocabulary from a workspace that has since
    /// changed does not outlive the session that observed it.
    pub async fn invalidate(&self, cwd: Option<&Path>) {
        let mut cached = self.inner.lock().await;
        match cwd {
            Some(cwd) => {
                cached.remove(&key(cwd));
            }
            None => cached.clear(),
        }
    }
}

/// Normalise a path into a cache key.
///
/// The frontend sends the cwd as a string it got from the pane binding,
/// while sessions hold a `PathBuf`; a trailing separator on one side would
/// otherwise miss an entry the other side wrote.
fn key(cwd: &Path) -> String {
    let text = cwd.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        text.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str) -> ProviderSlashCommand {
        ProviderSlashCommand {
            name: name.into(),
            description: String::new(),
            argument_hint: String::new(),
        }
    }

    #[tokio::test]
    async fn the_notification_fills_what_the_pull_reads() {
        let cache = HermesSlashCommandCache::new();
        assert!(cache.get("/work/repo").await.is_empty());
        cache
            .record(Path::new("/work/repo"), vec![command("compress")])
            .await;
        assert_eq!(cache.get("/work/repo").await.len(), 1);
        // A trailing separator addresses the same workspace.
        assert_eq!(cache.get("/work/repo/").await.len(), 1);
    }

    #[tokio::test]
    async fn a_later_update_replaces_rather_than_merges() {
        let cache = HermesSlashCommandCache::new();
        cache
            .record(Path::new("/work/repo"), vec![command("a"), command("b")])
            .await;
        cache
            .record(Path::new("/work/repo"), vec![command("a")])
            .await;
        let commands = cache.get("/work/repo").await;
        assert_eq!(commands.len(), 1, "a removed command must disappear");
        assert_eq!(commands[0].name, "a");
        cache.invalidate(Some(Path::new("/work/repo"))).await;
        assert!(cache.get("/work/repo").await.is_empty());
    }
}
