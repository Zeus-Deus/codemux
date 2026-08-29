//! Per-instance identifier shim for forward-compatible multi-instance
//! support.
//!
//! v1 (Stage 1) collapses the reference distinction between
//! `ProviderDriverKind` and `ProviderInstanceId` so each driver has
//! exactly one instance and the instance id is just the driver slug
//! (`"claude"`, `"codex"`, `"opencode"`). This is intentional:
//!
//! * The picker, capabilities store, and chat-pane provider field all
//!   currently key off `ProviderKind`. Lifting them to a richer
//!   `(driver, instance)` tuple ripples through every chat surface and
//!   the Step 10 settings-sync schema, which is way out of scope for
//!   the OpenCode scaffold.
//! * For OpenCode specifically, upstream credentials live in
//!   `~/.config/opencode/`, not in Codemux. Multi-instance would only
//!   differentiate the optional server password, which a typical user
//!   never changes — so the v2 lift has near-zero user-visible
//!   payoff today.
//!
//! Stage 1 lands the type so future call sites can already speak
//! `ProviderInstanceId` without exporting a richer model. v2 changes
//! the constructor to read from a per-driver config map; the public
//! struct stays the same.
//!
//! # Why the type and not just a `String`?
//!
//! Because every payload that crosses the IPC boundary has to make a
//! choice between "raw string" and "newtype". Locking the newtype now
//! means future callers can't accidentally swap an instance id for a
//! free-form provider label. The serialization format
//! (`#[serde(transparent)]`) stays string so JSON wire compatibility is
//! preserved when v2 lands.

use serde::{Deserialize, Serialize};

use crate::agent_provider::types::ProviderKind;

/// User-named instance routing turns. v1 maps 1:1 with [`ProviderKind`]
/// (driver === instance) so this is effectively a typed alias for the
/// driver slug, but every site that needs to disambiguate `(driver,
/// instance)` tuples in v2 can already accept this type.
///
/// Serialized as a transparent string so the wire format is
/// `"claude"` / `"codex"` / `"opencode"` and not a struct envelope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderInstanceId(pub String);

impl ProviderInstanceId {
    /// Stage 1 mapping: instance id is the driver slug.
    ///
    /// Lowercase strings match `ProviderKind`'s
    /// `#[serde(rename_all = "lowercase")]` so the serialized
    /// representations are identical and JSON payloads carrying
    /// either field decode interchangeably while v2 isn't shipped.
    pub fn from_driver(driver: ProviderKind) -> Self {
        let slug = match driver {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
            ProviderKind::Cursor => "cursor",
            ProviderKind::Grok => "grok",
            ProviderKind::OpenCode => "opencode",
        };
        Self(slug.to_string())
    }

    /// Borrow the inner slug. Useful at telemetry sites that don't
    /// want to clone the `String`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_driver_round_trips_each_variant() {
        // Pinned: every variant must round-trip. Adding a new
        // ProviderKind without updating from_driver is a compile-time
        // error today (match is exhaustive); this test catches the
        // "developer added the arm but typo'd the slug" case.
        let cases = [
            (ProviderKind::Claude, "claude"),
            (ProviderKind::Codex, "codex"),
            (ProviderKind::Cursor, "cursor"),
            (ProviderKind::Grok, "grok"),
            (ProviderKind::OpenCode, "opencode"),
        ];
        for (driver, slug) in cases {
            let id = ProviderInstanceId::from_driver(driver);
            assert_eq!(id.as_str(), slug);
        }
    }

    #[test]
    fn instance_id_serialises_as_bare_string() {
        // Pinned: the wire format is `"opencode"`, not
        // `{"0":"opencode"}`. Step 10 settings-sync and the eventual
        // v2 multi-instance lift both depend on this — the JSON
        // shape must NOT become an envelope.
        let id = ProviderInstanceId::from_driver(ProviderKind::OpenCode);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"opencode\"");
    }

    #[test]
    fn instance_id_deserialises_from_bare_string() {
        let id: ProviderInstanceId = serde_json::from_str("\"claude\"").unwrap();
        assert_eq!(id.as_str(), "claude");
    }

    #[test]
    fn instance_id_matches_provider_kind_serialisation() {
        // Co-locks ProviderKind <-> ProviderInstanceId so future v2
        // payloads can carry either field without duplicating the
        // slug map.
        let driver_json = serde_json::to_string(&ProviderKind::OpenCode).unwrap();
        let instance_json =
            serde_json::to_string(&ProviderInstanceId::from_driver(ProviderKind::OpenCode))
                .unwrap();
        assert_eq!(driver_json, instance_json);
    }
}
