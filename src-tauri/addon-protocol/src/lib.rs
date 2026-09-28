//! Shared, inert contracts. This crate never executes package code.
pub mod limits;
pub mod catalog;
pub mod manifest;
pub mod ui;
pub mod wire;
pub use manifest::Manifest;
pub use wire::{ErrorCode, ProtocolError};
