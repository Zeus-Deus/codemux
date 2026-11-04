//! The registry tools and socket commands each verb uses, in one place, so a
//! rename touches one line.

// Registry tools, reached through the socket's `tool_call`.
pub(crate) const WORKSPACE_CREATE: &str = "workspace_create";
pub(crate) const WORKSPACE_LIST: &str = "workspace_list";
pub(crate) const WORKSPACE_INFO: &str = "workspace_info";
pub(crate) const WORKSPACE_CLOSE: &str = "workspace_close";
pub(crate) const WORKTREE_CREATE: &str = "worktree_create";
pub(crate) const THREAD_LAUNCH: &str = "thread_launch";
pub(crate) const THREAD_SEND: &str = "thread_send";
pub(crate) const THREAD_WAIT: &str = "thread_wait";
pub(crate) const THREAD_READ: &str = "thread_read";
pub(crate) const THREAD_LIST: &str = "thread_list";
pub(crate) const THREAD_STATUS: &str = "thread_status";
pub(crate) const THREAD_RESPOND: &str = "thread_respond";
pub(crate) const THREAD_INTERRUPT: &str = "thread_interrupt";
pub(crate) const THREAD_STOP: &str = "thread_stop";
pub(crate) const OPERATION_STATUS: &str = "operation_status";

// Admin socket commands (not registry tools) behind `codemux connector`.
pub(crate) const CONNECTOR_STATUS: &str = "agent_connector_status";
pub(crate) const CONNECTOR_SET_CONFIG: &str = "agent_connector_set_config";
pub(crate) const CONNECTOR_ISSUE: &str = "agent_connector_issue";
pub(crate) const CONNECTOR_REVOKE: &str = "agent_connector_revoke";
pub(crate) const CONNECTOR_REVOKE_ALL: &str = "agent_connector_revoke_all";
pub(crate) const CONNECTOR_APPROVE: &str = "agent_connector_approve";
pub(crate) const CONNECTOR_DENY: &str = "agent_connector_deny";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_control::{registry, Audience};

    #[test]
    fn workspace_and_thread_verbs_use_outside_registry_tools() {
        for name in [
            WORKSPACE_CREATE, WORKSPACE_LIST, WORKSPACE_INFO, WORKSPACE_CLOSE, WORKTREE_CREATE, THREAD_LAUNCH,
            THREAD_SEND, THREAD_WAIT, THREAD_READ, THREAD_LIST, THREAD_STATUS, THREAD_RESPOND, THREAD_INTERRUPT,
            THREAD_STOP, OPERATION_STATUS,
        ] {
            assert!(registry::served(name, Audience::Outside).is_some(), "{name} is not an outside registry tool");
        }
    }

    #[test]
    fn connector_verbs_use_socket_commands_the_identity_layer_classifies() {
        for name in [
            CONNECTOR_STATUS, CONNECTOR_SET_CONFIG, CONNECTOR_ISSUE, CONNECTOR_REVOKE, CONNECTOR_REVOKE_ALL,
            CONNECTOR_APPROVE, CONNECTOR_DENY,
        ] {
            assert!(crate::control_identity::CLASSIFIED.contains(&name), "{name} is not a classified socket command");
            assert!(registry::lookup(name).is_none(), "{name} must stay a socket command, not a registry tool");
        }
    }
}
