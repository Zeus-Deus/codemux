//! Access classes for registry tools and control-socket commands, and the one
//! gate that checks them.
//!
//! The gate is coarse: it runs before any handler. Handlers that resolve a
//! target thread still apply that thread's permission ceiling
//! (`service::permission_ceiling`).
use super::{permissions::Ceiling, ControlAccess, ControlCaller, ControlError, ErrorCode};
use crate::agent_provider::ProviderKind;
use serde::Serialize;

/// Which route lists and serves a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    /// The explicit-argument contract: socket, stdio, HTTP, in-process callers.
    Outside,
    /// In-app chats' own tools.
    InApp,
    Both,
}

/// What a tool or command may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Reads state. Every caller.
    Read,
    /// Changes native threads. Not for read-only connections.
    Write,
    /// Starts native threads. Not for read-only connections; the handler
    /// limits the requested mode to the caller's ceiling.
    StartsThreads,
    /// Changes this machine outside any thread's sandbox: opens, creates or
    /// closes workspaces and worktrees (setup scripts run). Full-access
    /// connections and chats with a full-access ceiling only.
    Machine,
    /// Changes who may control Codemux or what runs without the person
    /// (connector grants, web remote, automations). Only the owner at full
    /// access; never an agent or a connector client.
    Admin,
}

/// Who is calling, as far as visibility and the access gate care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKind {
    /// The person: the local socket (named local clients too; their name is
    /// attribution, `access` governs them) and the paired web remote.
    Owner,
    /// An OAuth connector client.
    Client,
    /// A Codemux-started agent chat, with its effective ceiling.
    Agent { full_access: bool },
}

/// A sync, app-free view of a caller, used to list tools and to gate calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallerView {
    /// `Outside` or `InApp`, never `Both`.
    pub audience: Audience,
    pub access: ControlAccess,
    pub kind: CallerKind,
    /// Agent collaboration (Settings). It only hides collaboration tools from agents.
    pub collaboration: bool,
}

impl Access {
    pub fn check(self, view: &CallerView) -> Result<(), ControlError> {
        let allowed = match (self, view.kind) {
            (Access::Read, _) => true,
            (Access::Write | Access::StartsThreads, _) => view.access != ControlAccess::ReadOnly,
            (Access::Machine, CallerKind::Agent { full_access }) => full_access,
            (Access::Machine, _) => view.access == ControlAccess::FullAccess,
            (Access::Admin, CallerKind::Owner) => view.access == ControlAccess::FullAccess,
            (Access::Admin, _) => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(ControlError::new(ErrorCode::AccessDenied, self.denial()))
        }
    }

    /// Callers are offered only what they can run.
    pub fn listed(self, view: &CallerView) -> bool {
        self.check(view).is_ok()
    }

    /// The least access an owner or connector client needs.
    pub fn minimum(self) -> ControlAccess {
        match self {
            Access::Read => ControlAccess::ReadOnly,
            Access::Write | Access::StartsThreads => ControlAccess::Supervised,
            Access::Machine | Access::Admin => ControlAccess::FullAccess,
        }
    }

    fn denial(self) -> &'static str {
        match self {
            Access::Read | Access::Write | Access::StartsThreads => "Read-only connections cannot modify native threads",
            Access::Machine => "This tool changes workspaces on this machine; it needs a full-access connection or a full-access chat",
            Access::Admin => "Only the Codemux owner at full access can run this; agents and connector clients cannot",
        }
    }
}

impl CallerView {
    pub fn of(caller: &ControlCaller, audience: Audience) -> Self {
        Self { audience, access: caller.access, kind: caller.kind(), collaboration: true }
    }

    /// An outside caller without an app to ask (stdio lists before it connects).
    pub fn outside(access: ControlAccess) -> Self {
        Self { audience: Audience::Outside, access, kind: CallerKind::Owner, collaboration: true }
    }

    /// An in-app chat. Its ceiling comes from the mode, the same predicate
    /// that decides which legacy self tools it sees (`native_tool_visible`).
    pub fn in_app(provider: ProviderKind, permission_mode: Option<&str>, collaboration: bool) -> Self {
        let full_access = Ceiling::for_mode(provider, permission_mode).is_some_and(Ceiling::full_access);
        Self {
            audience: Audience::InApp,
            access: ControlAccess::Supervised,
            kind: CallerKind::Agent { full_access },
            collaboration,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ControlAccess::{FullAccess, ReadOnly, Supervised};

    const OWNER: CallerKind = CallerKind::Owner;
    const CLIENT: CallerKind = CallerKind::Client;
    const FULL_AGENT: CallerKind = CallerKind::Agent { full_access: true };
    const AGENT: CallerKind = CallerKind::Agent { full_access: false };

    #[test]
    fn access_gate_matrix() {
        const ANY: &[ControlAccess] = &[ReadOnly, Supervised, FullAccess];
        const WRITERS: &[ControlAccess] = &[Supervised, FullAccess];
        const FULL: &[ControlAccess] = &[FullAccess];
        const NONE: &[ControlAccess] = &[];
        // (class, caller kind, the access levels that pass)
        let table: &[(Access, CallerKind, &[ControlAccess])] = &[
            (Access::Read, OWNER, ANY), (Access::Read, CLIENT, ANY), (Access::Read, FULL_AGENT, ANY), (Access::Read, AGENT, ANY),
            (Access::Write, OWNER, WRITERS), (Access::Write, CLIENT, WRITERS), (Access::Write, FULL_AGENT, WRITERS), (Access::Write, AGENT, WRITERS),
            (Access::StartsThreads, OWNER, WRITERS), (Access::StartsThreads, CLIENT, WRITERS),
            (Access::StartsThreads, FULL_AGENT, WRITERS), (Access::StartsThreads, AGENT, WRITERS),
            (Access::Machine, OWNER, FULL), (Access::Machine, CLIENT, FULL), (Access::Machine, FULL_AGENT, ANY), (Access::Machine, AGENT, NONE),
            (Access::Admin, OWNER, FULL), (Access::Admin, CLIENT, NONE), (Access::Admin, FULL_AGENT, NONE), (Access::Admin, AGENT, NONE),
        ];
        assert_eq!(table.len(), 5 * 4);
        for (class, kind, passing) in table {
            for access in [ReadOnly, Supervised, FullAccess] {
                for audience in [Audience::Outside, Audience::InApp] {
                    let view = CallerView { audience, access, kind: *kind, collaboration: true };
                    let result = class.check(&view);
                    assert_eq!(result.is_ok(), passing.contains(&access), "{class:?} {kind:?} {access:?}");
                    assert_eq!(class.listed(&view), result.is_ok(), "listing follows the gate");
                    if let Err(error) = result {
                        assert_eq!(error.code, ErrorCode::AccessDenied);
                        assert_eq!(error.message, class.denial());
                    }
                }
            }
        }
        assert_eq!(Access::Write.check(&CallerView::outside(ReadOnly)).unwrap_err().to_string(),
            "access_denied: Read-only connections cannot modify native threads");
    }

    #[test]
    fn minimum_access_is_what_an_owner_needs() {
        for class in [Access::Read, Access::Write, Access::StartsThreads, Access::Machine, Access::Admin] {
            for access in [ReadOnly, Supervised, FullAccess] {
                assert_eq!(class.check(&CallerView::outside(access)).is_ok(), access.rank() >= class.minimum().rank(), "{class:?} {access:?}");
            }
        }
        assert_eq!(serde_json::to_value(Access::StartsThreads).unwrap(), "starts_threads");
    }

    #[test]
    fn caller_views_carry_the_callers_kind_and_ceiling() {
        let owner = CallerView::of(&ControlCaller::trusted(), Audience::Outside);
        assert_eq!((owner.kind, owner.access, owner.audience), (OWNER, FullAccess, Audience::Outside));
        let client = CallerView::of(&ControlCaller::outside("c".into(), "g".into(), ReadOnly), Audience::Outside);
        assert_eq!((client.kind, client.access), (CLIENT, ReadOnly));
        assert_eq!(CallerView::of(&ControlCaller::remote("s".into()), Audience::Outside).kind, OWNER);
        let named = crate::agent_control::ClientName::parse("Hermes").unwrap();
        assert_eq!(CallerView::of(&ControlCaller::local(Some(&named), Supervised), Audience::Outside).kind, OWNER);
        let full = CallerView::in_app(ProviderKind::Claude, Some("bypassPermissions"), true);
        assert_eq!((full.kind, full.access, full.audience), (FULL_AGENT, Supervised, Audience::InApp));
        assert_eq!(CallerView::in_app(ProviderKind::Codex, Some("plan"), false).kind, AGENT);
        assert!(!CallerView::in_app(ProviderKind::Codex, Some("plan"), false).collaboration);
        assert_eq!(CallerView::in_app(ProviderKind::Claude, Some("no-such-mode"), true).kind, AGENT, "unknown modes fail closed");
    }
}
