//! Per-turn git diffs between two worktree snapshots.
//!
//! An agent turn is bracketed by two [`snapshot`]s: one right before the
//! turn is dispatched and one when it settles. Both are full worktree trees
//! (tracked changes plus untracked, non-ignored files), so diffing them
//! yields exactly what the turn changed, new files included, without
//! touching the user's index or stash. Snapshots are anchored under
//! [`TURN_DIFF_REF_PREFIX`] so `git gc` keeps them.
//!
//! Paths are relative to the repository root. Patches are capped per file
//! and per turn while git's output is read, so a generated lockfile or a
//! vendored blob can balloon neither memory nor the IPC payload; the stats
//! are always complete.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Ref namespace for per-turn diff snapshots:
/// `<prefix>/<thread>/<token>/{before,after}`.
pub const TURN_DIFF_REF_PREFIX: &str = "refs/codemux/turn-diffs";

/// Blobs above this size are diffed as binary (stats `-`, no patch), so
/// git never line-diffs a huge generated file. A `diff` attribute can
/// override that, so [`MAX_PATCH_BLOB_BYTES`] is also checked directly.
const BIG_FILE_THRESHOLD: &str = "core.bigFileThreshold=4m";
/// No patch is rendered for a file whose old or new blob is larger.
const MAX_PATCH_BLOB_BYTES: u64 = 4 * 1024 * 1024;
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

/// Run git in `repo`, optionally against a private index and with `stdin`.
fn git_with(
    repo: &Path,
    args: &[&str],
    index: Option<&Path>,
    stdin: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(repo)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    // Feed stdin from another thread while stdout drains here: git can
    // answer before it has read everything (`cat-file --batch-check`), and
    // with both pipes full each side would wait on the other.
    let output = std::thread::scope(|scope| {
        if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
            scope.spawn(move || {
                let _ = pipe.write_all(input);
            });
        }
        child.wait_with_output()
    })
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

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    git_with(repo, args, None, None)
}

fn git_line(repo: &Path, args: &[&str]) -> Result<String, String> {
    git(repo, args).map(|out| String::from_utf8_lossy(&out).trim_end().to_string())
}

fn git_path(repo: &Path, path: &str) -> Result<PathBuf, String> {
    git_line(repo, &["rev-parse", "--path-format=absolute", "--git-path", path]).map(PathBuf::from)
}

/// The private index a thread's snapshots are staged through. It persists
/// between snapshots, so git's stat cache spares unchanged files (untracked
/// ones included) from being read and hashed again on every turn.
fn snapshot_index(repo: &Path, thread_id: &str) -> Result<PathBuf, String> {
    git_path(
        repo,
        &format!(
            "codemux/turn-diff-index/{}",
            crate::git::sanitize_ref_component(thread_id)
        ),
    )
}

/// Start a snapshot index from the real one, so the first snapshot only
/// hashes what differs from it. The copy keeps the real index's mtime: git
/// trusts a cached stat only when the entry is older than its index, and a
/// fresh mtime would wrongly trust entries written in the same second.
fn seed_snapshot_index(repo: &Path, index: &Path) -> Result<(), String> {
    if let Some(parent) = index.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create snapshot index dir: {e}"))?;
    }
    let real = git_path(repo, "index")?;
    let Ok(mtime) = std::fs::metadata(&real).and_then(|m| m.modified()) else {
        // No real index (fresh repository): start empty.
        return Ok(());
    };
    let copied = std::fs::copy(&real, index).is_ok()
        && std::fs::OpenOptions::new()
            .write(true)
            .open(index)
            .and_then(|file| file.set_modified(mtime))
            .is_ok();
    if !copied {
        // Without a trustworthy stat cache, hash everything once instead.
        let _ = std::fs::remove_file(index);
        return Ok(());
    }
    // The user's assume-unchanged marks would make `add -A` skip those
    // files for good in this private index; capture them like any other.
    let listed = git_with(repo, &["ls-files", "-z", "-v"], Some(index), None)?;
    let assumed: Vec<&[u8]> = listed
        .split(|b| *b == 0)
        .filter(|entry| entry.len() > 2 && entry[0].is_ascii_lowercase() && entry[1] == b' ')
        .map(|entry| &entry[2..])
        .collect();
    if !assumed.is_empty() {
        let mut paths = assumed.join(&0u8);
        paths.push(0);
        git_with(
            repo,
            &["update-index", "-z", "--no-assume-unchanged", "--stdin"],
            Some(index),
            Some(&paths),
        )?;
    }
    Ok(())
}

fn nul_pathspecs<'a>(paths: impl Iterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut specs = Vec::new();
    for path in paths {
        specs.extend_from_slice(b":(top,literal)");
        specs.extend_from_slice(path);
        specs.push(0);
    }
    specs
}

/// `git add -A` follows `.gitignore` only for files not yet in the index,
/// and the private index keeps whatever it once staged. Make its ignored
/// entries match the real index's: drop files that became ignored after
/// they were first captured, and pick up ignored files the user force-added.
fn sync_ignored(repo: &Path, index: &Path) -> Result<(), String> {
    let ignored_args = ["ls-files", "-z", "--cached", "--ignored", "--exclude-standard"];
    let staged = git_with(repo, &ignored_args, Some(index), None)?;
    let tracked = git(repo, &ignored_args)?;
    let staged: HashSet<&[u8]> = staged.split(|b| *b == 0).filter(|p| !p.is_empty()).collect();
    let tracked: HashSet<&[u8]> = tracked.split(|b| *b == 0).filter(|p| !p.is_empty()).collect();

    let stale = nul_pathspecs(staged.difference(&tracked).copied());
    if !stale.is_empty() {
        let args = [
            "rm",
            "--cached",
            "-q",
            "-f",
            "--ignore-unmatch",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ];
        git_with(repo, &args, Some(index), Some(&stale))?;
    }
    let missing = nul_pathspecs(tracked.difference(&staged).copied().filter(|path| {
        std::fs::symlink_metadata(repo.join(String::from_utf8_lossy(path).as_ref())).is_ok()
    }));
    if !missing.is_empty() {
        let args = [
            "-c",
            "core.ignoreStat=false",
            "add",
            "-f",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ];
        git_with(repo, &args, Some(index), Some(&missing))?;
    }
    Ok(())
}

/// Root of the git worktree containing `path`, or `None` when `path` is
/// gone or not inside one.
pub fn repo_root(path: &Path) -> Option<String> {
    if !path.is_dir() {
        return None;
    }
    git_line(path, &["rev-parse", "--show-toplevel"]).ok()
}

/// Snapshot the full worktree of the repository containing `repo_path`
/// (tracked and untracked files, `.gitignore` respected) as a parentless
/// commit anchored at `ref_name`. The user's index, worktree and stash are
/// never touched, no hooks run, and a repository without commits works too.
/// `Ok(None)` when `repo_path` is not inside a git worktree.
pub fn snapshot(
    repo_path: &Path,
    thread_id: &str,
    ref_name: &str,
    message: &str,
) -> Result<Option<String>, String> {
    let Ok(toplevel) = git_line(repo_path, &["rev-parse", "--show-toplevel"]) else {
        return Ok(None);
    };
    let repo = PathBuf::from(toplevel);
    let index = snapshot_index(&repo, thread_id)?;
    if !index.exists() {
        seed_snapshot_index(&repo, &index)?;
    }
    // `core.ignoreStat` would mark what `add` stages as assume-unchanged.
    let add = ["-c", "core.ignoreStat=false", "add", "-A"];
    let staged = git_with(&repo, &add, Some(&index), None)
        .and_then(|_| sync_ignored(&repo, &index))
        .and_then(|_| git_with(&repo, &["write-tree"], Some(&index), None));
    let tree = match staged {
        Ok(tree) => String::from_utf8_lossy(&tree).trim().to_string(),
        Err(error) => {
            // It is only a cache: rebuild it from scratch next time.
            let _ = std::fs::remove_file(&index);
            return Err(error);
        }
    };
    let commit = git_line(
        &repo,
        &[
            "-c",
            "user.name=Codemux",
            "-c",
            "user.email=checkpoint@codemux.invalid",
            "commit-tree",
            &tree,
            "-m",
            message,
        ],
    )?;
    git(&repo, &["update-ref", ref_name, &commit])?;
    Ok(Some(commit))
}

/// Point `ref_name` at an existing snapshot commit, so one snapshot can end
/// one turn and start the next under refs each row owns.
pub fn point_ref(repo: &Path, ref_name: &str, commit: &str) -> Result<(), String> {
    git(repo, &["update-ref", ref_name, commit]).map(|_| ())
}

pub fn delete_ref(repo: &Path, ref_name: &str) -> Result<(), String> {
    git(repo, &["update-ref", "-d", ref_name]).map(|_| ())
}

/// Delete many refs in one git call. Missing refs are not an error.
pub fn delete_refs<'a>(repo: &Path, refs: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let commands: String = refs
        .into_iter()
        .filter(|name| !name.is_empty())
        .map(|name| format!("delete {name}\n"))
        .collect();
    if commands.is_empty() {
        return Ok(());
    }
    git_with(repo, &["update-ref", "--stdin"], None, Some(commands.as_bytes())).map(|_| ())
}

/// Drop a thread's snapshot index, a cache that is rebuilt on demand.
pub fn forget_snapshot_index(repo: &Path, thread_id: &str) {
    if let Ok(index) = snapshot_index(repo, thread_id) {
        let _ = std::fs::remove_file(index);
    }
}

/// Drop every turn-diff ref named after a thread in `repo`, including any a
/// crash orphaned before its row was written, and its snapshot index.
pub fn delete_thread_refs(repo: &Path, thread_id: &str) -> Result<(), String> {
    forget_snapshot_index(repo, thread_id);
    let listed = git(
        repo,
        &["for-each-ref", "--format=%(refname)", &thread_ref_prefix(thread_id)],
    )?;
    delete_refs(repo, String::from_utf8_lossy(&listed).lines())
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
    /// Old and new blob ids; gitlinks and absent sides are left out.
    blobs: Vec<String>,
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
        // `:<old mode> <new mode> <old id> <new id> <status>`
        let parts: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
        let status = parts
            .get(4)
            .and_then(|s| s.chars().next())
            .ok_or_else(|| format!("Unexpected git diff record: {header}"))?;
        let blobs = [(0, 2), (1, 3)]
            .iter()
            .filter_map(|&(mode, id)| Some((*parts.get(mode)?, *parts.get(id)?)))
            .filter(|(mode, id)| *mode != "160000" && id.bytes().any(|b| b != b'0'))
            .map(|(_, id)| id.to_string())
            .collect();
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
            blobs,
        });
        index += if two_paths { 3 } else { 2 };
    }
    Ok(entries)
}

/// Parse `git diff --numstat -z` into `(additions, deletions)` by the
/// file's new path, `None` for a binary file. Renames carry an empty path
/// field followed by both paths.
fn parse_numstat(out: &[u8]) -> Result<HashMap<String, Option<(u64, u64)>>, String> {
    let fields = split_nul(out);
    let mut stats = HashMap::new();
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
        let counts = match (added.parse(), deleted.parse()) {
            (Ok(a), Ok(d)) => Some((a, d)),
            _ => None,
        };
        let (path, step) = if path.is_empty() {
            (fields.get(index + 2).cloned().unwrap_or_default(), 3)
        } else {
            (path.to_string(), 1)
        };
        stats.insert(path, counts);
        index += step;
    }
    Ok(stats)
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

/// One file's share of a patch run.
#[derive(Debug, PartialEq, Eq)]
enum PatchChunk {
    Patch(String),
    TooLarge,
    Budget,
}

/// One file's section of a patch run, with the `diff --git` line that
/// opened it so it can be matched to the file it belongs to.
struct PatchSection {
    header: Vec<u8>,
    chunk: PatchChunk,
}

/// Turn one file's collected bytes into its chunk, charging the turn budget.
/// Once a patch does not fit, the budget is spent: later files get none.
fn finish_chunk(bytes: Option<Vec<u8>>, budget: &mut usize) -> PatchChunk {
    match bytes {
        None => PatchChunk::TooLarge,
        Some(bytes) if bytes.len() > *budget => {
            *budget = 0;
            PatchChunk::Budget
        }
        Some(bytes) => {
            *budget -= bytes.len();
            PatchChunk::Patch(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
}

/// Read one line, keeping at most `keep` bytes of it. Returns the line's
/// full length; 0 at the end of output.
fn read_line_capped(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    keep: usize,
) -> std::io::Result<usize> {
    line.clear();
    let mut total = 0;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(total);
        }
        let (take, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(end) => (end + 1, true),
            None => (buf.len(), false),
        };
        let room = keep.saturating_sub(line.len());
        line.extend_from_slice(&buf[..take.min(room)]);
        reader.consume(take);
        total += take;
        if done {
            return Ok(total);
        }
    }
}

/// Run one patch call for `specs` and split it per file while reading, at
/// the `diff --git` headers (every other patch line starts with a space,
/// `+`, `-`, `@`, `\` or a header keyword). Lines are read in bounded
/// pieces, a file's bytes are dropped as soon as they pass the per-file
/// cap, and git is stopped once the turn budget is spent, so memory stays
/// bounded whatever the files hold. A type change (say, a symlink replaced
/// by a file) prints two sections under the same header; they are kept as
/// one. The second value is whether reading stopped early.
fn stream_patches(
    repo: &Path,
    from: &str,
    to: &str,
    specs: &[String],
    budget: &mut usize,
) -> Result<(Vec<PatchSection>, bool), String> {
    let mut args = diff_args(&["--src-prefix=a/", "--dst-prefix=b/"], from, to);
    args.push("--");
    args.extend(specs.iter().map(String::as_str));
    let mut child = Command::new("git")
        .args(&args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    let stdout = child.stdout.take().ok_or("git diff has no output pipe")?;
    let mut reader = BufReader::new(stdout);
    let mut sections = Vec::new();
    // The open section's header and bytes; `None` bytes once over the cap.
    let mut current: Option<(Vec<u8>, Option<Vec<u8>>)> = None;
    let mut line = Vec::new();
    let mut stopped = false;
    loop {
        let read = read_line_capped(&mut reader, &mut line, MAX_FILE_PATCH_BYTES + 1)
            .map_err(|e| format!("Failed to read git diff: {e}"))?;
        if read == 0 {
            break;
        }
        let opens = line.starts_with(b"diff --git ")
            && current.as_ref().is_none_or(|(header, _)| *header != line);
        if opens || current.is_none() {
            if let Some((header, bytes)) = current.take() {
                let chunk = finish_chunk(bytes, budget);
                sections.push(PatchSection { header, chunk });
            }
            if *budget == 0 {
                stopped = true;
                break;
            }
            current = Some((line.clone(), Some(Vec::new())));
        }
        if let Some((_, slot)) = current.as_mut() {
            match slot {
                Some(bytes) if read > line.len() || bytes.len() + read > MAX_FILE_PATCH_BYTES => {
                    *slot = None
                }
                Some(bytes) => bytes.extend_from_slice(&line),
                None => {}
            }
        }
    }
    if let Some((header, bytes)) = current.take() {
        let chunk = finish_chunk(bytes, budget);
        sections.push(PatchSection { header, chunk });
    }
    if stopped {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !stopped && !status.success() {
        return Err(format!("git diff failed: {status}"));
    }
    Ok((sections, stopped))
}

/// Whether `header` can be `file`'s `diff --git` line. Paths git would
/// quote are not checked.
fn header_matches(header: &[u8], file: &TurnDiffFile) -> bool {
    let needs_quoting = |path: &str| path.bytes().any(|b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\');
    let old = file.old_path.as_deref().unwrap_or(&file.path);
    if needs_quoting(old) || needs_quoting(&file.path) {
        return true;
    }
    let expected = format!("diff --git a/{old} b/{}", file.path);
    header.strip_suffix(b"\n").unwrap_or(header) == expected.as_bytes()
}

/// Patches for `files`, in order. One git call per batch; the sections must
/// match the files one to one (all of them, or a prefix when the budget
/// stopped the run). Otherwise that batch is fetched file by file instead of
/// guessing which section is whose. Files after the budget runs out get
/// [`PatchChunk::Budget`] without asking git.
fn fetch_patches(
    repo: &Path,
    from: &str,
    to: &str,
    files: &[&TurnDiffFile],
) -> Result<Vec<PatchChunk>, String> {
    let mut budget = MAX_TOTAL_PATCH_BYTES;
    let mut patches = Vec::with_capacity(files.len());
    for batch in files.chunks(PATCH_BATCH) {
        if budget == 0 {
            patches.extend(batch.iter().map(|_| PatchChunk::Budget));
            continue;
        }
        let specs: Vec<String> = batch.iter().flat_map(|file| pathspecs(file)).collect();
        let (sections, stopped) = stream_patches(repo, from, to, &specs, &mut budget)?;
        let matched = if stopped {
            sections.len() <= batch.len()
        } else {
            sections.len() == batch.len()
        } && sections
            .iter()
            .zip(batch)
            .all(|(section, file)| header_matches(&section.header, file));
        if matched {
            let read = sections.len();
            patches.extend(sections.into_iter().map(|section| section.chunk));
            patches.extend((read..batch.len()).map(|_| PatchChunk::Budget));
            continue;
        }
        // Undo the mismatched run's charges before fetching file by file.
        budget += sections
            .iter()
            .map(|section| match &section.chunk {
                PatchChunk::Patch(text) => text.len(),
                _ => 0,
            })
            .sum::<usize>();
        for file in batch {
            if budget == 0 {
                patches.push(PatchChunk::Budget);
                continue;
            }
            let (sections, _) = stream_patches(repo, from, to, &pathspecs(file), &mut budget)?;
            // A literal pathspec also matches paths beneath it (`foo` deleted,
            // `foo/x` added), so keep only this file's own sections.
            let mut chunks = sections
                .into_iter()
                .filter(|section| header_matches(&section.header, file))
                .map(|section| section.chunk);
            let first = chunks.next().unwrap_or(PatchChunk::Patch(String::new()));
            // More than one section for one file: keep them together.
            patches.push(chunks.fold(first, |joined, next| match (joined, next) {
                (PatchChunk::Patch(mut a), PatchChunk::Patch(b)) => {
                    a.push_str(&b);
                    PatchChunk::Patch(a)
                }
                (PatchChunk::Patch(_), other) | (other, _) => other,
            }));
        }
    }
    Ok(patches)
}

/// Size in bytes of each blob, by id. Missing objects are left out.
fn blob_sizes<'a>(
    repo: &Path,
    ids: impl Iterator<Item = &'a str>,
) -> Result<HashMap<String, u64>, String> {
    let input: String = ids.map(|id| format!("{id}\n")).collect();
    if input.is_empty() {
        return Ok(Default::default());
    }
    let out = git_with(
        repo,
        &["cat-file", "--batch-check=%(objectname) %(objectsize)"],
        None,
        Some(input.as_bytes()),
    )?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|line| {
            let (id, size) = line.split_once(' ')?;
            Some((id.to_string(), size.parse().ok()?))
        })
        .collect())
}

/// Diff two snapshot commits taken in `repo`.
pub fn diff_snapshots(repo: &Path, from: &str, to: &str) -> Result<TurnDiff, String> {
    let raw = parse_raw(&git(repo, &diff_args(&["--raw", "-z", "--no-abbrev"], from, to))?)?;

    // A `diff` attribute can force a text diff past core.bigFileThreshold,
    // so oversized blobs are found by size and kept out of every text pass,
    // line counts included: git would hold them in memory to diff them.
    let sizes = blob_sizes(repo, raw.iter().flat_map(|e| e.blobs.iter().map(String::as_str)))?;
    let oversized: Vec<bool> = raw
        .iter()
        .map(|entry| {
            entry
                .blobs
                .iter()
                .any(|id| sizes.get(id).is_some_and(|size| *size > MAX_PATCH_BLOB_BYTES))
        })
        .collect();
    let excludes: Vec<String> = raw
        .iter()
        .zip(&oversized)
        .filter(|(_, big)| **big)
        .flat_map(|(entry, _)| entry.old_path.iter().chain([&entry.path]))
        .map(|path| format!(":(exclude,top,literal){path}"))
        .collect();
    let mut numstat_args = diff_args(&["--numstat", "-z"], from, to);
    if !excludes.is_empty() {
        numstat_args.extend(["--", ":/"]);
        numstat_args.extend(excludes.iter().map(String::as_str));
    }
    let numstat = parse_numstat(&git(repo, &numstat_args)?)?;

    let mut diff = TurnDiff {
        files_changed: raw.len(),
        ..TurnDiff::default()
    };
    let mut too_big = Vec::new();
    for (entry, big) in raw.into_iter().zip(oversized) {
        // Oversized files have no line counts; anything git did not count
        // reads as zero rather than failing the whole turn.
        let stats = if big {
            Some((0, 0))
        } else {
            numstat.get(&entry.path).copied().unwrap_or(Some((0, 0)))
        };
        let (additions, deletions) = stats.unwrap_or((0, 0));
        diff.additions += additions;
        diff.deletions += deletions;
        if diff.files.len() < MAX_FILES {
            too_big.push(big);
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

    // Choose which files get a patch from their size and line counts,
    // before asking git to render anything.
    let mut lines_budget = MAX_TOTAL_PATCH_LINES;
    let mut wanted = Vec::new();
    for (index, (file, oversized)) in diff.files.iter_mut().zip(too_big).enumerate() {
        if file.binary {
            continue;
        }
        let lines = file.additions + file.deletions;
        if oversized || lines > MAX_FILE_PATCH_LINES {
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
    for (index, patch) in wanted.into_iter().zip(patches) {
        let file = &mut diff.files[index];
        match patch {
            PatchChunk::Patch(text) => file.patch = Some(text),
            PatchChunk::TooLarge => file.patch_omitted = Some(TurnDiffPatchOmitted::TooLarge),
            PatchChunk::Budget => file.patch_omitted = Some(TurnDiffPatchOmitted::Budget),
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

    fn snap(repo: &Path, side: &str) -> String {
        snapshot(repo, "thread", &turn_diff_ref_name("thread", "t1", side), "test")
            .unwrap()
            .unwrap()
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
        let before = snap(&repo, "before");

        fs::write(repo.join("keep.txt"), "one\nTWO\nthree\nuser edit\n").unwrap();
        fs::remove_file(repo.join("gone.txt")).unwrap();
        fs::rename(repo.join("move-me.txt"), repo.join("moved.txt")).unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/new file.rs"), "fn main() {}\n").unwrap();
        fs::write(repo.join("logo.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
        fs::create_dir_all(repo.join("ignored")).unwrap();
        fs::write(repo.join("ignored/cache.txt"), "noise\n").unwrap();
        let after = snap(&repo, "after");

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
        let before = snap(&repo, "before");
        let lines = (MAX_FILE_PATCH_LINES + 1) as usize;
        fs::write(
            repo.join("huge.txt"),
            (0..lines).map(|i| format!("{i}\n")).collect::<String>(),
        )
        .unwrap();
        // Over the 4 MiB blob cap.
        fs::write(repo.join("blob.txt"), "a\n".repeat(3 * 1024 * 1024)).unwrap();
        fs::write(repo.join("small.txt"), "ok\n").unwrap();
        let after = snap(&repo, "after");

        let diff = diff_snapshots(&repo, &before, &after).unwrap();
        let huge = file(&diff, "huge.txt");
        assert_eq!(huge.additions, lines as u64);
        assert_eq!(huge.patch, None);
        assert_eq!(huge.patch_omitted, Some(TurnDiffPatchOmitted::TooLarge));
        // Over the blob size cap: never line-counted or rendered.
        let blob = file(&diff, "blob.txt");
        assert_eq!((blob.additions, blob.deletions), (0, 0));
        assert_eq!(blob.patch_omitted, Some(TurnDiffPatchOmitted::TooLarge));
        assert!(file(&diff, "small.txt").patch.is_some());
        assert!(!diff.truncated);
    }

    #[test]
    fn turn_diff_streams_patches_within_byte_caps() {
        let (_dir, repo) = init_repo();
        let before = snap(&repo, "before");
        // One line, under the line and blob limits, over the per-file bytes.
        // Sorted first, so it is read before the budget runs out.
        fs::write(repo.join("a-wide.txt"), format!("{}\n", "x".repeat(MAX_FILE_PATCH_BYTES))).unwrap();
        // Enough mid-sized files to spend the turn budget.
        let body: String = (0..1000).map(|i| format!("{i:0>99}\n")).collect();
        for i in 0..50 {
            fs::write(repo.join(format!("mid-{i:02}.txt")), &body).unwrap();
        }
        let after = snap(&repo, "after");

        let diff = diff_snapshots(&repo, &before, &after).unwrap();
        let wide = file(&diff, "a-wide.txt");
        assert_eq!(wide.additions, 1);
        assert_eq!(wide.patch_omitted, Some(TurnDiffPatchOmitted::TooLarge));
        let kept: usize = diff.files.iter().filter_map(|f| f.patch.as_ref()).map(String::len).sum();
        assert!(kept <= MAX_TOTAL_PATCH_BYTES, "{kept}");
        let spent = diff
            .files
            .iter()
            .filter(|f| f.patch_omitted == Some(TurnDiffPatchOmitted::Budget))
            .count();
        assert!(spent > 0 && spent < 50, "{spent}");
        // Budget is spent in order: no patch after the first omitted file.
        let first_spent = diff
            .files
            .iter()
            .position(|f| f.patch_omitted == Some(TurnDiffPatchOmitted::Budget))
            .unwrap();
        assert!(diff.files[first_spent..].iter().all(|f| f.patch.is_none()));
        assert!(diff.truncated);
        assert_eq!(diff.files_changed, 51);
    }

    #[cfg(unix)]
    #[test]
    fn turn_diff_keeps_both_halves_of_a_type_change() {
        let (_dir, repo) = init_repo();
        std::os::unix::fs::symlink("keep.txt", repo.join("link")).unwrap();
        let before = snap(&repo, "before");
        fs::remove_file(repo.join("link")).unwrap();
        fs::write(repo.join("link"), "now a file\n").unwrap();
        fs::write(repo.join("zz-after.txt"), "later\n").unwrap();
        let after = snap(&repo, "after");
        let diff = diff_snapshots(&repo, &before, &after).unwrap();
        let link = file(&diff, "link");
        assert_eq!(link.change, TurnDiffChange::TypeChanged);
        let patch = link.patch.as_deref().unwrap();
        assert!(patch.contains("-keep.txt") && patch.contains("+now a file"), "{patch}");
        let later = file(&diff, "zz-after.txt").patch.as_deref().unwrap();
        assert!(later.contains("+later"), "{later}");
    }

    #[test]
    fn turn_diff_withholds_huge_blobs_even_when_attributes_force_text() {
        let (_dir, repo) = init_repo();
        fs::write(repo.join(".gitattributes"), "*.json diff\n").unwrap();
        let before = snap(&repo, "before");
        let line = "x".repeat((MAX_PATCH_BLOB_BYTES + 1) as usize);
        fs::write(repo.join("data.json"), format!("{line}\n")).unwrap();
        let after = snap(&repo, "after");
        let diff = diff_snapshots(&repo, &before, &after).unwrap();
        let data = file(&diff, "data.json");
        assert!(!data.binary);
        assert_eq!((data.additions, data.deletions), (0, 0), "never line-counted");
        assert_eq!(data.patch_omitted, Some(TurnDiffPatchOmitted::TooLarge));
    }

    #[test]
    fn a_file_replaced_by_a_directory_keeps_only_its_own_patch() {
        let (_dir, repo) = init_repo();
        let before = snap(&repo, "before");
        fs::remove_file(repo.join("gone.txt")).unwrap();
        fs::create_dir_all(repo.join("gone.txt")).unwrap();
        fs::write(repo.join("gone.txt/inner.txt"), "inside\n").unwrap();
        let after = snap(&repo, "after");
        let diff = diff_snapshots(&repo, &before, &after).unwrap();
        let gone = file(&diff, "gone.txt").patch.as_deref().unwrap();
        assert!(gone.contains("-bye") && !gone.contains("inside"), "{gone}");
        let inner = file(&diff, "gone.txt/inner.txt").patch.as_deref().unwrap();
        assert!(inner.contains("+inside"), "{inner}");
    }

    #[test]
    fn snapshot_ignores_assume_unchanged_marks() {
        let (_dir, repo) = init_repo();
        run(&repo, &["update-index", "--assume-unchanged", "keep.txt"]);
        let before = snap(&repo, "before");
        fs::write(repo.join("keep.txt"), "edited by the agent\n").unwrap();
        let after = snap(&repo, "after");
        let diff = diff_snapshots(&repo, &before, &after).unwrap();
        assert_eq!(file(&diff, "keep.txt").change, TurnDiffChange::Modified);
    }

    #[test]
    fn snapshot_works_before_the_first_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        let before = snap(repo, "before");
        fs::write(repo.join("hello.txt"), "hi\n").unwrap();
        let after = snap(repo, "after");
        let diff = diff_snapshots(repo, &before, &after).unwrap();
        assert_eq!(file(&diff, "hello.txt").change, TurnDiffChange::Added);
        // The user's index is untouched.
        assert_eq!(run(repo, &["ls-files"]), "");
    }

    #[test]
    fn snapshot_drops_files_that_became_ignored() {
        let (_dir, repo) = init_repo();
        fs::create_dir_all(repo.join("dist")).unwrap();
        fs::write(repo.join("dist/app.js"), "built\n").unwrap();
        let first = snap(&repo, "before");
        let listed = |commit: &str| run(&repo, &["ls-tree", "-r", "--name-only", commit]);
        assert!(listed(&first).contains("dist/app.js"));

        fs::write(repo.join(".gitignore"), "ignored/\ndist/\n").unwrap();
        let second = snap(&repo, "after");
        assert!(!listed(&second).contains("dist/app.js"));

        // An ignored file the user tracks stays.
        run(&repo, &["add", "-f", "dist/app.js"]);
        let third = snap(&repo, "after");
        assert!(listed(&third).contains("dist/app.js"));
    }

    #[test]
    fn turn_diff_of_identical_snapshots_is_empty() {
        let (_dir, repo) = init_repo();
        let before = snap(&repo, "before");
        let after = snap(&repo, "after");
        assert_eq!(diff_snapshots(&repo, &before, &after).unwrap(), TurnDiff::default());
    }

    #[test]
    fn turn_diff_ignores_relative_and_noprefix_config() {
        let (_dir, repo) = init_repo();
        run(&repo, &["config", "diff.relative", "true"]);
        run(&repo, &["config", "diff.noprefix", "true"]);
        fs::create_dir_all(repo.join("sub")).unwrap();
        let before = snap(&repo, "before");
        fs::write(repo.join("keep.txt"), "changed\n").unwrap();
        let after = snap(&repo, "after");
        let diff = diff_snapshots(&repo.join("sub"), &before, &after).unwrap();
        // Paths are relative to the root the command reports, not the cwd.
        let root = repo_root(&repo.join("sub")).unwrap();
        assert_eq!(Path::new(&root).canonicalize().unwrap(), repo.canonicalize().unwrap());
        assert_eq!(repo_root(&repo.join("missing")), None);
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
        let index = snapshot_index(&repo, "thread-a").unwrap();
        snapshot(&repo, "thread-a", &turn_diff_ref_name("thread-a", "2", "before"), "test")
            .unwrap()
            .unwrap();
        assert!(index.exists());
        delete_thread_refs(&repo, "thread-a").unwrap();
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", &mine]).is_err());
        assert!(!index.exists());
        assert_eq!(run(&repo, &["rev-parse", &other]), head);
        // Nothing left to delete is still a success.
        delete_thread_refs(&repo, "thread-a").unwrap();
    }
}
