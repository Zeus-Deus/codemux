//! Dedicated guarded turn path; ordinary RPC writers are unchanged.
use std::{io, task::Poll};
pub type WritePoll = Poll<io::Result<usize>>;
pub trait DispatchGuard: Send + Sync + std::fmt::Debug {
    /// Run the actual first-byte poll under synchronous activity/admission.
    /// Pending releases admission and the next poll must revalidate.
    fn register(&self, _waker: &std::task::Waker) {}
    fn poll_first(&self, write: &mut dyn FnMut() -> WritePoll) -> Result<WritePoll, String>;
    fn accepted(&self) -> Result<(), String> {
        Ok(())
    }
}
