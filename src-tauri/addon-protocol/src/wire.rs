use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    PermissionDenied,
    ContextStale,
    NoWorkspace,
    RemoteUnsupported,
    NoComposer,
    InteractionRequired,
    NotAGitRepo,
    IncompatibleApi,
    ResourceLimit,
    Timeout,
    PluginStopped,
    InvalidMessage,
    CredentialRequired,
    NetworkDenied,
    StorageUnavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorData {
    pub code: ErrorCode,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolError {
    pub code: i32,
    pub message: String,
    pub data: ErrorData,
}
impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
            data: ErrorData { code },
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidMessage, message)
    }
}
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.data.code, self.message)
    }
}
impl std::error::Error for ProtocolError {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub jsonrpc: String,
    pub generation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}
impl Envelope {
    pub fn parse(
        bytes: &[u8],
        generation: Option<&str>,
        from_child: bool,
    ) -> Result<Self, ProtocolError> {
        if bytes.len() > super::limits::FRAME {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Frame too large",
            ));
        }
        let raw: Value =
            serde_json::from_slice(bytes).map_err(|_| ProtocolError::invalid("Invalid JSON"))?;
        let message: Self = serde_json::from_value(raw.clone())
            .map_err(|_| ProtocolError::invalid("Invalid envelope"))?;
        if message.jsonrpc != "2.0"
            || message.generation.is_empty()
            || message.generation.len() > 128
            || generation.is_some_and(|g| g != message.generation)
        {
            return Err(ProtocolError::invalid("Wrong protocol or generation"));
        }
        if let Some(method) = &message.method {
            let methods: &[&str] = if from_child {
                &["ready", "ui.patch", "host.request", "log"]
            } else {
                &[
                    "initialize",
                    "activate",
                    "command.execute",
                    "view.mount",
                    "view.unmount",
                    "ui.event",
                    "workspace.changed",
                    "settings.changed",
                    "deactivate",
                ]
            };
            if !methods.contains(&method.as_str())
                || raw.get("result").is_some()
                || message.error.is_some()
                || !message.params.as_ref().is_some_and(Value::is_object)
            {
                return Err(ProtocolError::invalid("Invalid method or parameters"));
            }
        } else if message.id.is_none()
            || (raw.get("result").is_some() == message.error.is_some())
            || message.params.is_some()
        {
            return Err(ProtocolError::invalid("Invalid response"));
        }
        Ok(message)
    }
    pub fn response(generation: &str, id: u64, result: Result<Value, ProtocolError>) -> Self {
        let (result, error) = match result {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        Self {
            jsonrpc: "2.0".into(),
            generation: generation.into(),
            id: Some(id),
            method: None,
            params: None,
            result,
            error,
        }
    }
}

/// Never allocate an unbounded line. A rejected frame terminates its transport.
pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Partial frame",
                ))
            };
        }
        let end = bytes.iter().position(|b| *b == b'\n');
        let count = end.unwrap_or(bytes.len());
        if line.len() + count > super::limits::FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Frame limit"));
        }
        line.extend_from_slice(&bytes[..count]);
        reader.consume(count + usize::from(end.is_some()));
        if end.is_some() {
            return Ok(Some(line));
        }
    }
}
