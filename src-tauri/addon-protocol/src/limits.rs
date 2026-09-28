use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
pub const FRAME: usize = 1024 * 1024;
pub const HEAP: usize = 64 * 1024 * 1024;
pub const STACK: usize = 512 * 1024;
pub const BUNDLE: usize = 5 * 1024 * 1024;
pub const MANIFEST: usize = 64 * 1024;
pub const TREE: usize = 256 * 1024;
pub const NODES: usize = 2000;
pub const DEPTH: usize = 32;
pub const MUTATIONS: usize = 1000;
pub const CALLBACKS: usize = 4096;
pub const TIMERS: usize = 128;
pub const VIEWS: usize = 4;

/// Sliding windows, supplied clock for deterministic quota tests.
#[derive(Default)]
pub struct RateLimit {
    events: VecDeque<Instant>,
}
impl RateLimit {
    pub fn accept(&mut self, now: Instant, second: usize, minute: usize) -> bool {
        while self
            .events
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60))
        {
            self.events.pop_front();
        }
        if self.events.len() >= minute
            || self
                .events
                .iter()
                .rev()
                .take_while(|t| now.saturating_duration_since(**t) < Duration::from_secs(1))
                .count()
                >= second
        {
            return false;
        }
        self.events.push_back(now);
        true
    }
}
