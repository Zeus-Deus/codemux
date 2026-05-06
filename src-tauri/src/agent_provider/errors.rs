//! Provider error taxonomy.
//!
//! [`ProviderError`] is the error type returned from every fallible method on
//! [`AgentProvider`](super::AgentProvider). It is deliberately not
//! `Serialize` so callers are forced to think about which fields belong on
//! the wire; use [`ProviderError::to_serializable`] when a JSON-friendly
//! form is needed.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::types::{ProviderKind, ThreadId};

/// All ways an [`AgentProvider`](super::AgentProvider) call can fail.
#[derive(Debug)]
pub enum ProviderError {
    /// The underlying CLI is not installed on the user's machine.
    NotInstalled {
        provider: ProviderKind,
        /// User-visible hint describing how to install the CLI.
        hint: String,
    },
    /// The CLI is installed but the user is not logged in.
    NotAuthenticated {
        provider: ProviderKind,
        hint: String,
    },
    /// The referenced thread has no live session binding.
    SessionNotFound { thread_id: ThreadId },
    /// The session existed but has already been closed.
    SessionClosed { thread_id: ThreadId },
    /// Inputs failed adapter-level validation (missing required fields,
    /// unsupported mode, etc.).
    ValidationError { message: String },
    /// Subprocess-level failure (spawn, pipe, exit).
    ProcessError {
        message: String,
        /// Optional stringified underlying error for diagnostics.
        source: Option<String>,
    },
    /// RPC-layer failure (malformed response, protocol violation, ...).
    RpcError { message: String },
    /// An operation exceeded its deadline.
    Timeout { operation: String, elapsed_ms: u64 },
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled { provider, hint } => {
                write!(f, "{provider:?} CLI is not installed: {hint}")
            }
            Self::NotAuthenticated { provider, hint } => {
                write!(f, "{provider:?} CLI is not authenticated: {hint}")
            }
            Self::SessionNotFound { thread_id } => {
                write!(f, "no live session bound to thread {:?}", thread_id.0)
            }
            Self::SessionClosed { thread_id } => {
                write!(f, "session for thread {:?} is closed", thread_id.0)
            }
            Self::ValidationError { message } => write!(f, "validation error: {message}"),
            Self::ProcessError { message, source } => match source {
                Some(src) => write!(f, "process error: {message} ({src})"),
                None => write!(f, "process error: {message}"),
            },
            Self::RpcError { message } => write!(f, "rpc error: {message}"),
            Self::Timeout {
                operation,
                elapsed_ms,
            } => {
                write!(f, "{operation} timed out after {elapsed_ms}ms")
            }
        }
    }
}

impl std::error::Error for ProviderError {}

/// JSON-friendly projection of [`ProviderError`] for IPC / logging.
///
/// Mirrors the tags in [`ProviderError`] but carries only plain serializable
/// fields — notably, error-chain pointers are flattened into a string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SerializableProviderError {
    NotInstalled {
        provider: ProviderKind,
        hint: String,
    },
    NotAuthenticated {
        provider: ProviderKind,
        hint: String,
    },
    SessionNotFound {
        thread_id: ThreadId,
    },
    SessionClosed {
        thread_id: ThreadId,
    },
    ValidationError {
        message: String,
    },
    ProcessError {
        message: String,
        source: Option<String>,
    },
    RpcError {
        message: String,
    },
    Timeout {
        operation: String,
        elapsed_ms: u64,
    },
}

impl ProviderError {
    /// Produce a JSON-safe copy of this error suitable for transport across
    /// an IPC surface.
    pub fn to_serializable(&self) -> SerializableProviderError {
        match self {
            Self::NotInstalled { provider, hint } => SerializableProviderError::NotInstalled {
                provider: *provider,
                hint: hint.clone(),
            },
            Self::NotAuthenticated { provider, hint } => {
                SerializableProviderError::NotAuthenticated {
                    provider: *provider,
                    hint: hint.clone(),
                }
            }
            Self::SessionNotFound { thread_id } => SerializableProviderError::SessionNotFound {
                thread_id: thread_id.clone(),
            },
            Self::SessionClosed { thread_id } => SerializableProviderError::SessionClosed {
                thread_id: thread_id.clone(),
            },
            Self::ValidationError { message } => SerializableProviderError::ValidationError {
                message: message.clone(),
            },
            Self::ProcessError { message, source } => SerializableProviderError::ProcessError {
                message: message.clone(),
                source: source.clone(),
            },
            Self::RpcError { message } => SerializableProviderError::RpcError {
                message: message.clone(),
            },
            Self::Timeout {
                operation,
                elapsed_ms,
            } => SerializableProviderError::Timeout {
                operation: operation.clone(),
                elapsed_ms: *elapsed_ms,
            },
        }
    }
}

impl From<SerializableProviderError> for ProviderError {
    fn from(value: SerializableProviderError) -> Self {
        match value {
            SerializableProviderError::NotInstalled { provider, hint } => {
                Self::NotInstalled { provider, hint }
            }
            SerializableProviderError::NotAuthenticated { provider, hint } => {
                Self::NotAuthenticated { provider, hint }
            }
            SerializableProviderError::SessionNotFound { thread_id } => {
                Self::SessionNotFound { thread_id }
            }
            SerializableProviderError::SessionClosed { thread_id } => {
                Self::SessionClosed { thread_id }
            }
            SerializableProviderError::ValidationError { message } => {
                Self::ValidationError { message }
            }
            SerializableProviderError::ProcessError { message, source } => {
                Self::ProcessError { message, source }
            }
            SerializableProviderError::RpcError { message } => Self::RpcError { message },
            SerializableProviderError::Timeout {
                operation,
                elapsed_ms,
            } => Self::Timeout {
                operation,
                elapsed_ms,
            },
        }
    }
}
