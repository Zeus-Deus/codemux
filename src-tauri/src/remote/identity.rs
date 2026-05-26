//! Caller identity carried on every dispatched request.
//!
//! v1 only ever produces [`Identity::Local`] — any caller that
//! presents the manifest's bearer secret is treated as a fully
//! trusted local user. The variant exists so a future cloud-relay
//! integration can attach `Identity::Cloud { user_id, org_id, role }`
//! parsed from a forwarded auth header, **without** changing the
//! signature of any handler.
//!
//! Handlers should not branch on the variant in v1. They take it as
//! a parameter and ignore the value. The point is that the parameter
//! is *there*, ready to be consumed later.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Identity {
    /// Caller that authenticated by presenting the manifest's local
    /// bearer secret. The only variant in v1.
    Local,

    /// Reserved for a future cloud-relay deployment. The relay
    /// terminates user authentication (JWT validation against Better
    /// Auth or equivalent), then forwards a trusted identity header
    /// to the daemon, which deserialises it into this variant. v1
    /// handlers don't construct this and don't branch on it; it's
    /// here so callers can be added later without a signature change.
    #[allow(dead_code)]
    Cloud {
        user_id: String,
        org_id: String,
        role: String,
    },
}

impl Identity {
    /// Convenience for the v1 case.
    pub fn local() -> Self {
        Self::Local
    }
}
