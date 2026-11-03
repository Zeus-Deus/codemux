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
    /// The name an outside client goes by, shown as the sender of its
    /// messages. Attribution only: it never grants anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A self-asserted client name (`--client`, MCP `clientInfo.name`): attribution, never authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientName {
    slug: String,
    display: String,
}

impl ClientName {
    const MAX: usize = 64;

    /// The slug keys receipts (`local:<slug>`), so one client keeps one
    /// principal across processes: ASCII-lowercased, every run of characters
    /// outside `[a-z0-9._-]` becomes one `-`, leading and trailing `-` and `.`
    /// are trimmed, at most 64 bytes. The display name is the trimmed text
    /// without control characters, at most 64 characters. `None` when nothing
    /// is left of the slug.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut slug = String::new();
        let mut replacing = false;
        for c in raw.chars().map(|c| c.to_ascii_lowercase()) {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                slug.push(c);
                replacing = false;
            } else if !replacing {
                slug.push('-');
                replacing = true;
            }
        }
        let trim = |s: &str| s.trim_matches(|c| c == '-' || c == '.').to_string();
        let mut slug = trim(&slug);
        if slug.len() > Self::MAX {
            slug.truncate(Self::MAX);
            slug = trim(&slug);
        }
        if slug.is_empty() {
            return None;
        }
        let visible: String = raw.chars().filter(|c| !c.is_control()).collect();
        let display = visible.trim().chars().take(Self::MAX).collect::<String>().trim_end().to_string();
        Some(Self { slug, display })
    }
    pub fn slug(&self) -> &str {
        &self.slug
    }
    pub fn display(&self) -> &str {
        &self.display
    }
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
            label: None,
        }
    }

    /// A same-user client on the control socket. Unnamed callers are the
    /// owner; a named one is an outside assistant with its own receipts and
    /// attribution, still governed only by `access`.
    pub fn local(name: Option<&ClientName>, access: ControlAccess) -> Self {
        match name {
            None => Self { access, ..Self::trusted() },
            Some(name) => Self {
                created_by: crate::agent_provider::message::CreatedBy::Agent,
                principal: format!("local:{}", name.slug()),
                access,
                label: Some(name.display().into()),
                ..Self::trusted()
            },
        }
    }

    /// For doors that know a display name (an OAuth client name, a token label).
    pub fn labelled(mut self, label: Option<String>) -> Self {
        self.label = label;
        self
    }

    /// Connector and web-remote callers reach this machine over the network.
    pub fn is_network(&self) -> bool {
        matches!(self.identity, CallerPrincipal::OutsideClient | CallerPrincipal::RemoteClient { .. })
    }

    pub(crate) fn remote(session_id: String) -> Self {
        Self {
            admitted: None,
            principal: format!("web-remote:{session_id}"),
            identity: CallerPrincipal::RemoteClient { session_id },
            created_by: crate::agent_provider::message::CreatedBy::User,
            access: ControlAccess::FullAccess,
            grant_id: None,
            label: None,
        }
    }

    /// Receipt ownership is the authenticated registration, while every action
    /// remains bound to the particular (replaceable/revocable) grant. Its
    /// messages are an assistant's, never shown as the person's.
    pub fn outside(client_id: String, grant_id: String, access: ControlAccess) -> Self {
        Self {
            admitted: None,
            identity: CallerPrincipal::OutsideClient,
            created_by: crate::agent_provider::message::CreatedBy::Agent,
            principal: format!("mcp-client:{client_id}"),
            access,
            grant_id: Some(grant_id),
            label: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::message::CreatedBy;

    fn name(raw: &str) -> ClientName {
        ClientName::parse(raw).unwrap()
    }

    #[test]
    fn client_name_slug_and_display_rules() {
        assert_eq!((name("Claude Code").slug(), name("Claude Code").display()), ("claude-code", "Claude Code"));
        assert_eq!(name("Hermes Agent").slug(), "hermes-agent");
        assert_eq!(name("vexis-watcher").slug(), "vexis-watcher");
        assert_eq!(name("H\u{e9}rmes").slug(), "h-rmes");
        assert_eq!(name("  my  bot!!").slug(), "my-bot");
        for blank in ["", "  ", "---", "..", "\u{e9}\u{e9}"] {
            assert_eq!(ClientName::parse(blank), None, "{blank:?}");
        }
        let long = name(&"a".repeat(200));
        assert_eq!((long.slug().len(), long.display().chars().count()), (64, 64));
        assert_eq!(name(&format!("{}-tail", "b".repeat(63))).slug(), "b".repeat(63), "no trailing dash after truncation");
        assert_eq!(name(" Bot\u{7}\nName\t ").display(), "BotName");
    }

    #[test]
    fn named_local_callers_share_one_principal_across_processes() {
        let hermes = name("hermes");
        let (first, second) = (ControlCaller::local(Some(&hermes), ControlAccess::FullAccess), ControlCaller::local(Some(&name("hermes")), ControlAccess::FullAccess));
        assert_eq!(first, second);
        assert_eq!(first.principal, "local:hermes");
        assert_eq!(first.created_by, CreatedBy::Agent);
        assert_eq!(first.label.as_deref(), Some("hermes"));
        assert_eq!(first.identity, CallerPrincipal::LocalController);
        assert_eq!(ControlCaller::local(Some(&hermes), ControlAccess::ReadOnly).access, ControlAccess::ReadOnly);
    }

    #[test]
    fn unnamed_local_caller_is_the_owner() {
        assert_eq!(ControlCaller::local(None, ControlAccess::FullAccess), ControlCaller::trusted());
        let narrowed = ControlCaller::local(None, ControlAccess::Supervised);
        assert_eq!((narrowed.principal.as_str(), narrowed.created_by, narrowed.access), ("local", CreatedBy::User, ControlAccess::Supervised));
    }

    #[test]
    fn network_callers_are_outside_and_remote_only() {
        assert!(ControlCaller::outside("client".into(), "grant".into(), ControlAccess::ReadOnly).is_network());
        assert!(ControlCaller::remote("session".into()).is_network());
        assert!(!ControlCaller::trusted().is_network());
        assert!(!ControlCaller::local(Some(&name("hermes")), ControlAccess::FullAccess).is_network());
        let thread: ControlCaller = serde_json::from_value(serde_json::json!({
            "principal": "thread:t", "access": "supervised", "grant_id": null,
            "identity": {"kind": "thread", "thread_id": "t", "provider_instance_id": "claude", "provider": "claude", "session_id": "s",
                "authority": {"session_key": "s", "generation": 0, "run_id": null},
                "ceiling": crate::agent_control::permissions::Ceiling::for_mode(crate::agent_provider::ProviderKind::Claude, Some("default"))},
        })).unwrap();
        assert!(thread.thread_id().is_some() && !thread.is_network());
    }

    #[test]
    fn outside_clients_are_not_shown_as_the_user() {
        let outside = ControlCaller::outside("client".into(), "grant".into(), ControlAccess::FullAccess);
        assert_eq!(outside.created_by, CreatedBy::Agent);
        assert_eq!(outside.clone().labelled(Some("hermes-bot".into())).label.as_deref(), Some("hermes-bot"));
        let saved = serde_json::to_value(&outside).unwrap();
        assert!(saved.get("label").is_none(), "an unlabelled caller serializes as before");
        assert_eq!(serde_json::from_value::<ControlCaller>(saved).unwrap(), outside);
    }
}
