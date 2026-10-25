//! Callers originate at a trusted transport, independently of plugin-selected targets.
use super::{platform_manifest::PlatformManifest, platform_store::InstallationRecord};
use crate::agent_control::{ControlAccess, ControlCaller};

#[derive(Clone, Debug)]
pub struct PlatformCaller {
    pub(crate) installation_id: String,
    pub(crate) generation: String,
    pub(crate) control: ControlCaller,
    pub(crate) project_ids: Option<Vec<String>>,
    pub(crate) context_thread_id: Option<String>,
    pub(crate) delivery_id: Option<String>,
    pub(crate) operation_id: Option<String>,
    pub(crate) cancellation: Option<super::platform_runtime::RuntimeCancellation>,
}
impl PlatformCaller {
    /// Must only be called with host-verified identity, never decoded from plugin arguments.
    pub fn new(
        installation_id: String,
        generation: String,
        control: ControlCaller,
        project_ids: Option<Vec<String>>,
        context_thread_id: Option<String>,
    ) -> Self {
        Self {
            installation_id,
            generation,
            control,
            project_ids,
            context_thread_id,
            delivery_id: None,
            operation_id: None,
            cancellation: None,
        }
    }
    pub fn for_delivery(mut self, delivery_id: String) -> Self {
        if self.control.access == ControlAccess::FullAccess {
            self.control.access = ControlAccess::Supervised;
        }
        if let crate::agent_control::CallerPrincipal::Thread { ceiling, .. } =
            &mut self.control.identity
        {
            if let Some(supervised) = crate::agent_control::permissions::Ceiling::for_mode(
                crate::agent_provider::ProviderKind::Claude,
                Some("acceptEdits"),
            ) {
                *ceiling = ceiling.intersection(supervised);
            }
        }
        self.context_thread_id = None;
        self.operation_id = Some(format!("event-{delivery_id}"));
        self.delivery_id = Some(delivery_id);
        self
    }
    pub(crate) fn with_cancellation(
        mut self,
        cancellation: super::platform_runtime::RuntimeCancellation,
    ) -> Self {
        self.cancellation = Some(cancellation);
        self
    }
    pub fn with_operation_id(mut self, operation_id: String) -> Self {
        self.operation_id = Some(operation_id);
        self
    }
    pub fn installation_id(&self) -> &str {
        &self.installation_id
    }
    pub fn generation(&self) -> &str {
        &self.generation
    }
    /// An agent's identity can never be replaced by a UI/context target.
    pub fn source_thread_id(&self) -> Option<&str> {
        match &self.control.identity {
            crate::agent_control::CallerPrincipal::Thread { thread_id, .. } => Some(thread_id),
            crate::agent_control::CallerPrincipal::LocalController
            | crate::agent_control::CallerPrincipal::RemoteClient { .. } => {
                self.context_thread_id.as_deref()
            }
            crate::agent_control::CallerPrincipal::OutsideClient => None,
        }
    }
}

pub fn assert_active(record: &InstallationRecord, caller: &PlatformCaller) -> Result<(), String> {
    if caller
        .cancellation
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err("cancelled: Plugin invocation ended".into());
    }
    if record.installation_id != caller.installation_id || record.generation != caller.generation {
        return Err("stale_generation: Plugin authority was revoked".into());
    }
    if !record.enabled {
        return Err("disabled: Plugin is disabled".into());
    }
    if record.consented_digest.as_deref() != Some(&record.digest) {
        return Err("needs_consent: Review the exact plugin snapshot before execution".into());
    }
    if !record.manifest.compatibility().available {
        return Err("incompatible: Plugin requires unsupported platform features".into());
    }
    if let Some(reason) = &record.unavailable_reason {
        return Err(format!("unavailable: {reason}"));
    }
    Ok(())
}
pub fn capability(
    record: &InstallationRecord,
    caller: &PlatformCaller,
    name: &str,
    project: Option<&str>,
) -> Result<(), String> {
    assert_active(record, caller)?;
    if !record
        .manifest
        .requested_capabilities()
        .iter()
        .any(|v| v == name)
        || !record.grants.capabilities.iter().any(|v| v == name)
        || project.is_some_and(|id| {
            !record.grants.project_ids.iter().any(|v| v == id)
                || caller
                    .project_ids
                    .as_ref()
                    .is_some_and(|ids| !ids.iter().any(|v| v == id))
        })
        || (matches!(name, "threads.send" | "threads.create" | "worktrees.create")
            && caller.control.access == ControlAccess::ReadOnly)
    {
        return Err(
            "permission_denied: Plugin capability or project scope is not authorized".into(),
        );
    }
    Ok(())
}
pub fn validate_grants(
    manifest: &PlatformManifest,
    grants: &super::platform_store::PlatformGrants,
) -> Result<(), String> {
    let requested = manifest.requested_capabilities();
    if grants.capabilities.iter().any(|v| {
        !requested.contains(v)
            || !super::platform_manifest::PLATFORM_CAPABILITIES.contains(&v.as_str())
    }) || grants.project_ids.len() > 1000
        || grants
            .project_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 4096)
    {
        return Err("invalid_input: Grants must name supported requested capabilities and explicit projects".into());
    }
    Ok(())
}
