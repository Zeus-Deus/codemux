//! Bounded directory-size walks.
//!
//! Three callers need "how many bytes does this tree occupy":
//!
//! - the desktop's `workspaces_worktree_sizes` command (per-worktree
//!   sizes for the sweep dialog),
//! - `codemux-remote workspace list` (the sum of every registered
//!   workspace on a host, reported back to the desktop's inventory
//!   poller), and
//! - Settings → Browser data (`get_browser_data_size`).
//!
//! All walk in Rust rather than shelling out to `du`: `du` differs
//! between GNU and BSD (flags, block units), isn't guaranteed present on
//! a minimal host, and we want a hard time budget the caller controls.
//!
//! Symlinks are never followed. A worktree can legitimately point at
//! directories outside itself (`node_modules` symlinked into a shared
//! cache, `.git` files pointing at the parent repo's object store), and
//! following those would both inflate the number and risk a walk that
//! never ends. Hard links are counted once per path, which slightly
//! over-reports shared object stores; that is acceptable for a
//! "disk used" hint.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// How many directory entries to classify between clock checks.
/// `Instant::now()` is cheap but not free on a walk that can touch a
/// million files; checking per entry would cost more than the stat.
const DEADLINE_CHECK_EVERY: u32 = 512;

/// Sum the sizes of every regular file below `root`, without following
/// symlinks. Returns `None` when `deadline` passes mid-walk so callers
/// report "unknown" instead of a misleading partial number.
///
/// A missing, unreadable or non-directory `root` yields `Some(0)`: the
/// caller already decided the path is a directory worth sizing, and an
/// unreadable subtree is a permissions detail rather than a reason to
/// surface an error.
pub fn dir_size_bounded(root: &Path, deadline: Option<Instant>) -> Option<u64> {
    // The root itself may be a symlink (a worktree registered through a
    // symlinked path). `is_dir` resolves that one level so we still size
    // the real directory, but nothing below it is ever followed.
    if !root.is_dir() {
        return Some(0);
    }

    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    let mut entries_seen = 0u32;
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            // Counted per entry, not per directory: a single flat
            // directory with a million files must hit the deadline too.
            entries_seen = entries_seen.wrapping_add(1);
            if entries_seen % DEADLINE_CHECK_EVERY == 0 {
                if let Some(deadline) = deadline {
                    if Instant::now() >= deadline {
                        return None;
                    }
                }
            }
            // `DirEntry::file_type` comes straight from the directory
            // listing on most filesystems (no extra stat) and reports the
            // link itself, so a symlink never registers as a directory to
            // descend into.
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                // `DirEntry::metadata` stats relative to the open
                // directory (fstatat) rather than re-resolving the full
                // path, and does not follow symlinks.
                total = total.saturating_add(meta.len());
            }
        }
    }
    Some(total)
}

/// Drop every path that lives inside another path in the list (and exact
/// duplicates), so a project root and one of its nested worktrees are
/// not counted twice. Paths are canonicalised for the comparison so
/// `/srv/app/` and `/srv/app` collapse; paths that no longer exist are
/// kept as-is (they size to zero anyway).
pub fn dedupe_nested_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut canonical: Vec<PathBuf> = paths
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    canonical.sort();
    canonical.dedup();

    let mut kept: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for path in canonical {
        // Sorted order guarantees any ancestor was visited first, so one
        // check against the kept set's ancestors is sufficient.
        let nested = path.ancestors().skip(1).any(|a| seen.contains(a));
        if nested {
            continue;
        }
        seen.insert(path.clone());
        kept.push(path);
    }
    kept
}

/// Total bytes across `paths` (after [`dedupe_nested_paths`]) within a
/// single shared `deadline`. `None` as soon as the budget is exhausted.
pub fn total_size_bounded(paths: Vec<PathBuf>, deadline: Option<Instant>) -> Option<u64> {
    let mut total = 0u64;
    for path in dedupe_nested_paths(paths) {
        total = total.saturating_add(dir_size_bounded(&path, deadline)?);
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn sums_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), 10);
        write(&dir.path().join("sub/b.txt"), 20);
        write(&dir.path().join("sub/deeper/c.txt"), 30);
        assert_eq!(dir_size_bounded(dir.path(), None), Some(60));
    }

    #[test]
    fn sums_flat_files() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a"), 100);
        write(&dir.path().join("b"), 200);
        assert_eq!(dir_size_bounded(dir.path(), None), Some(300));
    }

    #[test]
    fn empty_dir_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(dir_size_bounded(dir.path(), None), Some(0));
    }

    #[test]
    fn missing_path_is_zero_not_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(dir_size_bounded(&dir.path().join("nope"), None), Some(0));
        assert_eq!(
            dir_size_bounded(Path::new("/this/path/should/definitely/not/exist/xyz123"), None),
            Some(0)
        );
    }

    #[test]
    fn regular_file_root_is_zero() {
        // Callers ask for the size of a directory; pointing at a file is
        // a caller mistake that must not crash or report the file's size
        // as if it were a tree.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("only.txt");
        write(&f, 42);
        assert_eq!(dir_size_bounded(&f, None), Some(0));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("big.bin"), 5000);
        write(&dir.path().join("real.txt"), 7);
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link-dir")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("big.bin"),
            dir.path().join("link-file"),
        )
        .unwrap();
        assert_eq!(dir_size_bounded(dir.path(), None), Some(7));
    }

    #[test]
    fn expired_deadline_reports_unknown_on_nested_tree() {
        let dir = tempfile::tempdir().unwrap();
        // Enough entries that the periodic clock check fires.
        for i in 0..(DEADLINE_CHECK_EVERY + 100) {
            write(&dir.path().join(format!("d{i}/f")), 1);
        }
        let past = Instant::now() - Duration::from_secs(1);
        assert_eq!(dir_size_bounded(dir.path(), Some(past)), None);
    }

    #[test]
    fn expired_deadline_reports_unknown_on_flat_directory() {
        // One directory, many files: the deadline must be checked per
        // entry, not per directory, or a flat million-file dir would
        // never notice the clock.
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(DEADLINE_CHECK_EVERY + 100) {
            write(&dir.path().join(format!("f{i}")), 1);
        }
        let past = Instant::now() - Duration::from_secs(1);
        assert_eq!(dir_size_bounded(dir.path(), Some(past)), None);
    }

    #[test]
    fn generous_deadline_still_returns_a_size() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(DEADLINE_CHECK_EVERY + 100) {
            write(&dir.path().join(format!("f{i}")), 1);
        }
        let later = Instant::now() + Duration::from_secs(60);
        assert_eq!(
            dir_size_bounded(dir.path(), Some(later)),
            Some(u64::from(DEADLINE_CHECK_EVERY + 100))
        );
    }

    // The walk is plain blocking code; commands run it through
    // `spawn_blocking` so a large tree never stalls the async runtime.
    // Guards against the helper accidentally capturing something !Send.
    #[tokio::test]
    async fn walk_runs_on_the_blocking_pool() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("payload"), 1024);
        let path = dir.path().to_path_buf();
        let size = tokio::task::spawn_blocking(move || dir_size_bounded(&path, None))
            .await
            .expect("blocking task");
        assert_eq!(size, Some(1024));
    }

    #[test]
    fn nested_paths_are_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("root.txt"), 100);
        write(&dir.path().join("wt/inner.txt"), 50);
        let paths = vec![
            dir.path().join("wt"),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        ];
        assert_eq!(dedupe_nested_paths(paths.clone()).len(), 1);
        assert_eq!(total_size_bounded(paths, None), Some(150));
    }

    #[test]
    fn sibling_paths_are_both_kept() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/f"), 1);
        write(&dir.path().join("b/f"), 2);
        let kept = dedupe_nested_paths(vec![dir.path().join("a"), dir.path().join("b")]);
        assert_eq!(kept.len(), 2);
        assert_eq!(
            total_size_bounded(vec![dir.path().join("a"), dir.path().join("b")], None),
            Some(3)
        );
    }
}
