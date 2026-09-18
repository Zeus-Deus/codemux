//! Optional desktop feature plugins. State and authority are independent of core sync.
pub mod credentials;
pub mod download;
pub mod development;
pub mod catalog;
pub mod cleanup;
pub mod git;
pub mod http;
pub mod lifecycle;
pub mod manager;
pub mod package;
pub mod permissions;
pub mod protocol;
pub mod storage;
pub mod workspace;
pub use codemux_addon_protocol::{ErrorCode, Manifest, ProtocolError};
pub type Result<T> = std::result::Result<T, ProtocolError>;
