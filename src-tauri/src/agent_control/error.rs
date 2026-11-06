//! Typed failure codes for the control plane.
//!
//! Every door reports a failure as `code: message` (and HTTP and stdio also as
//! `structuredContent.error {code, message, retryable}`). The code text is a wire
//! contract: clients branch on it, so a code is never renamed. Codes are
//! registered here up front, so the code that reports them never edits this file.
use serde::{Serialize, Serializer};

macro_rules! codes {
    ($($name:ident => $text:literal),* $(,)?) => {
        /// A stable, snake_case failure reason.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ErrorCode { $($name),* }

        impl ErrorCode {
            pub const ALL: &'static [ErrorCode] = &[$(ErrorCode::$name),*];

            pub const fn as_str(self) -> &'static str {
                match self { $(ErrorCode::$name => $text),* }
            }
        }
    };
}

codes! {
    // Arguments and catalog.
    InvalidArguments => "invalid_arguments",
    InvalidCursor => "invalid_cursor",
    UnknownTool => "unknown_tool",
    PermissionModeRequired => "permission_mode_required",
    UnsupportedOperation => "unsupported_operation",
    SerializationFailed => "serialization_failed",
    // Access and authority.
    AccessDenied => "access_denied",
    AdminRefused => "admin_refused",
    AgentIdentityRequired => "agent_identity_required",
    PermissionDenied => "permission_denied",
    PermissionCeiling => "permission_ceiling",
    PermissionStateUnconfirmed => "permission_state_unconfirmed",
    UnsupportedPermissionCeiling => "unsupported_permission_ceiling",
    ProviderControlDenied => "provider_control_denied",
    StaleCaller => "stale_caller",
    StaleTask => "stale_task",
    StaleOperation => "stale_operation",
    StaleDelivery => "stale_delivery",
    OrchestrationDisabled => "orchestration_disabled",
    // Connector grants.
    ConnectorUnavailable => "connector_unavailable",
    ConnectorDisabled => "connector_disabled",
    GrantRevoked => "grant_revoked",
    GrantChanged => "grant_changed",
    InvalidGrant => "invalid_grant",
    // Targets.
    WorkspaceNotFound => "workspace_not_found",
    NoActiveWorkspace => "no_active_workspace",
    ThreadNotFound => "thread_not_found",
    ThreadOutsideWorkspace => "thread_outside_workspace",
    ThreadNotBound => "thread_not_bound",
    StaleThreadBinding => "stale_thread_binding",
    UnexpectedThreadBinding => "unexpected_thread_binding",
    ImportedSnapshotReadOnly => "imported_snapshot_read_only",
    UnsupportedRemoteWorkspace => "unsupported_remote_workspace",
    OperationNotFound => "operation_not_found",
    DeliveryNotFound => "delivery_not_found",
    InvalidTarget => "invalid_target",
    // Providers and worker options.
    ProviderNotConfigured => "provider_not_configured",
    ProviderNotReady => "provider_not_ready",
    CatalogNotDiscovered => "catalog_not_discovered",
    UnsupportedProvider => "unsupported_provider",
    UnsupportedPermissionMode => "unsupported_permission_mode",
    UnsupportedModel => "unsupported_model",
    UnsupportedModelOption => "unsupported_model_option",
    UnsupportedEffort => "unsupported_effort",
    UnsupportedContextWindow => "unsupported_context_window",
    UnsupportedServiceTier => "unsupported_service_tier",
    UnsupportedFastMode => "unsupported_fast_mode",
    StaleProviderSelection => "stale_provider_selection",
    StaleProviderCallback => "stale_provider_callback",
    // Operations and receipts.
    RequestKeyConflict => "request_key_conflict",
    ResourceLimit => "resource_limit",
    OperationAdmissionFailed => "operation_admission_failed",
    OperationLookupFailed => "operation_lookup_failed",
    OperationFailed => "operation_failed",
    RequestCancelled => "request_cancelled",
    PluginOperationFailed => "plugin_operation_failed",
    NativeControlFailed => "native_control_failed",
    // Workspaces and worktrees.
    UnknownProjectRoot => "unknown_project_root",
    NotAGitRepository => "not_a_git_repository",
    WorktreeConflict => "worktree_conflict",
    WorktreeBaseUnavailable => "worktree_base_unavailable",
    WorktreeCreateFailed => "worktree_create_failed",
    WorkspaceCreateFailed => "workspace_create_failed",
    WorkspaceCloseRefused => "workspace_close_refused",
    // Worker questions answered from outside.
    QuestionNotFound => "question_not_found",
    QuestionClosed => "question_closed",
    QuestionAnswerFailed => "question_answer_failed",
    // Transports and protocol.
    AppUnavailable => "app_unavailable",
    OutcomeUnknown => "outcome_unknown",
    ServerTooOld => "server_too_old",
    WaitBudgetExhausted => "wait_budget_exhausted",
    UnsupportedProtocolVersion => "unsupported_protocol_version",
    HeaderMismatch => "header_mismatch",
}

impl ErrorCode {
    /// A bare code (`"access_denied"`), not `code: message`. A code this build
    /// does not know (a newer server) is `None`; callers keep the raw text.
    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|known| known.as_str() == code)
    }

    /// Retrying the identical request later may succeed without any change by
    /// the caller or the person. A refusal before admission keeps its
    /// client_request_id free; a failed receipt is final for its key, so that
    /// retry needs a new one.
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::ConnectorUnavailable
                | Self::ProviderNotReady
                | Self::ResourceLimit
                | Self::OperationAdmissionFailed
                | Self::OperationLookupFailed
                | Self::AppUnavailable
                | Self::WaitBudgetExhausted
        )
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl PartialEq<&str> for ErrorCode {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[derive(Debug, Clone)]
pub struct ControlError {
    pub code: ErrorCode,
    pub message: String,
}

impl ControlError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn retryable(&self) -> bool {
        self.code.retryable()
    }

    /// A string error from a lower layer. A known `code: …` prefix keeps its
    /// code; anything else becomes `fallback` with the text unchanged.
    pub fn from_text(text: String, fallback: ErrorCode) -> Self {
        match text.split_once(": ").and_then(|(code, message)| Some((ErrorCode::parse(code)?, message))) {
            Some((code, message)) => Self::new(code, message),
            None => Self::new(fallback, text),
        }
    }
}

impl From<String> for ControlError {
    fn from(text: String) -> Self {
        Self::from_text(text, ErrorCode::NativeControlFailed)
    }
}

impl Serialize for ControlError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut error = serializer.serialize_struct("ControlError", 3)?;
        error.serialize_field("code", &self.code)?;
        error.serialize_field("message", &self.message)?;
        error.serialize_field("retryable", &self.retryable())?;
        error.end()
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ControlError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_codes_round_trip_through_strings() {
        let mut seen = std::collections::HashSet::new();
        for code in ErrorCode::ALL {
            let text = code.as_str();
            assert!(seen.insert(text), "{text} is registered twice");
            assert!(!text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{text} is not snake_case");
            assert_eq!(ErrorCode::parse(text), Some(*code));
            assert_eq!(code.to_string(), text);
            assert_eq!(serde_json::to_value(code).unwrap(), json!(text));
        }
        assert_eq!(ErrorCode::parse("access_denied: no"), None, "parse takes a bare code");
        assert_eq!(ErrorCode::parse("no_such_code"), None);
    }

    #[test]
    fn string_errors_keep_known_code_prefixes() {
        let known = ControlError::from("stale_caller: x".to_string());
        assert_eq!((known.code, known.message.as_str()), (ErrorCode::StaleCaller, "x"));
        let plain = ControlError::from("boom".to_string());
        assert_eq!((plain.code, plain.message.as_str()), (ErrorCode::NativeControlFailed, "boom"));
        let unknown = ControlError::from_text("unknown_word: x".into(), ErrorCode::OperationLookupFailed);
        assert_eq!((unknown.code, unknown.message.as_str()), (ErrorCode::OperationLookupFailed, "unknown_word: x"));
        assert_eq!(known.to_string(), "stale_caller: x");
    }

    #[test]
    fn retryable_codes_are_exactly_the_transient_ones() {
        let retryable: Vec<_> = ErrorCode::ALL.iter().filter(|code| code.retryable()).map(|code| code.as_str()).collect();
        assert_eq!(retryable, [
            "connector_unavailable", "provider_not_ready", "resource_limit", "operation_admission_failed",
            "operation_lookup_failed", "app_unavailable", "wait_budget_exhausted",
        ]);
    }

    #[test]
    fn control_error_serializes_code_message_retryable() {
        let error = ControlError::new(ErrorCode::WaitBudgetExhausted, "Too many waits");
        assert_eq!(serde_json::to_value(&error).unwrap(), json!({"code":"wait_budget_exhausted","message":"Too many waits","retryable":true}));
        assert!(error.code == "wait_budget_exhausted");
        let fixed = ControlError::new(ErrorCode::WorkspaceNotFound, "gone");
        assert_eq!(serde_json::to_value(&fixed).unwrap()["retryable"], false);
    }
}
