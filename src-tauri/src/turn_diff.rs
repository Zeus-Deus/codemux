//! Per-turn git diffs between two worktree snapshots.
//!
//! An agent turn is bracketed by two snapshots taken with
//! [`crate::git::git_checkpoint_create`]: one right before the turn is
//! dispatched and one when it settles. Both are full worktree trees
//! (tracked changes plus untracked, non-ignored files), so diffing them
//! yields exactly what the turn changed, new files included, without
//! touching the user's index or stash. Snapshots are anchored under
//! [`TURN_DIFF_REF_PREFIX`] so `git gc` keeps them.
//!
//! Paths are relative to the repository root. Patches are capped per file
//! and per turn so a generated lockfile or a vendored blob cannot balloon
//! the IPC payload; the stats are always complete.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// Ref namespace for per-turn diff snapshots:
/// `<prefix>/<thread>/<token>/{before,after}`.
pub const TURN_DIFF_REF_PREFIX: &str = "refs/codemux/turn-diffs";

/// Blobs above this size are diffed as binary (stats `-`, no patch), so
/// git never line-diffs a huge generated file.
const BIG_FILE_THRESHOLD: &str = "core.bigFileThreshold=4m";
/// Files listed per turn. Totals still count every file.
const MAX_FILES: usize = 1000;
/// A file with more changed lines than this gets stats but no patch.
const MAX_FILE_PATCH_LINES: u64 = 20_000;
/// A single file's patch above this many bytes is withheld.
const MAX_FILE_PATCH_BYTES: usize = 512 * 1024;
/// Patch bytes returned per turn; later files keep only their stats.
const MAX_TOTAL_PATCH_BYTES: usize = 4 * 1024 * 1024;
/// Changed lines requested from git per turn, so an oversized turn is cut
/// off before git renders it rather than after.
const MAX_TOTAL_PATCH_LINES: u64 = 100_000;
/// Pathspecs per patch invocation, well under any argv limit.
const PATCH_BATCH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnDiffChange {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

/// Why a file in a turn diff carries no patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnDiffPatchOmitted {
    Binary,
    /// The file alone exceeds the per-file patch limit.
    TooLarge,
    /// The turn's patch budget ran out before this file.
    Budget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDiffFile {
    /// Path after the turn, relative to the repository root.
    pub path: String,
    /// Path before the turn, for renames and copies.
    pub old_path: Option<String>,
    pub change: TurnDiffChange,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
    /// This file's unified diff, starting at its `diff --git` header.
    pub patch: Option<String>,
    pub patch_omitted: Option<TurnDiffPatchOmitted>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDiff {
    pub files: Vec<TurnDiffFile>,
    /// Every changed file, including any beyond the listed `files`.
    pub files_changed: usize,
    pub additions: u64,
    pub deletions: u64,
    /// Files were left out of the list or lost their patch to the budget.
    pub truncated: bool,
}

/// Ref anchoring one side of a turn's snapshot pair.
pub fn turn_diff_ref_name(thread_id: &str, token: &str, side: &str) -> String {
    format!(
        "{TURN_DIFF_REF_PREFIX}/{}/{token}/{side}",
        crate::git::sanitize_ref_component(thread_id)
    )
}

fn thread_ref_prefix(thread_id: &str) -> String {
    format!(
        "{TURN_DIFF_REF_PREFIX}/{}/",
        crate::git::sanitize_ref_component(thread_id)
    )
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git {} failed: {}",
            args.iter().find(|a| !a.starts_with('-') && !a.contains('=')).unwrap_or(&""),
            stderr.trim()
        ));
    }
    Ok(output.stdout)
}

/// Point `ref_name` at an existing snapshot commit, so one snapshot can end
/// one turn and start the next under refs each row owns.
pub fn point_ref(repo: &Path, ref_name: &str, commit: &str) -> Result<(), String> {
    git(repo, &["update-ref", ref_name, commit]).map(|_| ())
}

pub fn delete_ref(repo: &Path, ref_name: &str) -> Result<(), String> {
    git(repo, &["update-ref", "-d", ref_name]).map(|_| ())
}

/// Drop every turn-diff ref a thread left in `repo`, including any a crash
/// orphaned before its row was written.
pub fn delete_thread_refs(repo: &Path, thread_id: &str) -> Result<(), String> {
    let listed = git(
        repo,
        &["for-each-ref", "--format=%(refname)", &thread_ref_prefix(thread_id)],
    )?;
    let commands: String = String::from_utf8_lossy(&listed)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| format!("delete {line}\n"))
        .collect();
    if commands.is_empty() {
        return Ok(());
    }
    let mut child = Command::new("git")
        .args(["update-ref", "--stdin"])
        .current_dir(repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(commands.as_bytes())
            .map_err(|e| format!("Failed to write ref deletions: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git update-ref failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Options shared by every diff call, so the raw, numstat and patch passes
/// list files in the same order. They also pin down user config that would
/// change paths or output (`diff.relative`, `diff.noprefix`, external diff
/// drivers, textconv, color).
fn diff_args<'a>(extra: &[&'a str], from: &'a str, to: &'a str) -> Vec<&'a str> {
    let mut args = vec![
        "-c",
        "core.quotePath=false",
        "-c",
        BIG_FILE_THRESHOLD,
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-relative",
        "-M",
    ];
    args.extend_from_slice(extra);
    args.push(from);
    args.push(to);
    args
}

struct RawEntry {
    change: TurnDiffChange,
    old_path: Option<String>,
    path: String,
}

fn split_nul(out: &[u8]) -> Vec<String> {
    out.split(|b| *b == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect()
}

/// Parse `git diff --raw -z`: `:<modes> <shas> <status>\0<path>\0`, with a
/// second path for renames and copies.
fn parse_raw(out: &[u8]) -> Result<Vec<RawEntry>, String> {
    let fields = split_nul(out);
    let mut entries = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let header = &fields[index];
        if header.is_empty() {
            index += 1;
            continue;
        }
        let status = header
            .rsplit(' ')
            .next()
            .and_then(|s| s.chars().next())
            .ok_or_else(|| format!("Unexpected git diff record: {header}"))?;
        let two_paths = matches!(status, 'R' | 'C');
        let first = fields
            .get(index + 1)
            .cloned()
            .ok_or_else(|| "Truncated git diff record".to_string())?;
        let (old_path, path) = if two_paths {
            let second = fields
                .get(index + 2)
                .cloned()
                .ok_or_else(|| "Truncated git diff rename record".to_string())?;
            (Some(first), second)
        } else {
            (None, first)
        };
        let change = match status {
            'A' => TurnDiffChange::Added,
            'D' => TurnDiffChange::Deleted,
            'R' => TurnDiffChange::Renamed,
            'C' => TurnDiffChange::Copied,
            'T' => TurnDiffChange::TypeChanged,
            _ => TurnDiffChange::Modified,
        };
        entries.push(RawEntry {
            change,
            old_path,
            path,
        });
        index += if two_paths { 3 } else { 2 };
    }
    Ok(entries)
}

/// Parse `git diff --numstat -z` into `(additions, deletions)`, `None` for a
/// binary file. Renames carry an empty path field followed by both paths.
fn parse_numstat(out: &[u8]) -> Result<Vec<Option<(u64, u64)>>, String> {
    let fields = split_nul(out);
    let mut stats = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let record = &fields[index];
        if record.is_empty() {
            index += 1;
            continue;
        }
        let mut parts = record.splitn(3, '\t');
        let added = parts.next().unwrap_or("");
        let deleted = parts.next().unwrap_or("");
        let path = parts
            .next()
            .ok_or_else(|| format!("Unexpected git numstat record: {record}"))?;
        stats.push(match (added.parse(), deleted.parse()) {
            (Ok(a), Ok(d)) => Some((a, d)),
            _ => None,
        });
        index += if path.is_empty() { 3 } else { 1 };
    }
    Ok(stats)
}

/// Split a multi-file patch at its `diff --git` headers. Every other patch
/// line starts with a space, `+`, `-`, `@`, `\` or a header keyword, so the
/// split is unambiguous.
fn split_patch(text: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") || chunks.is_empty() {
            chunks.push(String::new());
        }
        if let Some(chunk) = chunks.last_mut() {
            chunk.push_str(line);
        }
    }
    chunks
}

fn literal_pathspec(path: &str) -> String {
    format!(":(top,literal){path}")
}

fn pathspecs(file: &TurnDiffFile) -> Vec<String> {
    let mut specs = vec![literal_pathspec(&file.path)];
    if let Some(old) = &file.old_path {
        specs.push(literal_pathspec(old));
    }
    specs
}

fn patch_for(repo: &Path, from: &str, to: &str, specs: &[String]) -> Result<String, String> {
    let mut args = diff_args(&["--src-prefix=a/", "--dst-prefix=b/"], from, to);
    args.push("--");
    args.extend(specs.iter().map(String::as_str));
    git(repo, &args).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// Patches for `files`, in order. One git call per batch; if git's output
/// does not split into exactly one chunk per file, fetch that batch's files
/// one by one instead of guessing which chunk is whose.
fn fetch_patches(
    repo: &Path,
    from: &str,
    to: &str,
    files: &[&TurnDiffFile],
) -> Result<Vec<String>, String> {
    let mut patches = Vec::with_capacity(files.len());
    for batch in files.chunks(PATCH_BATCH) {
        let specs: Vec<String> = batch.iter().flat_map(|file| pathspecs(file)).collect();
        let chunks = split_patch(&patch_for(repo, from, to, &specs)?);
        if chunks.len() == batch.len() {
            patches.extend(chunks);
            continue;
        }
        for file in batch {
            patches.push(patch_for(repo, from, to, &pathspecs(file))?);
        }
    }
    Ok(patches)
}

/// Diff two snapshot commits taken in `repo`.
pub fn diff_snapshots(repo: &Path, from: &str, to: &str) -> Result<TurnDiff, String> {
    let raw = parse_raw(&git(repo, &diff_args(&["--raw", "-z"], from, to))?)?;
    let numstat = parse_numstat(&git(repo, &diff_args(&["--numstat", "-z"], from, to))?)?;
    if raw.len() != numstat.len() {
        return Err(format!(
            "git listed {} changed files but {} line counts",
            raw.len(),
            numstat.len()
        ));
    }

    let mut diff = TurnDiff {
        files_changed: raw.len(),
        ..TurnDiff::default()
    };
    for (entry, stats) in raw.into_iter().zip(numstat) {
        let (additions, deletions) = stats.unwrap_or((0, 0));
        diff.additions += additions;
        diff.deletions += deletions;
        if diff.files.len() < MAX_FILES {
            diff.files.push(TurnDiffFile {
                path: entry.path,
                old_path: entry.old_path,
                change: entry.change,
                additions,
                deletions,
                binary: stats.is_none(),
                patch: None,
                patch_omitted: stats.is_none().then_some(TurnDiffPatchOmitted::Binary),
            });
        }
    }
    diff.truncated = diff.files.len() < diff.files_changed;

    // Choose which files get a patch from their line counts, before asking
    // git to render anything.
    let mut lines_budget = MAX_TOTAL_PATCH_LINES;
    let mut wanted = Vec::new();
    for (index, file) in diff.files.iter_mut().enumerate() {
        if file.binary {
            continue;
        }
        let lines = file.additions + file.deletions;
        if lines > MAX_FILE_PATCH_LINES {
            file.patch_omitted = Some(TurnDiffPatchOmitted::TooLarge);
        } else if lines > lines_budget {
            file.patch_omitted = Some(TurnDiffPatchOmitted::Budget);
        } else {
            lines_budget -= lines;
            wanted.push(index);
        }
    }
    let selected: Vec<&TurnDiffFile> = wanted.iter().map(|&index| &diff.files[index]).collect();
    let patches = fetch_patches(repo, from, to, &selected)?;

    let mut bytes_budget = MAX_TOTAL_PATCH_BYTES;
    for (index, patch) in wanted.into_iter().zip(patches) {
        let file = &mut diff.files[index];
        if patch.len() > MAX_FILE_PATCH_BYTES {
            file.patch_omitted = Some(TurnDiffPatchOmitted::TooLarge);
        } else if patch.len() > bytes_budget {
            file.patch_omitted = Some(TurnDiffPatchOmitted::Budget);
        } else {
            bytes_budget -= patch.len();
            file.patch = Some(patch);
        }
    }
    diff.truncated |= diff
        .files
        .iter()
        .any(|file| file.patch_omitted == Some(TurnDiffPatchOmitted::Budget));
    Ok(diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn run(repo: &Path, args: &[&str]) -> String {
        String::from_utf8(git(repo, args).unwrap()).unwrap().trim().to_string()
    }

    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["config", "user.name", "Test"]);
        run(&repo, &["config", "user.email", "test@example.com"]);
        run(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("keep.txt"), "one\ntwo\nthree\n").unwrap();
        fs::write(repo.join("gone.txt"), "bye\n").unwrap();
        fs::write(
            repo.join("move-me.txt"),
            (0..40).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        fs::write(repo.join(".gitignore"), "ignored/\n").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "init"]);
        (dir, repo)
    }

    fn snapshot(repo: &Path, side: &str) -> String {
        crate::git::git_checkpoint_create(repo, &turn_diff_ref_name("thread", "t1", side), "test")
            .unwrap()
            .unwrap()
            .snapshot_commit
    }

    fn file<'a>(diff: &'a TurnDiff, path: &str) -> &'a TurnDiffFile {
        diff.files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} missing from {:?}", diff.files))
    }

    #[test]
    fn turn_diff_reports_each_kind_of_change_with_patches() {
        let (_dir, repo) = init_repo();
        // Pre-existing dirty state belongs to the user, not the turn.
        fs::write(repo.join("keep.txt"), "one\ntwo\nthree\nuser edit\n").unwrap();
        let before = snapshot(&repo, "before");

        fs::write(repo.join("keep.txt"), "one\nTWO\nthree\nuser edit\n").unwrap();
        fs::remove_file(repo.join("gone.txt")).unwrap();
        fs::rename(repo.join("move-me.txt"), repo.join("moved.txt")).unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/new file.rs"), "fn main() {}\n").unwrap();
        fs::write(repo.join("logo.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
        fs::create_dir_all(repo.join("ignored")).unwrap();
        fs::write(repo.join("ignored/cache.txt"), "noise\n").unwrap();
        let after = snapshot(&repo, "after");

        let diff = diff_snapshots(&repo, &before, &after).unwrap();
        assert_eq!(diff.files_changed, 5, "{:?}", diff.files);
        assert!(!diff.truncated);
        assert!(diff.files.iter().all(|f| !f.path.starts_with("ignored/")));

        let keep = file(&diff, "keep.txt");
        assert_eq!(keep.change, TurnDiffChange::Modified);
        assert_eq!((keep.additions, keep.deletions), (1, 1));
        let patch = keep.patch.as_deref().unwrap();
        assert!(patch.starts_with("diff --git a/keep.txt b/keep.txt\n"), "{patch}");
        assert!(patch.contains("\n-two\n+TWO\n"), "{patch}");
        assert!(!patch.contains("+user edit"), "pre-turn edits are not the turn's");

        let gone = file(&diff, "gone.txt");
        assert_eq!(gone.change, TurnDiffChange::Deleted);
        assert_eq!((gone.additions, gone.deletions), (0, 1));

        let moved = file(&diff, "moved.txt");
        assert_eq!(moved.change, TurnDiffChange::Renamed);
        assert_eq!(moved.old_path.as_deref(), Some("move-me.txt"));
        assert!(moved.patch.as_deref().unwrap().contains("rename from move-me.txt"));

        let added = file(&diff, "src/new file.rs");
        assert_eq!(added.change, TurnDiffChange::Added);
        assert_eq!((added.additions, added.deletions), (1, 0));
        assert!(added.patch.as_deref().unwrap().contains("+fn main() {}"));

        let binary = file(&diff, "logo.bin");
        assert!(binary.binary);
        assert_eq!(binary.patch, None);
        assert_eq!(binary.patch_omitted, Some(TurnDiffPatchOmitted::Binary));

        assert_eq!(diff.additions, 1 + 1);
        assert_eq!(diff.deletions, 1 + 1);
    }

    #[test]
    fn turn_diff_withholds_oversized_patches_but_keeps_stats() {
        let (_dir, repo) = init_repo();
        let before = snapshot(&repo, "before");
        let lines = (MAX_FILE_PATCH_LINES + 1) as usize;
        fs::write(
            repo.join("huge.txt"),
            (0..lines).map(|i| format!("{i}\n")).collect::<String>(),
        )
        .unwrap();
        // Over core.bigFileThreshold: git treats it as binary outright.
        fs::write(repo.join("blob.txt"), "a\n".repeat(3 * 1024 * 1024)).unwrap();
        fs::write(repo.join("small.txt"), "ok\n").unwrap();
        let after = snapshot(&repo, "after");

        let diff = diff_snapshots(&repo, &before, &after).unwrap();
        let huge = file(&diff, "huge.txt");
        assert_eq!(huge.additions, lines as u64);
        assert_eq!(huge.patch, None);
        assert_eq!(huge.patch_omitted, Some(TurnDiffPatchOmitted::TooLarge));
        let blob = file(&diff, "blob.txt");
        assert!(blob.binary);
        assert_eq!(blob.patch_omitted, Some(TurnDiffPatchOmitted::Binary));
        assert!(file(&diff, "small.txt").patch.is_some());
        assert!(!diff.truncated);
    }

    #[test]
    fn turn_diff_of_identical_snapshots_is_empty() {
        let (_dir, repo) = init_repo();
        let before = snapshot(&repo, "before");
        let after = snapshot(&repo, "after");
        assert_eq!(diff_snapshots(&repo, &before, &after).unwrap(), TurnDiff::default());
    }

    #[test]
    fn turn_diff_ignores_relative_and_noprefix_config() {
        let (_dir, repo) = init_repo();
        run(&repo, &["config", "diff.relative", "true"]);
        run(&repo, &["config", "diff.noprefix", "true"]);
        fs::create_dir_all(repo.join("sub")).unwrap();
        let before = snapshot(&repo, "before");
        fs::write(repo.join("keep.txt"), "changed\n").unwrap();
        let after = snapshot(&repo, "after");
        let diff = diff_snapshots(&repo.join("sub"), &before, &after).unwrap();
        let keep = file(&diff, "keep.txt");
        assert!(keep.patch.as_deref().unwrap().starts_with("diff --git a/keep.txt b/keep.txt"));
    }

    #[test]
    fn delete_thread_refs_removes_only_that_threads_refs() {
        let (_dir, repo) = init_repo();
        let head = run(&repo, &["rev-parse", "HEAD"]);
        let mine = turn_diff_ref_name("thread-a", "1", "before");
        let other = turn_diff_ref_name("thread-b", "1", "before");
        point_ref(&repo, &mine, &head).unwrap();
        point_ref(&repo, &other, &head).unwrap();
        delete_thread_refs(&repo, "thread-a").unwrap();
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", &mine]).is_err());
        assert_eq!(run(&repo, &["rev-parse", &other]), head);
        // Nothing left to delete is still a success.
        delete_thread_refs(&repo, "thread-a").unwrap();
    }
}
