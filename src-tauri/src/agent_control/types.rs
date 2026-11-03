use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAccess {
    ReadOnly,
    Supervised,
    FullAccess,
}

impl ControlAccess {
    /// ReadOnly < Supervised < FullAccess. An explicit rank rather than a
    /// derived `Ord`, so reordering the variants can never change who may do what.
    pub fn rank(self) -> u8 {
        match self {
            ControlAccess::ReadOnly => 0,
            ControlAccess::Supervised => 1,
            ControlAccess::FullAccess => 2,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallerPrincipal {
    LocalController,
    /// Established by the web-remote transport, never accepted from request arguments.
    RemoteClient { session_id: String },
    #[default]
    OutsideClient,
    Thread {
        thread_id: String,
        provider_instance_id: String,
        provider: crate::agent_provider::ProviderKind,
        session_id: String,
        authority: crate::delegation::authority::AuthorityIdentity,
        ceiling: super::permissions::Ceiling,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdmittedAuthority {
    Task { task_id: String },
    Delivery { delivery_id: String },
    Operation { operation_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlCaller {
    #[serde(default)]
    pub created_by: crate::agent_provider::message::CreatedBy,
    pub principal: String,
    #[serde(default)]
    pub identity: CallerPrincipal,
    #[serde(default)]
    pub(crate) admitted: Option<AdmittedAuthority>,
    pub access: ControlAccess,
    pub grant_id: Option<String>,
}

impl ControlCaller {
    pub(crate) async fn revoked(&self) {
        if let CallerPrincipal::Thread {
            thread_id,
            authority,
            ceiling,
            ..
        } = &self.identity
        {
            authority.wait_revoked(thread_id, *ceiling).await;
        } else {
            std::future::pending::<()>().await;
        }
    }
    pub(crate) fn admit<T>(&self, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        match (&self.identity, &self.admitted) {
            (
                CallerPrincipal::Thread {
                    thread_id,
                    authority,
                    ..
                },
                None,
            ) => authority.admit(thread_id, work),
            _ => work(),
        }
    }
    pub fn thread_id(&self) -> Option<&str> {
        match &self.identity {
            CallerPrincipal::Thread { thread_id, .. } => Some(thread_id),
            _ => None,
        }
    }
    pub(crate) fn admission_generation(&self) -> Option<u64> {
        match &self.identity {
            CallerPrincipal::Thread { authority, .. } => Some(authority.stop_generation),
            _ => None,
        }
    }
    /// What the access gate and tool listing treat this caller as. Named
    /// local clients stay owners: their name is attribution, `access`
    /// governs what they may do.
    pub fn kind(&self) -> super::CallerKind {
        match &self.identity {
            CallerPrincipal::LocalController | CallerPrincipal::RemoteClient { .. } => super::CallerKind::Owner,
            CallerPrincipal::OutsideClient => super::CallerKind::Client,
            CallerPrincipal::Thread { ceiling, .. } => super::CallerKind::Agent { full_access: ceiling.full_access() },
        }
    }
    pub fn ceiling(&self) -> Option<super::permissions::Ceiling> {
        match &self.identity {
            CallerPrincipal::Thread { ceiling, .. } => Some(*ceiling),
            _ => None,
        }
    }
    /// Local socket access already confers application control authority.
    pub fn trusted() -> Self {
        Self {
            admitted: None,
            identity: CallerPrincipal::LocalController,
            created_by: crate::agent_provider::message::CreatedBy::User,
            principal: "local".into(),
            access: ControlAccess::FullAccess,
            grant_id: None,
        }
    }

    pub(crate) fn remote(session_id: String) -> Self {
        Self {
            admitted: None,
            principal: format!("web-remote:{session_id}"),
            identity: CallerPrincipal::RemoteClient { session_id },
            created_by: crate::agent_provider::message::CreatedBy::User,
            access: ControlAccess::FullAccess,
            grant_id: None,
        }
    }

    /// Receipt ownership is the authenticated registration, while every action
    /// remains bound to the particular (replaceable/revocable) grant.
    pub fn outside(client_id: String, grant_id: String, access: ControlAccess) -> Self {
        Self {
            admitted: None,
            identity: CallerPrincipal::OutsideClient,
            created_by: crate::agent_provider::message::CreatedBy::User,
            principal: format!("mcp-client:{client_id}"),
            access,
            grant_id: Some(grant_id),
        }
    }
}
