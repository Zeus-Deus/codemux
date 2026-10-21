use crate::agent_provider::ProviderKind;
use crate::agent_control::permissions::Ceiling;
pub fn permits(parent_provider: ProviderKind, parent_mode: &str, child_provider: ProviderKind, child_mode: &str) -> bool {
    matches!(child_provider, ProviderKind::Codex | ProviderKind::Claude)
        && matches!((Ceiling::for_mode(parent_provider, Some(parent_mode)), Ceiling::for_mode(child_provider, Some(child_mode))), (Some(parent), Some(child)) if parent.permits(child))
}
