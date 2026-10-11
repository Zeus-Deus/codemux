use crate::agent_provider::ProviderKind;
// These are authority classes, not provider model catalogues. Unknown is denied.
fn ceiling(provider: ProviderKind, mode: &str) -> Option<u8> {
    use ProviderKind::*;
    match (provider, mode) {
        (Codex, "read-only") | (Claude, "plan") => Some(0),
        (Codex, "workspace-write") | (Claude, "default" | "acceptEdits") => Some(1),
        (Codex, "danger-full-access") | (Claude, "bypassPermissions") => Some(2),
        (Cursor, "ask") | (Grok, "ask") => Some(0),
        (Cursor, "agent") | (Grok, "agent") => Some(2),
        (OpenCode, "default") => Some(1),
        (Hermes, "default") => Some(1),
        _ => None,
    }
}
pub fn permits(
    parent_provider: ProviderKind,
    parent_mode: &str,
    child_provider: ProviderKind,
    child_mode: &str,
) -> bool {
    matches!(child_provider, ProviderKind::Codex | ProviderKind::Claude)
        && matches!((ceiling(parent_provider,parent_mode),ceiling(child_provider,child_mode)),(Some(parent),Some(child)) if child<=parent)
}
