//! User-defined, standard ACP v1 subprocesses. Native adapters remain separate.
pub mod catalog;
pub mod config;
mod runtime;

pub use catalog::{AcpAuthMethod, AcpBinding, AcpCatalog, AcpConfigOption, AcpConfigValue};
pub use config::{AcpAgent, AcpAgentInput, AcpLaunchConfig};
pub use runtime::{AcpCatalogChanged, AcpStore, GenericAcpProvider};

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "../../../tests/helpers/custom_acp_python.rs"]
mod test_python;
