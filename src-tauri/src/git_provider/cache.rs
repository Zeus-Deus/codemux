//! TTL cache primitive shared by non-GitHub provider adapters.
//!
//! `crate::github_cache` is the same idea specialised to GitHub: one
//! static per query shape, keyed by repository path, holding a value
//! for a fixed TTL. Rather than widen those statics — which would mean
//! touching the GitHub read path — this generalises the *primitive* and
//! lets each adapter own its own statics. The TTLs are re-exported from
//! `github_cache` so the two families cannot drift apart: a user
//! switching a workspace from one product to the other should not see a
//! different staleness window.
//!
//! Keys are strings built by the caller and must include the repository
//! path (issue and merge-request numbers are per-project, so a bare
//! number would serve project A's #1 for project B's #1) plus whatever
//! else varies the query.
//!
//! Best-effort, exactly like `github_cache`: a poisoned mutex falls
//! through to a fresh fetch instead of propagating the panic, and
//! errors are never cached.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use crate::github_cache::{DETAIL_TTL, LIST_TTL};

struct CacheEntry<V> {
    value: V,
    fetched_at: Instant,
}

pub struct TtlCache<V> {
    entries: Mutex<HashMap<String, CacheEntry<V>>>,
}

impl<V: Clone> Default for TtlCache<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Clone> TtlCache<V> {
    /// Free function form (rather than `Default::default`) so a static
    /// can be declared as `LazyLock::new(TtlCache::new)`.
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Cached value when one is younger than `ttl`, otherwise the result
    /// of `fetch` (stored on success only).
    pub fn get_or_fetch<E>(
        &self,
        key: &str,
        ttl: Duration,
        fetch: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E> {
        if let Ok(mut entries) = self.entries.lock() {
            if let Some(entry) = entries.get(key) {
                if entry.fetched_at.elapsed() < ttl {
                    return Ok(entry.value.clone());
                }
                // Drop it now rather than leaving it to be overwritten:
                // the fetch below may fail, and a stale value must not
                // sit in the map holding its payload alive until the
                // next *successful* call for that key.
                entries.remove(key);
            }
        }

        let fresh = fetch()?;
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                key.to_string(),
                CacheEntry {
                    value: fresh.clone(),
                    fetched_at: Instant::now(),
                },
            );
        }
        Ok(fresh)
    }

    /// Drop every entry whose key starts with `prefix`. Keys are built
    /// path-first, so this is how a single repository's entries are
    /// dropped without disturbing another's.
    pub fn invalidate_prefix(&self, prefix: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|key, _| !key.starts_with(prefix));
        }
    }

    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_fresh_entry_is_served_without_refetching() {
        let cache: TtlCache<u32> = TtlCache::new();
        let calls = AtomicUsize::new(0);
        let mut fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<u32, String>(7)
        };

        assert_eq!(cache.get_or_fetch("k", LIST_TTL, &mut fetch), Ok(7));
        assert_eq!(cache.get_or_fetch("k", LIST_TTL, &mut fetch), Ok(7));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_zero_ttl_always_refetches() {
        let cache: TtlCache<u32> = TtlCache::new();
        let calls = AtomicUsize::new(0);
        let mut fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<u32, String>(1)
        };

        let _ = cache.get_or_fetch("k", Duration::from_nanos(0), &mut fetch);
        let _ = cache.get_or_fetch("k", Duration::from_nanos(0), &mut fetch);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn errors_are_never_cached() {
        let cache: TtlCache<u32> = TtlCache::new();
        assert!(cache
            .get_or_fetch("k", LIST_TTL, || Err::<u32, String>("boom".into()))
            .is_err());
        // The failed lookup left nothing behind, so the next call runs.
        assert_eq!(cache.get_or_fetch("k", LIST_TTL, || Ok::<u32, String>(3)), Ok(3));
    }

    /// An expired entry is dropped when it is read, not left to be
    /// overwritten by a later success that may never come — a repository
    /// the user removed would otherwise hold its payload forever.
    #[test]
    fn an_expired_entry_is_evicted_even_when_the_refetch_fails() {
        let cache: TtlCache<u32> = TtlCache::new();
        let _ = cache.get_or_fetch("k", Duration::from_secs(60), || Ok::<u32, String>(1));
        assert_eq!(cache.len(), 1);

        assert!(cache
            .get_or_fetch("k", Duration::from_nanos(0), || Err::<u32, String>("boom".into()))
            .is_err());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn prefix_invalidation_is_scoped_to_one_repository() {
        let cache: TtlCache<u32> = TtlCache::new();
        let _ = cache.get_or_fetch("/repo-a|1", LIST_TTL, || Ok::<u32, String>(1));
        let _ = cache.get_or_fetch("/repo-b|1", LIST_TTL, || Ok::<u32, String>(2));

        cache.invalidate_prefix("/repo-a|");

        // Repo A refetches; repo B still serves its cached value.
        assert_eq!(
            cache.get_or_fetch("/repo-a|1", LIST_TTL, || Ok::<u32, String>(99)),
            Ok(99)
        );
        assert_eq!(
            cache.get_or_fetch("/repo-b|1", LIST_TTL, || Ok::<u32, String>(99)),
            Ok(2)
        );
    }
}
