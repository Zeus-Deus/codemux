//! OpenCode CLI 1.x and 2.x are different HTTP protocols, not SDK versions.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpenCodeApi {
    #[default]
    Legacy,
    V2,
}

impl OpenCodeApi {
    pub fn from_ready_banner(line: &str) -> Option<Self> {
        let line = line.trim_start();
        if line.starts_with("opencode server listening") {
            Some(Self::Legacy)
        } else if line.starts_with("server listening") {
            Some(Self::V2)
        } else {
            None
        }
    }

    pub fn route<'a>(self, legacy: &'a str, v2: &'a str) -> &'a str {
        match self {
            Self::Legacy => legacy,
            Self::V2 => v2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_selects_the_cli_protocol_not_the_sdk_version() {
        assert_eq!(
            OpenCodeApi::from_ready_banner("opencode server listening on http://127.0.0.1:1234"),
            Some(OpenCodeApi::Legacy)
        );
        assert_eq!(
            OpenCodeApi::from_ready_banner("  server listening on http://127.0.0.1:1234"),
            Some(OpenCodeApi::V2)
        );
        assert_eq!(
            OpenCodeApi::from_ready_banner("unrelated server listening on http://127.0.0.1:1234"),
            None
        );
    }

    #[test]
    fn routes_keep_legacy_and_v2_contracts_separate() {
        assert_eq!(
            OpenCodeApi::Legacy.route("/session/x/abort", "/api/session/x/interrupt"),
            "/session/x/abort"
        );
        assert_eq!(
            OpenCodeApi::V2.route("/session/x/abort", "/api/session/x/interrupt"),
            "/api/session/x/interrupt"
        );
    }
}
