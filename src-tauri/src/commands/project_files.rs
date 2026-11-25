//! `list_project_files` — fuzzy filename search for the chat composer's
//! `@` mention popup and `+ → File…` picker (Step 8 Stage 1).
//!
//! Audit decision (Step 8 research): `search_project_index` is a
//! lexical *content* index (chunks files into 40-line blocks, scored
//! by term-count). `search_file_names` is filename-only but lacks
//! fuzzy ranking, scoring, and caching. This command lives alongside
//! both — purpose-built for fast filename autocomplete with fuzzy
//! ranking + 60s in-memory cache.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::WalkBuilder;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Step 8 Stage 2 — content tier thresholds. Mirrors the locked
/// research decision: full inline for files ≤200KB AND ≤1500 lines;
/// bounded first-50 preview + outline for anything larger.
const FULL_CONTENT_BYTES_LIMIT: u64 = 200 * 1024;
const FULL_CONTENT_LINE_LIMIT: usize = 1500;
const TRUNCATED_PREVIEW_LINES: usize = 50;
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// A workspace file can be much larger than the attachment preview. Stop
/// scanning after this many text bytes instead of loading the whole file.
const MAX_ATTACHMENT_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_ATTACHMENT_PREVIEW_BYTES: usize = 32 * 1024;
const MAX_ATTACHMENT_OUTLINE_ENTRIES: usize = 200;

/// Step 8 Stage 3 — folder-tree rendering caps. Walking is bounded by
/// depth and visits; the rendered tree uses at most this many entries.
const FOLDER_TREE_MAX_ENTRIES: usize = 100;
const FOLDER_WALK_MAX_VISITS: usize = 10_000;
const FOLDER_WALK_MAX_DEPTH: usize = 8;
const MAX_ATTACHMENT_IGNORE_FILE_BYTES: u64 = 1024 * 1024;
const MAX_ATTACHMENT_IGNORE_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ATTACHMENT_IGNORE_RULE_LINES: usize = 10_000;

/// Convert a `Path` (or path string) to a forward-slash-separated string
/// for the frontend. On Linux/macOS this is a no-op. On Windows the
/// native separator is `\`, which would surface as `src\lib\foo.ts` in
/// the picker UI and break frontend assertions written for POSIX-style
/// paths. The frontend always expects `/`, so we normalize at the
/// boundary instead of asking every caller to remember.
fn to_forward_slash(p: impl AsRef<Path>) -> String {
    let s = p.as_ref().to_string_lossy().into_owned();
    if std::path::MAIN_SEPARATOR == '/' {
        s
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

/// Cache TTL for the per-cwd file list. Aligns with the frontend skills
/// store TTL (60s) so the user's mental model of "freshness" is
/// consistent across cached lookups.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// Hard upper bound on entries returned to the frontend. Even at the
/// `+` button's submode, no UI surface usefully consumes >500 paths
/// at once; cap below the popup virtualization threshold.
const MAX_LIMIT: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct FileMatch {
    /// Path relative to `cwd`.
    pub path: String,
    /// Absolute, canonicalized path. Used by the frontend to attach
    /// without re-resolving.
    pub absolute_path: String,
    /// Fuzzy match score. Higher = better. `0` for empty-query
    /// (alphabetical) listings.
    pub score: u32,
}

struct FileCandidate<'a> {
    path: &'a str,
    absolute_path: &'a str,
}

impl AsRef<str> for FileCandidate<'_> {
    fn as_ref(&self) -> &str {
        self.path
    }
}

struct CacheEntry {
    /// Relative paths (UTF-8 lossy). Pre-sorted alphabetically for the
    /// empty-query path.
    paths: Vec<String>,
    /// Parallel array of absolute paths, same indices as `paths`.
    absolute: Vec<String>,
    scanned_at: Instant,
}

type CacheSlot = OnceLock<Arc<CacheEntry>>;

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Arc<CacheSlot>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cached_project(cwd: &Path) -> Arc<CacheEntry> {
    cached_project_with(cwd, walk_project)
}

fn cached_project_with(cwd: &Path, walk: impl FnOnce(&Path) -> CacheEntry) -> Arc<CacheEntry> {
    let slot = {
        let mut cache = CACHE.lock().unwrap();
        // An uninitialized slot already owns this folder's in-flight walk.
        // Expired initialized slots are replaced atomically by a new generation.
        if let Some(slot) = cache
            .get(cwd)
            .filter(|slot| {
                slot.get()
                    .is_none_or(|entry| entry.scanned_at.elapsed() < CACHE_TTL)
            })
            .cloned()
        {
            slot
        } else {
            cache.retain(|_, slot| {
                Arc::strong_count(slot) > 1
                    || slot
                        .get()
                        .is_some_and(|entry| entry.scanned_at.elapsed() < CACHE_TTL)
            });
            let slot = Arc::new(CacheSlot::new());
            cache.insert(cwd.to_path_buf(), slot.clone());
            slot
        }
    };

    // Only callers for this folder wait on initialization. In-flight slots
    // survive eviction, and old snapshot readers retain their immutable entry.
    slot.get_or_init(|| Arc::new(walk(cwd))).clone()
}

/// List project files under `cwd`, fuzzy-ranked by `query` if present.
///
/// - Walks `cwd` respecting `.gitignore`, `.ignore`, and
///   `.git/info/exclude` via the `ignore` crate (ripgrep's walker).
/// - Caches the walk result per `cwd` for 60s. Re-walks on cache
///   miss or TTL expiry.
/// - Empty / `None` query returns the first `limit` paths alphabetical.
/// - Non-empty query fuzzy-matches with `nucleo-matcher`.
/// - Non-existent or non-directory `cwd` returns `Ok(vec![])`, not
///   `Err` — the popup shouldn't error-flash when a workspace is
///   transiently misconfigured.
#[tauri::command]
pub async fn list_project_files(
    cwd: String,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<FileMatch>, String> {
    tokio::task::spawn_blocking(move || list_project_files_blocking(cwd, query, limit))
        .await
        .map_err(|e| format!("list_project_files task join failed: {e}"))?
}

fn list_project_files_blocking(
    cwd: String,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<FileMatch>, String> {
    let cwd_path = Path::new(&cwd);
    if !cwd_path.is_dir() {
        return Ok(Vec::new());
    }
    let canonical_cwd = cwd_path
        .canonicalize()
        .unwrap_or_else(|_| cwd_path.to_path_buf());
    let limit = limit.min(MAX_LIMIT);
    if limit == 0 {
        return Ok(Vec::new());
    }

    let entry = cached_project(&canonical_cwd);
    let paths = &entry.paths;
    let absolute = &entry.absolute;

    let trimmed_query = query.as_deref().map(str::trim).unwrap_or("");

    if trimmed_query.is_empty() {
        return Ok(paths
            .iter()
            .take(limit)
            .enumerate()
            .map(|(i, rel)| FileMatch {
                path: rel.clone(),
                absolute_path: absolute[i].clone(),
                score: 0,
            })
            .collect());
    }

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(
        trimmed_query,
        CaseMatching::Smart,
        Normalization::Smart,
    );

    // `match_list` returns items already sorted by score (highest
    // first). Carry each path pair through matching to avoid rebuilding a
    // project-sized lookup map for every keystroke.
    let candidates = paths
        .iter()
        .zip(absolute)
        .map(|(path, absolute_path)| FileCandidate {
            path,
            absolute_path,
        });
    let scored = pattern.match_list(candidates, &mut matcher);

    let results: Vec<FileMatch> = scored
        .into_iter()
        .take(limit)
        .map(|(candidate, score)| FileMatch {
            path: candidate.path.to_string(),
            absolute_path: candidate.absolute_path.to_string(),
            score,
        })
        .collect();
    Ok(results)
}

fn walk_project(cwd: &Path) -> CacheEntry {
    let mut paths = Vec::new();
    let mut absolute = Vec::new();

    let walker = WalkBuilder::new(cwd)
        .hidden(false) // surface dotfiles like `.env.example` if not gitignored
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        .build();

    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        // Skip directories — only file entries are listable.
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let rel = to_forward_slash(abs.strip_prefix(cwd).unwrap_or(&abs));
        // Skip the `.git` directory contents — `ignore` already handles
        // top-level `.git`, but be defensive for nested git repos. The
        // `rel` string is forward-slash-normalized above so this prefix
        // check works on Windows too (where the raw walk produced
        // `.git\foo`).
        if rel.starts_with(".git/") || rel == ".git" {
            continue;
        }
        paths.push(rel);
        absolute.push(abs.to_string_lossy().to_string());
    }

    // Sort by relative path for the empty-query / alphabetical surface.
    // We keep `absolute` aligned via index sort.
    let mut indices: Vec<usize> = (0..paths.len()).collect();
    indices.sort_by(|&a, &b| paths[a].cmp(&paths[b]));
    let sorted_paths: Vec<String> = indices.iter().map(|&i| paths[i].clone()).collect();
    let sorted_absolute: Vec<String> =
        indices.iter().map(|&i| absolute[i].clone()).collect();

    CacheEntry {
        paths: sorted_paths,
        absolute: sorted_absolute,
        scanned_at: Instant::now(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderMatch {
    /// Directory path relative to cwd, no trailing slash.
    pub path: String,
    /// Absolute, canonicalized path.
    pub absolute_path: String,
    /// Fuzzy-match score; `0` for empty-query / alphabetical.
    pub score: u32,
    /// Immediate-children count (files + dirs at depth 1).
    pub item_count: usize,
}

/// List directories under `cwd`, fuzzy-ranked by `query` if present.
/// Reuses the file-walk cache from `list_project_files`: directory
/// prefixes are derived from the cached relative paths, so calling
/// this command after a recent file-list call is essentially free
/// (no second walk).
#[tauri::command]
pub async fn list_project_folders(
    cwd: String,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<FolderMatch>, String> {
    tokio::task::spawn_blocking(move || list_project_folders_blocking(cwd, query, limit))
        .await
        .map_err(|e| format!("list_project_folders task join failed: {e}"))?
}

fn list_project_folders_blocking(
    cwd: String,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<FolderMatch>, String> {
    let cwd_path = Path::new(&cwd);
    if !cwd_path.is_dir() {
        return Ok(Vec::new());
    }
    let canonical_cwd = cwd_path
        .canonicalize()
        .unwrap_or_else(|_| cwd_path.to_path_buf());
    let limit = limit.min(MAX_LIMIT);
    if limit == 0 {
        return Ok(Vec::new());
    }

    // Reuse the same cache the file walker maintains. If absent, walk
    // once now — single source of truth means folder ↔ file lists
    // never drift.
    let entry = cached_project(&canonical_cwd);
    let paths = &entry.paths;

    // Derive directory prefixes + immediate-children counts from the
    // cached file list. Walking the path components builds every
    // ancestor directory; counting "immediate" entries requires only
    // counting paths whose parent equals each candidate dir.
    let mut item_counts: HashMap<String, usize> = HashMap::new();
    let mut dirs: HashSet<String> = HashSet::new();
    for rel in paths {
        let mut current = String::new();
        let segments: Vec<&str> = rel.split('/').collect();
        // Every prefix except the last (which is the file itself) is
        // a directory.
        for seg in &segments[..segments.len().saturating_sub(1)] {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(seg);
            dirs.insert(current.clone());
        }
        // Tally the file's immediate parent so we can surface
        // item_count in the popup chip preview.
        let parent = if segments.len() <= 1 {
            ""
        } else {
            // Reconstruct the parent by joining all but the last segment.
            // Doing it this way avoids allocating per-segment.
            let last = segments.last().unwrap();
            &rel[..rel.len() - last.len() - if rel.contains('/') { 1 } else { 0 }]
        };
        *item_counts.entry(parent.to_string()).or_insert(0) += 1;
    }

    // Fold "" (the root) into the dirs map so it can be returned too
    // when the user wants to attach the workspace root itself.
    dirs.insert(String::new());

    // Counts also need to include subdirectories as items of their parents.
    // For each dir, count direct subdirectories whose parent equals it.
    for dir in &dirs {
        let parent = match dir.rfind('/') {
            Some(i) => &dir[..i],
            None => "",
        };
        if dir == parent {
            continue; // skip self-reference (the root entry)
        }
        *item_counts.entry(parent.to_string()).or_insert(0) += 1;
    }

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let trimmed_query = query.as_deref().map(str::trim).unwrap_or("");

    // Skip the empty-string root entry from the surfaced list — users
    // typically don't want to "attach the project root" via a popup
    // pick (they'd just message the agent without an attachment).
    let mut candidate_dirs: Vec<String> = dirs.into_iter().filter(|d| !d.is_empty()).collect();
    candidate_dirs.sort();

    let make_match = |dir: &str, score: u32| -> FolderMatch {
        let abs = canonical_cwd.join(dir).to_string_lossy().to_string();
        FolderMatch {
            path: dir.to_string(),
            absolute_path: abs,
            score,
            item_count: *item_counts.get(dir).unwrap_or(&0),
        }
    };

    if trimmed_query.is_empty() {
        return Ok(candidate_dirs
            .iter()
            .take(limit)
            .map(|d| make_match(d, 0))
            .collect());
    }

    let pattern = Pattern::parse(trimmed_query, CaseMatching::Smart, Normalization::Smart);
    let scored = pattern.match_list(candidate_dirs.iter().map(String::as_str), &mut matcher);
    let results: Vec<FolderMatch> = scored
        .into_iter()
        .take(limit)
        .map(|(matched_str, score)| make_match(matched_str, score))
        .collect();
    Ok(results)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderAttachmentInfo {
    pub absolute_path: String,
    pub relative_path: Option<String>,
    /// Pre-rendered unicode tree, bounded by the caller's `max_depth` and
    /// the hard depth cap. Truncated at FOLDER_TREE_MAX_ENTRIES with a
    /// trailing marker. `item_count` is a lower bound when the walk stops
    /// early or encounters an error.
    pub tree: String,
    pub item_count: usize,
    pub item_count_lower_bound: bool,
}

fn open_bounded_file(path: &Path, allow_symlinks: bool) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = libc::O_NONBLOCK | if allow_symlinks { 0 } else { libc::O_NOFOLLOW };
        options.custom_flags(flags);
    }
    #[cfg(windows)]
    if !allow_symlinks {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the reparse point itself so a replaced symlink cannot
        // redirect the bounded read to another file.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

#[derive(Default)]
struct FolderRuleSet {
    ignore: Option<Gitignore>,
    gitignore: Option<Gitignore>,
    git_exclude: Option<Gitignore>,
    has_git: bool,
}

#[derive(Clone, Copy)]
enum FolderRuleSource {
    Ignore,
    Gitignore,
    GitExclude,
}

enum RuleReadError {
    Io(String),
    Rejected(String),
}

impl RuleReadError {
    fn into_message(self) -> String {
        match self {
            Self::Io(message) | Self::Rejected(message) => message,
        }
    }
}

struct FolderWalkFilter {
    rules: HashMap<PathBuf, FolderRuleSet>,
    global_rules: Option<Gitignore>,
    global_rules_checked: bool,
    raw_visits: usize,
    exhausted: bool,
    had_error: bool,
    fatal_error: Option<String>,
    checked_rule_dirs: HashSet<PathBuf>,
    rule_bytes: u64,
    rule_lines: usize,
}

impl FolderWalkFilter {
    fn check_global_rule_files(&mut self) -> Result<(), String> {
        if !self.global_rules_checked {
            self.global_rules =
                Self::load_global_rule_files(&mut self.rule_bytes, &mut self.rule_lines)?;
            self.global_rules_checked = true;
        }
        Ok(())
    }

    fn load_global_rule_files(
        rule_bytes: &mut u64,
        rule_lines: &mut usize,
    ) -> Result<Option<Gitignore>, String> {
        #[allow(deprecated)]
        let home = std::env::home_dir();
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|path| path.join(".config")));
        let global_config = std::env::var_os("GIT_CONFIG_GLOBAL")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let system_config = std::env::var_os("GIT_CONFIG_SYSTEM")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/etc/gitconfig"));
        let mut exclude = None;
        for file in [
            global_config,
            home.as_ref().map(|path| path.join(".gitconfig")),
            xdg.as_ref().map(|path| path.join("git/config")),
            Some(system_config),
        ]
        .into_iter()
        .flatten()
        {
            // Git treats unreadable optional config sources as unavailable.
            // Budget and file-shape violations still reject the preview.
            let bytes = match Self::read_bounded_file(&file, rule_bytes, true) {
                Ok(Some(bytes)) => bytes,
                Ok(None) | Err(RuleReadError::Io(_)) => continue,
                Err(RuleReadError::Rejected(message)) => return Err(message),
            };
            if let Some(path) = Self::config_exclude_path(&bytes, home.as_deref()) {
                exclude = Some(path);
                break;
            }
        }
        let cwd = std::env::current_dir()
            .map_err(|error| format!("cannot resolve folder preview working directory: {error}"))?;
        let Some(file) = exclude.or_else(|| xdg.map(|xdg| xdg.join("git/ignore"))) else {
            return Ok(None);
        };
        let bytes = match Self::read_bounded_file(&file, rule_bytes, true) {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err(RuleReadError::Io(_)) => return Ok(None),
            Err(RuleReadError::Rejected(message)) => return Err(message),
        };
        Self::compile_rule_bytes(&cwd, &file, &bytes, rule_lines).map(Some)
    }

    fn config_exclude_path(bytes: &[u8], home: Option<&Path>) -> Option<PathBuf> {
        use std::sync::OnceLock;

        // Match the file picker's `ignore` parser on the bounded snapshot.
        // Its byte matcher ignores unrelated non-UTF-8 text and falls through
        // to lower-priority sources when the selected path cannot be decoded.
        static EXCLUDES_FILE: OnceLock<regex::bytes::Regex> = OnceLock::new();
        let matcher = EXCLUDES_FILE.get_or_init(|| {
            regex::bytes::Regex::new(r#"(?im-u)^\s*excludesfile\s*=\s*"?\s*(\S+?)\s*"?\s*$"#)
                .expect("valid Git excludesFile pattern")
        });
        let captures = matcher.captures(bytes)?;
        let value = std::str::from_utf8(captures.get(1)?.as_bytes()).ok()?;
        Some(if let Some(rest) = value.strip_prefix('~') {
            if rest.is_empty() || rest.starts_with(std::path::is_separator) {
                home.map(|home| home.join(rest.trim_start_matches(std::path::is_separator)))
                    .unwrap_or_else(|| PathBuf::from(value))
            } else {
                PathBuf::from(value)
            }
        } else {
            PathBuf::from(value)
        })
    }

    fn has_repository_marker(dir: &Path) -> bool {
        dir.join(".git").metadata().is_ok() || dir.join(".jj").exists()
    }

    fn check_rule_files(&mut self, dir: &Path, load_git_rules: bool) -> Result<(), String> {
        if !self.checked_rule_dirs.insert(dir.to_path_buf()) {
            return Ok(());
        }
        let ignore = self.load_rule_file(dir, &dir.join(".ignore"))?;
        let gitignore = if load_git_rules {
            self.load_rule_file(dir, &dir.join(".gitignore"))?
        } else {
            None
        };
        let (has_git, git_exclude) = if load_git_rules {
            self.load_git_exclude(dir)?
        } else {
            (false, None)
        };
        self.rules.insert(
            dir.to_path_buf(),
            FolderRuleSet {
                ignore,
                gitignore,
                git_exclude,
                has_git,
            },
        );
        Ok(())
    }

    fn load_git_exclude(&mut self, dir: &Path) -> Result<(bool, Option<Gitignore>), String> {
        let dot_git = dir.join(".git");
        let dot_git_metadata = match std::fs::metadata(&dot_git) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((dir.join(".jj").exists(), None));
            }
            Err(error) => {
                return Err(format!(
                    "cannot inspect attachment ignore rules at {}: {error}",
                    dot_git.display()
                ));
            }
        };
        if dot_git_metadata.is_dir() {
            let exclude = self.load_rule_file(dir, &dot_git.join("info/exclude"))?;
            return Ok((true, exclude));
        }
        // Linked worktrees store their Git directory in a `.git` file and
        // may keep `info/exclude` outside the attached folder.
        if !dot_git_metadata.is_file() {
            return Err(format!(
                "attachment ignore rules are not a regular file: {}",
                dot_git.display()
            ));
        }
        // The picker follows linked-worktree pointer symlinks. This read is
        // still capped and validates the opened handle as a regular file.
        let bytes = Self::read_bounded_file(&dot_git, &mut self.rule_bytes, true)
            .map_err(RuleReadError::into_message)?
            .ok_or_else(|| format!("missing attachment Git directory: {}", dot_git.display()))?;
        let git_dir_line = Self::rule_line(&bytes)?
            .ok_or_else(|| format!("invalid attachment Git directory: {}", dot_git.display()))?;
        let Some(git_dir) = git_dir_line.strip_prefix("gitdir: ") else {
            return Err(format!("invalid attachment Git directory: {}", dot_git.display()));
        };
        if git_dir.is_empty() {
            return Err(format!(
                "invalid attachment Git directory: {}",
                dot_git.display()
            ));
        }
        let git_dir = PathBuf::from(git_dir);
        let git_dir = if git_dir.is_absolute() {
            git_dir
        } else {
            dir.join(git_dir)
        };
        let commondir_file = git_dir.join("commondir");
        // Worktree metadata pointers may be symlinks; the bounded reader
        // follows them while validating the opened handle and byte count.
        let commondir_bytes = Self::read_bounded_file(
            &commondir_file,
            &mut self.rule_bytes,
            true,
        )
        .map_err(RuleReadError::into_message)?;
        let common_dir = if let Some(commondir_bytes) = commondir_bytes {
            let commondir_line = Self::rule_line(&commondir_bytes)?.ok_or_else(|| {
                format!(
                    "invalid attachment Git common directory: {}",
                    commondir_file.display()
                )
            })?;
            if commondir_line.is_empty() {
                return Err(format!(
                    "invalid attachment Git common directory: {}",
                    commondir_file.display()
                ));
            }
            let common_dir = PathBuf::from(commondir_line);
            if common_dir.is_absolute() {
                common_dir
            } else {
                git_dir.join(common_dir)
            }
        } else {
            git_dir
        };
        let exclude = self.load_rule_file(dir, &common_dir.join("info/exclude"))?;
        Ok((true, exclude))
    }

    fn load_rule_file(&mut self, dir: &Path, file: &Path) -> Result<Option<Gitignore>, String> {
        let bytes = Self::read_bounded_file(file, &mut self.rule_bytes, false)
            .map_err(RuleReadError::into_message)?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let matcher = Self::compile_rule_bytes(dir, file, &bytes, &mut self.rule_lines)?;
        Ok(Some(matcher))
    }

    fn compile_rule_bytes(
        dir: &Path,
        file: &Path,
        bytes: &[u8],
        rule_lines: &mut usize,
    ) -> Result<Gitignore, String> {
        let mut builder = GitignoreBuilder::new(dir);
        let mut rejected_lines = 0;
        let mut first_rejected_line = None;
        for (index, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
            *rule_lines += 1;
            if *rule_lines > MAX_ATTACHMENT_IGNORE_RULE_LINES {
                return Err("attachment ignore rules exceed 10,000 lines".to_string());
            }
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            let Ok(line) = std::str::from_utf8(line) else {
                rejected_lines += 1;
                first_rejected_line.get_or_insert(index + 1);
                continue;
            };
            let line = line.trim_end_matches('\r');
            let line = if index == 0 {
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            if builder.add_line(None, line).is_err() {
                rejected_lines += 1;
                first_rejected_line.get_or_insert(index + 1);
            }
        }
        if rejected_lines > 0 {
            log::warn!(
                "Skipped {rejected_lines} malformed attachment ignore rules in {} (first at line {})",
                file.display(),
                first_rejected_line.expect("a rejected line was counted")
            );
        }
        let matcher = builder.build().map_err(|error| {
            format!("invalid attachment ignore rules at {}: {error}", file.display())
        })?;
        Ok(matcher)
    }

    fn is_ignored(&mut self, path: &Path, is_dir: bool) -> Result<bool, String> {
        if let Some(ignored) = self.match_source(path, is_dir, FolderRuleSource::Ignore, None) {
            return Ok(ignored);
        }
        let git_root = path.parent().and_then(|parent| {
            parent.ancestors().find(|directory| {
                self.rules
                    .get(*directory)
                    .is_some_and(|rules| rules.has_git)
            })
        });
        let Some(git_root) = git_root else {
            return Ok(false);
        };
        for source in [FolderRuleSource::Gitignore, FolderRuleSource::GitExclude] {
            if let Some(ignored) = self.match_source(path, is_dir, source, Some(git_root)) {
                return Ok(ignored);
            }
        }
        self.check_global_rule_files()?;
        Ok(self
            .global_rules
            .as_ref()
            .is_some_and(|rules| rules.matched(path, is_dir).is_ignore()))
    }

    fn match_source(
        &self,
        path: &Path,
        is_dir: bool,
        source: FolderRuleSource,
        git_root: Option<&Path>,
    ) -> Option<bool> {
        for directory in path.parent()?.ancestors() {
            if git_root.is_some_and(|root| !directory.starts_with(root)) {
                break;
            }
            let Some(rules) = self.rules.get(directory) else {
                continue;
            };
            let matcher = match source {
                FolderRuleSource::Ignore => rules.ignore.as_ref(),
                FolderRuleSource::Gitignore => rules.gitignore.as_ref(),
                FolderRuleSource::GitExclude => rules.git_exclude.as_ref(),
            };
            let Some(matcher) = matcher else {
                continue;
            };
            let matched = matcher.matched(path, is_dir);
            if matched.is_ignore() {
                return Some(true);
            }
            if matched.is_whitelist() {
                return Some(false);
            }
        }
        None
    }

    fn read_bounded_file(
        file: &Path,
        rule_bytes: &mut u64,
        allow_symlinks: bool,
    ) -> Result<Option<Vec<u8>>, RuleReadError> {
        #[cfg(unix)]
        if allow_symlinks && file == Path::new("/dev/null") {
            return Ok(None);
        }
        #[cfg(windows)]
        if allow_symlinks && file.to_string_lossy().eq_ignore_ascii_case("NUL") {
            return Ok(None);
        }
        let metadata = match if allow_symlinks {
            std::fs::metadata(file)
        } else {
            std::fs::symlink_metadata(file)
        } {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RuleReadError::Io(format!(
                    "cannot inspect attachment ignore rules at {}: {error}",
                    file.display()
                )));
            }
        };
        if !metadata.is_file() {
            return Err(RuleReadError::Rejected(format!(
                "attachment ignore rules are not a regular file: {}",
                file.display()
            )));
        }
        if metadata.len() > MAX_ATTACHMENT_IGNORE_FILE_BYTES {
            return Err(RuleReadError::Rejected(format!(
                "attachment ignore rule file exceeds 1 MiB: {}",
                file.display()
            )));
        }
        let reader = open_bounded_file(file, allow_symlinks).map_err(|error| {
            RuleReadError::Io(format!(
                "cannot read attachment ignore rules at {}: {error}",
                file.display()
            ))
        })?;
        let opened_metadata = reader.metadata().map_err(|error| {
            RuleReadError::Io(format!(
                "cannot inspect attachment ignore rules at {}: {error}",
                file.display()
            ))
        })?;
        if !opened_metadata.is_file()
            || (!allow_symlinks && opened_metadata.file_type().is_symlink())
        {
            return Err(RuleReadError::Rejected(format!(
                "attachment ignore rules are not a regular file: {}",
                file.display()
            )));
        }
        if opened_metadata.len() > MAX_ATTACHMENT_IGNORE_FILE_BYTES {
            return Err(RuleReadError::Rejected(format!(
                "attachment ignore rule file exceeds 1 MiB: {}",
                file.display()
            )));
        }
        let mut bytes = Vec::new();
        reader
            .take(MAX_ATTACHMENT_IGNORE_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                RuleReadError::Io(format!(
                    "cannot read attachment ignore rules at {}: {error}",
                    file.display()
                ))
            })?;
        if bytes.len() as u64 > MAX_ATTACHMENT_IGNORE_FILE_BYTES {
            return Err(RuleReadError::Rejected(format!(
                "attachment ignore rule file exceeds 1 MiB: {}",
                file.display()
            )));
        }
        *rule_bytes = rule_bytes.saturating_add(
            metadata.len().max(opened_metadata.len()).max(bytes.len() as u64),
        );
        if *rule_bytes > MAX_ATTACHMENT_IGNORE_TOTAL_BYTES {
            return Err(RuleReadError::Rejected(
                "attachment ignore rules exceed 4 MiB in total".to_string(),
            ));
        }
        Ok(Some(bytes))
    }

    fn rule_line(bytes: &[u8]) -> Result<Option<&str>, String> {
        let Some(line) = bytes.split(|byte| *byte == b'\n').next() else {
            return Ok(None);
        };
        if line.is_empty() && bytes.is_empty() {
            return Ok(None);
        }
        let line = std::str::from_utf8(line)
            .map_err(|_| "invalid attachment Git metadata".to_string())?;
        Ok(Some(line.trim_end_matches('\r')))
    }
}

/// Read a folder for attachment. Walks the directory with depth and raw-entry
/// caps, applies the same ignore rules as the file list, renders a unicode
/// tree preview, and reports the item count seen before the walk stops.
#[tauri::command]
pub async fn read_folder_for_attachment(
    absolute_path: String,
    cwd: Option<String>,
    max_depth: usize,
) -> Result<FolderAttachmentInfo, String> {
    tokio::task::spawn_blocking(move || {
        read_folder_for_attachment_blocking(absolute_path, cwd, max_depth)
    })
    .await
    .map_err(|e| format!("read_folder_for_attachment task join failed: {e}"))?
}

fn read_folder_for_attachment_blocking(
    absolute_path: String,
    cwd: Option<String>,
    max_depth: usize,
) -> Result<FolderAttachmentInfo, String> {
    read_folder_for_attachment_with_limit(absolute_path, cwd, max_depth, FOLDER_WALK_MAX_VISITS)
}

fn read_folder_for_attachment_with_limit(
    absolute_path: String,
    cwd: Option<String>,
    max_depth: usize,
    max_visits: usize,
) -> Result<FolderAttachmentInfo, String> {
    let supplied_path = Path::new(&absolute_path);
    if !supplied_path.is_dir() {
        return Err(format!("not a directory: {absolute_path}"));
    }
    let canonical_path = supplied_path
        .canonicalize()
        .map_err(|error| format!("cannot resolve attachment folder: {error}"))?;
    let path = canonical_path.as_path();
    let depth = max_depth.clamp(1, FOLDER_WALK_MAX_DEPTH);

    // The raw walker charges every candidate before filtering it. Rules are
    // compiled only from bounded bytes captured by the filter.
    let mut filter_state = FolderWalkFilter {
        rules: HashMap::new(),
        global_rules: None,
        global_rules_checked: false,
        raw_visits: 0,
        exhausted: false,
        had_error: false,
        fatal_error: None,
        checked_rule_dirs: HashSet::new(),
        rule_bytes: 0,
        rule_lines: 0,
    };
    // The canonical root is shared by the rules and the raw walk so symlinked
    // entry paths cannot select a different set of parent rule files.
    let git_root = path
        .ancestors()
        .find(|ancestor| FolderWalkFilter::has_repository_marker(ancestor));
    for ancestor in path.ancestors() {
        let load_git_rules = git_root.is_some_and(|root| ancestor.starts_with(root));
        filter_state.check_rule_files(ancestor, load_git_rules)?;
    }
    let filter = Arc::new(Mutex::new(filter_state));
    let filter_for_walk = Arc::clone(&filter);
    let root_for_filter = path.to_path_buf();
    let root_has_git = git_root.is_some();
    // Keep filesystem iteration order here: sorting before the visit cap
    // would materialize every entry in a wide directory.
    let mut raw_walker = WalkBuilder::new(path);
    raw_walker
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .follow_links(false)
        .max_depth(Some(depth))
        .filter_entry(move |entry| {
            let mut state = filter_for_walk.lock().expect("folder walk filter lock");
            if state.raw_visits >= max_visits {
                // Yield one sentinel entry so the caller can stop the walk
                // without scanning the rest of this directory.
                state.exhausted = true;
                return true;
            }
            state.raw_visits += 1;
            let Ok(relative) = entry.path().strip_prefix(&root_for_filter) else {
                state.had_error = true;
                return false;
            };
            if relative.components().any(|component| {
                matches!(component, std::path::Component::Normal(name) if name == ".git")
            }) {
                return false;
            }
            if let Some(parent) = entry.path().parent() {
                let load_git_rules = root_has_git
                    || parent.ancestors().any(|ancestor| {
                        state.rules.get(ancestor).is_some_and(|rules| rules.has_git)
                    })
                    || FolderWalkFilter::has_repository_marker(parent);
                if let Err(error) = state.check_rule_files(parent, load_git_rules) {
                    state.fatal_error = Some(error);
                    return true;
                }
            }
            if entry.depth() == 0 {
                return true;
            }
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            match state.is_ignored(entry.path(), is_dir) {
                Ok(ignored) => !ignored,
                Err(error) => {
                    state.fatal_error = Some(error);
                    true
                }
            }
        });

    let mut entries: Vec<(PathBuf, bool)> = Vec::new();
    let mut item_count_lower_bound = false;
    for result in raw_walker.build() {
        let state = filter.lock().expect("folder walk filter lock");
        if let Some(error) = &state.fatal_error {
            return Err(error.clone());
        }
        if state.exhausted {
            item_count_lower_bound = true;
            break;
        }
        drop(state);
        let Ok(entry) = result else {
            let mut state = filter.lock().expect("folder walk filter lock");
            if state.raw_visits >= max_visits {
                item_count_lower_bound = true;
                break;
            }
            state.raw_visits += 1;
            item_count_lower_bound = true;
            continue;
        };
        // The walker emits the root itself first — skip it; we only
        // want descendants.
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let p = entry.path().to_path_buf();
        entries.push((p, is_dir));
    }
    item_count_lower_bound |= filter.lock().expect("folder walk filter lock").had_error;

    // Sort: directories first, then files; alphabetical within each
    // group at every depth level.
    entries.sort_by(|(a, a_dir), (b, b_dir)| {
        let a_rel = a.strip_prefix(path).unwrap_or(a).to_string_lossy().to_string();
        let b_rel = b.strip_prefix(path).unwrap_or(b).to_string_lossy().to_string();
        let a_segments: Vec<&str> = a_rel.split('/').collect();
        let b_segments: Vec<&str> = b_rel.split('/').collect();
        let n = a_segments.len().min(b_segments.len());
        for i in 0..n {
            let same = a_segments[i] == b_segments[i];
            if !same {
                // At this divergence point, prefer dirs over files
                // only if we're at the LAST segment (the diverged
                // entries themselves). Otherwise alphabetic.
                let a_at_end = i + 1 == a_segments.len();
                let b_at_end = i + 1 == b_segments.len();
                if a_at_end && b_at_end {
                    return b_dir.cmp(a_dir).then(a_segments[i].cmp(b_segments[i]));
                }
                return a_segments[i].cmp(b_segments[i]);
            }
        }
        a_segments.len().cmp(&b_segments.len())
    });

    let item_count = entries.len();
    let tree = render_tree(
        path,
        supplied_path,
        &entries,
        FOLDER_TREE_MAX_ENTRIES,
        item_count_lower_bound,
    );

    let relative_path = cwd
        .as_deref()
        .map(Path::new)
        .and_then(|c| c.canonicalize().ok())
        .and_then(|canonical_cwd| {
            path.canonicalize().ok().and_then(|canonical_path| {
                canonical_path
                    .strip_prefix(&canonical_cwd)
                    .ok()
                    .map(to_forward_slash)
            })
        });

    Ok(FolderAttachmentInfo {
        absolute_path,
        relative_path,
        tree,
        item_count,
        item_count_lower_bound,
    })
}

/// Render a sorted entry list as a unicode tree. Each line carries
/// the appropriate `├──`, `└──`, or `│   ` prefix based on its
/// relationship to siblings at the same depth.
fn render_tree(
    root: &Path,
    display_root: &Path,
    entries: &[(PathBuf, bool)],
    cap: usize,
    count_lower_bound: bool,
) -> String {
    if entries.is_empty() {
        return if count_lower_bound {
            "(no scanned entries; additional entries may be omitted)".to_string()
        } else {
            "(empty)".to_string()
        };
    }

    // For a tree renderer to compute the right prefix at each depth,
    // we need to know which entries are the LAST in their parent's
    // child group. Walk the sorted list and, for each entry, find
    // whether the next entry shares the same parent — if not, this
    // entry is the last sibling.
    let mut last_at_depth: Vec<bool> = Vec::with_capacity(entries.len().min(cap));
    for i in 0..entries.len().min(cap) {
        let (cur, _) = &entries[i];
        let cur_parent = cur.parent();
        let mut is_last = true;
        for j in (i + 1)..entries.len() {
            let (next, _) = &entries[j];
            if next.parent() == cur_parent {
                is_last = false;
                break;
            }
            // If the next entry is deeper than current's parent, keep
            // scanning — they're descendants of `cur`, not later
            // siblings.
            let next_depth = next.iter().count();
            let parent_depth = cur_parent.map(|p| p.iter().count()).unwrap_or(0);
            if next_depth <= parent_depth {
                break;
            }
        }
        last_at_depth.push(is_last);
    }

    let truncated = entries.len() > cap;
    let visible = if truncated { &entries[..cap] } else { entries };

    let mut out = Vec::with_capacity(visible.len() + 2);
    out.push(
        display_root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| display_root.display().to_string()),
    );

    // Track the "is this depth's lineage last?" stack so we know when
    // to draw `│` vs space at each level above the current entry.
    let mut ancestor_is_last: Vec<bool> = Vec::new();

    for (idx, (entry_path, is_dir)) in visible.iter().enumerate() {
        let rel = entry_path.strip_prefix(root).unwrap_or(entry_path);
        let depth = rel.components().count();
        // Sync the ancestor stack so its length == depth - 1.
        ancestor_is_last.truncate(depth.saturating_sub(1));
        while ancestor_is_last.len() < depth.saturating_sub(1) {
            // Look UP for the immediate parent's last-flag. The parent
            // is whatever entry has path == rel.parent() prefixed with
            // root. We can get it by walking back through `visible`.
            let parent_components = rel.components().count() - 1;
            let mut parent_last = false;
            for back in (0..idx).rev() {
                let (p, _) = &visible[back];
                let prel = p.strip_prefix(root).unwrap_or(p);
                if prel.components().count() == parent_components {
                    parent_last = last_at_depth.get(back).copied().unwrap_or(false);
                    break;
                }
            }
            ancestor_is_last.push(parent_last);
        }

        let mut line = String::new();
        for &anc_last in &ancestor_is_last {
            line.push_str(if anc_last { "    " } else { "│   " });
        }
        let here_last = last_at_depth.get(idx).copied().unwrap_or(false);
        line.push_str(if here_last { "└── " } else { "├── " });
        let name = rel
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if *is_dir {
            line.push_str(&format!("{name}/"));
        } else {
            line.push_str(&name);
        }
        out.push(line);
    }

    if truncated {
        let more = entries.len() - cap;
        out.push(if count_lower_bound {
            format!("… {more} more scanned entries; additional entries may be omitted")
        } else {
            format!("… {more} more entries")
        });
    } else if count_lower_bound {
        out.push("… additional entries may be omitted".to_string());
    }

    out.join("\n")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineEntry {
    pub kind: String,
    pub name: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAttachmentInfo {
    pub absolute_path: String,
    pub relative_path: Option<String>,
    pub line_count: usize,
    /// True when scanning stopped at the text-byte limit; `line_count`
    /// then counts only the scanned prefix.
    pub line_count_lower_bound: bool,
    pub bytes: u64,
    pub language: Option<String>,
    pub is_text: bool,
    pub content: String,
    pub truncated: bool,
    pub outline: Option<Vec<OutlineEntry>>,
}

/// Read a file for attachment. Full content for small files
/// (≤200KB AND ≤1500 lines); bounded preview + outline for larger files.
/// Binary files return `is_text: false` with empty content. Errors
/// only on actual I/O failures (missing path, permission denied) —
/// not on "file too large" or "binary" since both are normal cases
/// the chip needs to render.
#[tauri::command]
pub async fn read_file_for_attachment(
    absolute_path: String,
    cwd: Option<String>,
) -> Result<FileAttachmentInfo, String> {
    tokio::task::spawn_blocking(move || read_file_for_attachment_blocking(absolute_path, cwd))
        .await
        .map_err(|e| format!("read_file_for_attachment task join failed: {e}"))?
}

fn read_file_for_attachment_blocking(
    absolute_path: String,
    cwd: Option<String>,
) -> Result<FileAttachmentInfo, String> {
    let path = Path::new(&absolute_path);
    if !path.is_file() {
        return Err(format!("not a file: {absolute_path}"));
    }

    let file = open_bounded_file(path, true)
        .map_err(|e| format!("read failed for {absolute_path}: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("metadata read failed for {absolute_path}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("not a file: {absolute_path}"));
    }
    let bytes = metadata.len();
    let (raw, line_count_lower_bound) = read_attachment_bytes(file)
        .map_err(|e| format!("read failed for {absolute_path}: {e}"))?;

    let language = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_string());

    let relative_path = cwd
        .as_deref()
        .map(Path::new)
        .and_then(|cwd_path| cwd_path.canonicalize().ok())
        .and_then(|canonical_cwd| {
            path.canonicalize().ok().and_then(|canonical_path| {
                canonical_path
                    .strip_prefix(&canonical_cwd)
                    .ok()
                    .map(to_forward_slash)
            })
        });

    if is_binary(&raw) {
        return Ok(FileAttachmentInfo {
            absolute_path,
            relative_path,
            line_count: 0,
            line_count_lower_bound: false,
            bytes,
            language,
            is_text: false,
            content: String::new(),
            truncated: false,
            outline: None,
        });
    }

    // Decode lossily so byte sequences that aren't strict UTF-8 still
    // surface useful content rather than failing the attachment.
    let content = String::from_utf8_lossy(&raw).into_owned();
    let line_count = content.lines().count();

    let fits_inline = !line_count_lower_bound
        && bytes <= FULL_CONTENT_BYTES_LIMIT
        && content.len() <= FULL_CONTENT_BYTES_LIMIT as usize
        && line_count <= FULL_CONTENT_LINE_LIMIT;

    if fits_inline {
        return Ok(FileAttachmentInfo {
            absolute_path,
            relative_path,
            line_count,
            line_count_lower_bound: false,
            bytes,
            language: language.clone(),
            is_text: true,
            content,
            truncated: false,
            outline: None,
        });
    }

    // Truncated path — preview + outline.
    let preview = bounded_preview(&content);
    let outline = extract_outline(&content, language.as_deref());

    Ok(FileAttachmentInfo {
        absolute_path,
        relative_path,
        line_count,
        line_count_lower_bound,
        bytes,
        language,
        is_text: true,
        content: preview,
        truncated: true,
        outline: Some(outline),
    })
}

fn read_attachment_bytes(mut reader: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut raw = Vec::new();
    reader
        .by_ref()
        .take(BINARY_SNIFF_BYTES as u64)
        .read_to_end(&mut raw)?;
    // Binary attachments only expose metadata. Text reads at most one byte
    // beyond the cap so a growing file is bounded even if metadata is stale.
    if is_binary(&raw) {
        return Ok((raw, false));
    }
    reader
        .take((MAX_ATTACHMENT_TEXT_BYTES + 1 - raw.len()) as u64)
        .read_to_end(&mut raw)?;
    let lower_bound = raw.len() > MAX_ATTACHMENT_TEXT_BYTES;
    raw.truncate(MAX_ATTACHMENT_TEXT_BYTES);
    Ok((raw, lower_bound))
}

fn bounded_preview(content: &str) -> String {
    let mut preview = String::new();
    let mut first = true;
    for line in content.lines().take(TRUNCATED_PREVIEW_LINES) {
        let separator = usize::from(!first);
        let room = MAX_ATTACHMENT_PREVIEW_BYTES.saturating_sub(preview.len() + separator);
        if room == 0 {
            break;
        }
        if separator != 0 {
            preview.push('\n');
        }
        first = false;
        if line.len() > room {
            let mut end = room;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            preview.push_str(&line[..end]);
            break;
        }
        preview.push_str(line);
    }
    preview
}

fn is_binary(bytes: &[u8]) -> bool {
    let limit = bytes.len().min(BINARY_SNIFF_BYTES);
    bytes[..limit].iter().any(|b| *b == 0)
}

/// Per-language outline regexes. Each `Regex::captures` pattern must
/// produce a capture group whose name matches `name_capture` — that's
/// the symbol the outline records. `kind_for` maps the matched
/// declaration keyword (or a fixed string) to the outline kind label.
struct LanguageOutline {
    re: Regex,
    /// Index of the capture group containing the declaration keyword
    /// ("fn", "class", "def", …). Set to `None` for languages with a
    /// fixed kind (e.g. markdown headings).
    kind_capture: Option<usize>,
    /// Index of the capture group containing the symbol name.
    name_capture: usize,
    /// Override that takes precedence over the matched keyword. Used
    /// for languages like markdown where the outline kind is
    /// always the same.
    fixed_kind: Option<&'static str>,
}

static OUTLINE_TS_JS: LazyLock<LanguageOutline> = LazyLock::new(|| LanguageOutline {
    re: Regex::new(
        r"(?m)^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?(function|class|const|let|var|interface|type|enum)\s+([A-Za-z_$][A-Za-z0-9_$]*)",
    )
    .expect("ts/js outline regex"),
    kind_capture: Some(1),
    name_capture: 2,
    fixed_kind: None,
});

static OUTLINE_RS: LazyLock<LanguageOutline> = LazyLock::new(|| LanguageOutline {
    re: Regex::new(
        r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(fn|struct|enum|trait|impl|mod)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("rs outline regex"),
    kind_capture: Some(1),
    name_capture: 2,
    fixed_kind: None,
});

static OUTLINE_PY: LazyLock<LanguageOutline> = LazyLock::new(|| LanguageOutline {
    re: Regex::new(r"(?m)^\s*(?:async\s+)?(def|class)\s+([A-Za-z_][A-Za-z0-9_]*)")
        .expect("py outline regex"),
    kind_capture: Some(1),
    name_capture: 2,
    fixed_kind: None,
});

static OUTLINE_GO: LazyLock<LanguageOutline> = LazyLock::new(|| LanguageOutline {
    re: Regex::new(
        r"(?m)^(func|type)\s+(?:\([^)]*\)\s+)?([A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("go outline regex"),
    kind_capture: Some(1),
    name_capture: 2,
    fixed_kind: None,
});

static OUTLINE_MD: LazyLock<LanguageOutline> = LazyLock::new(|| LanguageOutline {
    re: Regex::new(r"(?m)^(#{1,3})\s+(.+)$").expect("md outline regex"),
    kind_capture: None,
    name_capture: 2,
    fixed_kind: Some("heading"),
});

fn select_outline(language: Option<&str>) -> Option<&'static LanguageOutline> {
    match language? {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(&OUTLINE_TS_JS),
        "rs" => Some(&OUTLINE_RS),
        "py" => Some(&OUTLINE_PY),
        "go" => Some(&OUTLINE_GO),
        "md" | "markdown" => Some(&OUTLINE_MD),
        _ => None,
    }
}

fn extract_outline(content: &str, language: Option<&str>) -> Vec<OutlineEntry> {
    let Some(spec) = select_outline(language) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut scanned_to = 0;
    let mut line = 1;
    for caps in spec.re.captures_iter(content) {
        if out.len() >= MAX_ATTACHMENT_OUTLINE_ENTRIES {
            break;
        }
        let Some(name_match) = caps.get(spec.name_capture) else {
            continue;
        };
        let kind = if let Some(fixed) = spec.fixed_kind {
            fixed.to_string()
        } else if let Some(kind_idx) = spec.kind_capture {
            caps.get(kind_idx)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        if kind.is_empty() {
            continue;
        }
        let name: String = name_match.as_str().trim().chars().take(160).collect();
        if name.is_empty() {
            continue;
        }
        let offset = caps.get(0).map(|m| m.start()).unwrap_or(0);
        line += content[scanned_to..offset].bytes().filter(|b| *b == b'\n').count();
        scanned_to = offset;
        out.push(OutlineEntry {
            kind,
            name,
            line,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    async fn assert_yields_while_cache_is_locked<F: std::future::Future>(future: F) -> F::Output {
        use std::sync::mpsc;
        use std::task::{Context, Waker};

        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = std::thread::spawn(move || {
            let _guard = CACHE.lock().unwrap();
            locked_tx.send(()).unwrap();
            // Bound the old synchronous implementation's deadlock so a
            // regression fails instead of hanging the test process.
            let _ = release_rx.recv_timeout(Duration::from_secs(1));
        });
        locked_rx.recv().unwrap();

        let mut future = std::pin::pin!(future);
        let first_poll = future.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        let _ = release_tx.send(());
        blocker.join().unwrap();
        assert!(first_poll.is_pending(), "project filesystem work blocked the async executor");
        future.await
    }

    #[tokio::test]
    async fn file_listing_yields_instead_of_blocking_the_async_executor() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let results = assert_yields_while_cache_is_locked(list_project_files(
            dir.path().to_string_lossy().into_owned(), None, 100,
        )).await.unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn folder_listing_yields_instead_of_blocking_the_async_executor() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let results = assert_yields_while_cache_is_locked(list_project_folders(
            dir.path().to_string_lossy().into_owned(), None, 100,
        )).await.unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn cached_paths_are_shared_and_refresh_preserves_existing_readers() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().canonicalize().unwrap();
        let first = cached_project(&cwd);
        assert!(Arc::ptr_eq(&first, &cached_project(&cwd)));

        let mut expired = walk_project(&cwd);
        expired.scanned_at = Instant::now() - CACHE_TTL;
        let expired = Arc::new(expired);
        CACHE
            .lock()
            .unwrap()
            .insert(cwd.clone(), Arc::new(CacheSlot::from(expired.clone())));
        fs::write(cwd.join("new.txt"), "fresh").unwrap();

        let other = tempdir().unwrap();
        cached_project(other.path());
        assert!(!CACHE.lock().unwrap().contains_key(&cwd));

        let refreshed = cached_project(&cwd);
        assert!(!Arc::ptr_eq(&expired, &refreshed));
        assert!(!expired.paths.iter().any(|path| path == "new.txt"));
        assert!(refreshed.paths.iter().any(|path| path == "new.txt"));
        assert!(Arc::ptr_eq(&refreshed, &cached_project(&cwd)));
    }

    fn assert_concurrent_walks_share_one_entry(expired: bool) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc;

        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().canonicalize().unwrap();
        if expired {
            let mut entry = walk_project(&cwd);
            entry.scanned_at = Instant::now() - CACHE_TTL;
            CACHE
                .lock()
                .unwrap()
                .insert(cwd.clone(), Arc::new(CacheSlot::from(Arc::new(entry))));
        }
        fs::write(cwd.join("fresh.txt"), "fresh generation").unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let (walking_tx, walking_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_cwd = cwd.clone();
        let first_calls = calls.clone();
        let first = std::thread::spawn(move || {
            cached_project_with(&first_cwd, |path| {
                first_calls.fetch_add(1, Ordering::SeqCst);
                let entry = walk_project(path);
                walking_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                entry
            })
        });
        walking_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (second_done_tx, second_done_rx) = mpsc::channel();
        let second_cwd = cwd.clone();
        let second_calls = calls.clone();
        let second = std::thread::spawn(move || {
            second_started_tx.send(()).unwrap();
            let entry = cached_project_with(&second_cwd, |path| {
                second_calls.fetch_add(1, Ordering::SeqCst);
                walk_project(path)
            });
            second_done_tx.send(()).unwrap();
            entry
        });
        second_started_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();

        // Another directory must remain usable while the first directory's
        // walk is paused, including when its cache entry is created now.
        let unrelated = tempdir().unwrap();
        fs::write(unrelated.path().join("other.txt"), "other project").unwrap();
        let unrelated_cwd = unrelated.path().canonicalize().unwrap();
        let (unrelated_tx, unrelated_rx) = mpsc::channel();
        let other = std::thread::spawn(move || {
            let entry = cached_project(&unrelated_cwd);
            unrelated_tx
                .send(entry.paths.iter().any(|path| path == "other.txt"))
                .unwrap();
        });
        let unrelated_ready = unrelated_rx.recv_timeout(Duration::from_secs(2));
        let second_finished_early = second_done_rx
            .recv_timeout(Duration::from_millis(100))
            .is_ok();

        // Release and join every worker before assertions so a failure never
        // leaves a paused scanner or a cache lock behind for peer tests.
        let _ = release_tx.send(());
        let first_entry = first.join().unwrap();
        let second_entry = second.join().unwrap();
        other.join().unwrap();

        assert_eq!(
            unrelated_ready.unwrap(),
            true,
            "one folder blocked another folder's cache"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "each generation must perform exactly one walk"
        );
        assert!(
            !second_finished_early,
            "a concurrent caller started another walk"
        );
        assert!(Arc::ptr_eq(&first_entry, &second_entry));
        assert!(first_entry.paths.iter().any(|path| path == "fresh.txt"));
        assert!(Arc::ptr_eq(&first_entry, &cached_project(&cwd)));
    }

    #[test]
    fn concurrent_cold_cache_requests_share_one_walk_without_blocking_other_projects() {
        assert_concurrent_walks_share_one_entry(false);
    }

    #[test]
    fn concurrent_expired_cache_requests_share_one_walk_without_blocking_other_projects() {
        assert_concurrent_walks_share_one_entry(true);
    }

    fn seed_project(root: &Path) {
        fs::create_dir_all(root.join("src/components/chat")).unwrap();
        fs::create_dir_all(root.join("src/lib")).unwrap();
        fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join("README.md"), "# project\n").unwrap();
        fs::write(
            root.join("src/components/chat/Composer.tsx"),
            "// composer\n",
        )
        .unwrap();
        fs::write(
            root.join("src/components/chat/AttachmentChip.tsx"),
            "// chip\n",
        )
        .unwrap();
        fs::write(root.join("src/lib/utils.ts"), "// utils\n").unwrap();
        fs::write(root.join("node_modules/foo/index.js"), "// noisy\n").unwrap();
        fs::write(root.join(".git/objects/blob"), "junk\n").unwrap();
        fs::write(root.join(".gitignore"), "node_modules\n").unwrap();
    }

    #[tokio::test]
    async fn empty_query_returns_alphabetical_list() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let results = list_project_files(cwd, None, 100).await.unwrap();
        let paths: Vec<&str> = results.iter().map(|m| m.path.as_str()).collect();
        // Alphabetical, gitignored excluded, .git excluded.
        assert!(paths.contains(&".gitignore"));
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&"src/components/chat/AttachmentChip.tsx"));
        assert!(paths.contains(&"src/components/chat/Composer.tsx"));
        assert!(paths.contains(&"src/lib/utils.ts"));
        assert!(!paths.iter().any(|p| p.contains("node_modules")));
        assert!(!paths.iter().any(|p| p.starts_with(".git/")));
        // Strict ordering check: alphabetical.
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
        // Score is 0 for empty-query.
        assert!(results.iter().all(|m| m.score == 0));
    }

    #[tokio::test]
    async fn fuzzy_query_ranks_composer_high() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let results = list_project_files(cwd, Some("comp".into()), 10)
            .await
            .unwrap();
        assert!(!results.is_empty(), "expected some matches");
        // Composer.tsx must be in the top-3 for query "comp".
        let top3: Vec<&str> = results.iter().take(3).map(|m| m.path.as_str()).collect();
        assert!(
            top3.iter()
                .any(|p| p.contains("Composer.tsx")),
            "Composer.tsx not in top-3: {top3:?}"
        );
        // Scores are non-zero on a non-empty query.
        assert!(results.iter().any(|m| m.score > 0));
    }

    #[tokio::test]
    async fn fuzzy_results_preserve_ranking_limits_and_path_pairs() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        for path in ["a/Composer.tsx", "b/Composer.tsx", "comp.ts", "src/café.ts"] {
            let absolute = dir.path().join(path);
            fs::create_dir_all(absolute.parent().unwrap()).unwrap();
            fs::write(absolute, "// candidate\n").unwrap();
        }
        let cwd = dir.path().canonicalize().unwrap();
        let cwd_string = cwd.to_string_lossy().into_owned();
        let alphabetical = list_project_files(cwd_string.clone(), None, MAX_LIMIT)
            .await
            .unwrap();

        for query in [" comp ", "Composer", "composer", "cafe", "src/comp", "zzznotreal"] {
            let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
            let pattern = Pattern::parse(query.trim(), CaseMatching::Smart, Normalization::Smart);
            let expected = pattern.match_list(
                alphabetical.iter().map(|file| file.path.as_str()),
                &mut matcher,
            );
            for limit in [0, 1, 2, MAX_LIMIT] {
                let results = list_project_files(cwd_string.clone(), Some(query.into()), limit)
                    .await
                    .unwrap();
                let actual: Vec<_> = results
                    .iter()
                    .map(|file| (file.path.as_str(), file.absolute_path.clone(), file.score))
                    .collect();
                let expected: Vec<_> = expected
                    .iter()
                    .take(limit)
                    .map(|&(path, score)| {
                        let absolute = cwd.join(path).canonicalize().unwrap();
                        (path, absolute.to_string_lossy().into_owned(), score)
                    })
                    .collect();
                assert_eq!(actual, expected, "query={query:?}, limit={limit}");
            }
        }

        // Equal-score matches keep the cache's alphabetical order, including
        // separate files that share a basename.
        let results = list_project_files(cwd_string, Some("Composer".into()), MAX_LIMIT)
            .await
            .unwrap();
        let a = results
            .iter()
            .position(|file| file.path == "a/Composer.tsx")
            .unwrap();
        let b = results
            .iter()
            .position(|file| file.path == "b/Composer.tsx")
            .unwrap();
        assert_eq!(results[a].score, results[b].score);
        assert!(a < b);
    }

    #[tokio::test]
    async fn fuzzy_results_preserve_duplicates_and_cap_the_limit() {
        let dir = tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let path = "src/Composer.tsx".to_string();
        let absolute: Vec<_> = (0..=MAX_LIMIT)
            .map(|i| {
                cwd.join(format!("{i}/Composer.tsx"))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        // Use distinct absolute paths to detect aliasing when relative names
        // collide. Every cached candidate must retain its own path pair.
        let entry = CacheEntry {
            paths: vec![path.clone(); MAX_LIMIT + 1],
            absolute: absolute.clone(),
            scanned_at: Instant::now(),
        };
        CACHE
            .lock()
            .unwrap()
            .insert(cwd.clone(), Arc::new(CacheSlot::from(Arc::new(entry))));

        for limit in [1, MAX_LIMIT, usize::MAX] {
            let results = list_project_files(
                cwd.to_string_lossy().into_owned(),
                Some("comp".into()),
                limit,
            )
            .await
            .unwrap();
            assert_eq!(results.len(), limit.min(MAX_LIMIT));
            for (file, absolute_path) in results.iter().zip(&absolute) {
                assert_eq!(file.path, path);
                assert_eq!(&file.absolute_path, absolute_path);
                assert_eq!(file.score, results[0].score);
                assert!(file.score > 0);
            }
        }
    }

    #[tokio::test]
    async fn cache_is_hit_within_ttl() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        // First call populates the cache.
        let _ = list_project_files(cwd.clone(), None, 50).await.unwrap();
        // Mutate the project — this would re-appear if we re-walked.
        fs::write(dir.path().join("BRAND_NEW.txt"), "fresh\n").unwrap();
        // Second call should serve from cache (within TTL) and NOT
        // include the new file.
        let cached = list_project_files(cwd, None, 50).await.unwrap();
        let paths: Vec<&str> = cached.iter().map(|m| m.path.as_str()).collect();
        assert!(
            !paths.contains(&"BRAND_NEW.txt"),
            "cache miss within TTL: BRAND_NEW.txt should not be present yet"
        );
    }

    #[tokio::test]
    async fn nonexistent_cwd_returns_empty_vec_not_error() {
        let result =
            list_project_files("/this/path/should/not/exist/codemux".into(), None, 10)
                .await
                .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn excludes_gitignored_and_dot_git() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let results = list_project_files(cwd, Some("foo".into()), 50)
            .await
            .unwrap();
        // node_modules/foo/index.js is gitignored — must not appear.
        assert!(
            !results
                .iter()
                .any(|m| m.path.contains("node_modules")),
            "gitignored node_modules content leaked into results"
        );
    }

    #[tokio::test]
    async fn limit_is_respected() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let results = list_project_files(cwd, None, 2).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn limit_zero_returns_empty() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let results = list_project_files(cwd, None, 0).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn fuzzy_no_match_returns_empty() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let results = list_project_files(cwd, Some("zzznotreal".into()), 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    /// Smoke test against the real codemux worktree. Ignored by default —
    /// run with `--ignored -- --nocapture` to capture sample output for
    /// the Stage 1 deliverable. Not part of CI.
    #[tokio::test]
    #[ignore]
    async fn smoke_against_codemux_worktree() {
        let cwd = "/home/zeus/.codemux/worktrees/codemux/feature-agent-chat".to_string();
        let results =
            list_project_files(cwd.clone(), Some("composer".into()), 10)
                .await
                .unwrap();
        eprintln!("\n--- query=\"composer\", limit=10 ---");
        for r in &results {
            eprintln!("score={:5}  {}", r.score, r.path);
        }
        let alpha = list_project_files(cwd.clone(), None, 5).await.unwrap();
        eprintln!("\n--- empty query, limit=5 (alphabetical) ---");
        for r in &alpha {
            eprintln!("score={:5}  {}", r.score, r.path);
        }
        // Cache hit timing
        let t0 = std::time::Instant::now();
        let _ = list_project_files(cwd, Some("attach".into()), 10)
            .await
            .unwrap();
        eprintln!("\ncache-hit fuzzy match took: {:?}", t0.elapsed());
    }

    // ───── read_file_for_attachment tests (Step 8 Stage 2) ─────

    #[test]
    fn attachment_binary_sniff_does_not_read_the_remaining_file() {
        struct UnreadableTail;
        impl Read for UnreadableTail {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("the binary tail must not be read"))
            }
        }

        let mut prefix = vec![b'x'; BINARY_SNIFF_BYTES];
        prefix[BINARY_SNIFF_BYTES - 1] = 0;
        let header = std::io::Cursor::new(&prefix);
        let (raw, lower_bound) = read_attachment_bytes(header.chain(UnreadableTail)).unwrap();
        assert_eq!(raw, prefix);
        assert!(!lower_bound);
        assert!(is_binary(&raw));
    }

    #[test]
    fn attachment_text_reads_beyond_the_binary_sniff_window() {
        let mut content = vec![b'x'; BINARY_SNIFF_BYTES + 100];
        // Preserve the existing boundary: a NUL immediately after the
        // sniff window does not classify an attachment as binary.
        content[BINARY_SNIFF_BYTES] = 0;
        let (raw, lower_bound) = read_attachment_bytes(std::io::Cursor::new(&content)).unwrap();
        assert_eq!(raw, content);
        assert!(!lower_bound);
        assert!(!is_binary(&raw));
    }

    #[test]
    fn attachment_text_stops_reading_an_unbounded_source() {
        let (raw, lower_bound) = read_attachment_bytes(std::io::repeat(b'x')).unwrap();
        assert_eq!(raw.len(), MAX_ATTACHMENT_TEXT_BYTES);
        assert!(lower_bound);
        assert!(bounded_preview(&String::from_utf8(raw).unwrap()).len() <= MAX_ATTACHMENT_PREVIEW_BYTES);
    }

    #[test]
    fn attachment_outline_limits_declarations_without_losing_line_numbers() {
        let content = (0..500)
            .map(|n| format!("fn item_{n}() {{}}"))
            .collect::<Vec<_>>()
            .join("\n");
        let outline = extract_outline(&content, Some("rs"));
        assert_eq!(outline.len(), MAX_ATTACHMENT_OUTLINE_ENTRIES);
        assert_eq!(outline.last().unwrap().line, MAX_ATTACHMENT_OUTLINE_ENTRIES);
    }

    #[test]
    fn attachment_preview_preserves_blank_lines_and_utf8_at_the_byte_limit() {
        let content = format!("\n\n{}", "é".repeat(MAX_ATTACHMENT_PREVIEW_BYTES));
        let preview = bounded_preview(&content);
        assert!(preview.starts_with("\n\n"));
        assert!(preview.len() <= MAX_ATTACHMENT_PREVIEW_BYTES);
        assert!(preview.is_char_boundary(preview.len()));
    }

    #[tokio::test]
    async fn read_file_full_content_for_small_text_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("small.ts");
        fs::write(&p, "export function hello() {\n  return 1;\n}\n").unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        assert!(info.is_text);
        assert!(!info.truncated);
        assert!(info.outline.is_none());
        assert!(info.content.contains("export function hello"));
        assert_eq!(info.language.as_deref(), Some("ts"));
        assert_eq!(info.line_count, 3);
    }

    #[tokio::test]
    async fn read_file_truncates_when_exceeding_line_limit() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("big.rs");
        // 1501 lines — just over the line limit, well under bytes.
        let body: String = (1..=1501)
            .map(|i| format!("// line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        // Sprinkle a few real declarations so the outline has something
        // to find on the truncated path.
        let with_decls = format!(
            "{body}\nfn foo_fn() {{}}\nstruct BarStruct;\nenum Baz {{}}\n"
        );
        assert!(body.len() > BINARY_SNIFF_BYTES);
        fs::write(&p, &with_decls).unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        assert!(info.is_text);
        assert!(info.truncated);
        let outline = info.outline.expect("outline populated when truncated");
        let preview_line_count = info.content.lines().count();
        assert_eq!(preview_line_count, TRUNCATED_PREVIEW_LINES);
        // line_count reports the FULL file's lines.
        assert_eq!(info.line_count, 1504);
        assert!(
            outline.iter().any(|e| e.kind == "fn" && e.name == "foo_fn"),
            "outline missed foo_fn: {outline:?}"
        );
        assert!(
            outline.iter().any(|e| e.kind == "struct" && e.name == "BarStruct"),
            "outline missed BarStruct: {outline:?}"
        );
        assert!(
            outline.iter().any(|e| e.kind == "enum" && e.name == "Baz"),
            "outline missed Baz: {outline:?}"
        );
    }

    #[tokio::test]
    async fn read_file_truncates_when_exceeding_byte_limit() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("fat.txt");
        // Just over 200KB but well under 1500 lines (one giant line).
        let line: String = "x".repeat(210 * 1024);
        fs::write(&p, &line).unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        assert!(info.is_text);
        assert!(info.truncated);
    }

    #[tokio::test]
    async fn read_file_marks_binary_as_not_text() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("bin.dat");
        let mut content = vec![b'x'; BINARY_SNIFF_BYTES * 2];
        content[BINARY_SNIFF_BYTES - 1] = 0;
        fs::write(&p, &content).unwrap();
        let absolute_path = p.to_string_lossy().to_string();
        let info = read_file_for_attachment(
            absolute_path.clone(),
            Some(dir.path().to_string_lossy().to_string()),
        )
        .await
        .unwrap();
        assert!(!info.is_text);
        assert_eq!(info.bytes, content.len() as u64);
        assert_eq!(info.absolute_path, absolute_path);
        assert_eq!(info.relative_path.as_deref(), Some("bin.dat"));
        assert_eq!(info.language.as_deref(), Some("dat"));
        assert_eq!(info.line_count, 0);
        assert!(info.content.is_empty());
        assert!(info.outline.is_none());
        assert!(!info.truncated);
    }

    #[tokio::test]
    async fn read_file_returns_relative_path_when_under_cwd() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("src/lib");
        fs::create_dir_all(&nested).unwrap();
        let p = nested.join("utils.ts");
        fs::write(&p, "// utils\n").unwrap();
        let info = read_file_for_attachment(
            p.to_string_lossy().to_string(),
            Some(dir.path().to_string_lossy().to_string()),
        )
        .await
        .unwrap();
        assert_eq!(info.relative_path.as_deref(), Some("src/lib/utils.ts"));
    }

    #[tokio::test]
    async fn read_file_returns_none_relative_when_outside_cwd() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        let p = dir1.path().join("a.ts");
        fs::write(&p, "// a\n").unwrap();
        let info = read_file_for_attachment(
            p.to_string_lossy().to_string(),
            Some(dir2.path().to_string_lossy().to_string()),
        )
        .await
        .unwrap();
        assert!(info.relative_path.is_none());
    }

    #[tokio::test]
    async fn read_file_handles_empty_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("empty.ts");
        fs::write(&p, "").unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        assert!(info.is_text);
        assert!(!info.truncated);
        assert_eq!(info.line_count, 0);
        assert_eq!(info.bytes, 0);
        assert_eq!(info.content, "");
    }

    #[cfg(unix)]
    #[test]
    fn read_file_allows_regular_file_symlinks() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, "hello\n").unwrap();
        let link = dir.path().join("linked.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let info = read_file_for_attachment_blocking(link.to_string_lossy().into_owned(), None)
            .unwrap();
        assert_eq!(info.content, "hello\n");
        assert_eq!(info.bytes, 6);
    }

    #[tokio::test]
    async fn read_file_errors_on_missing_path() {
        let err = read_file_for_attachment(
            "/this/path/does/not/exist/codemux-attachment".to_string(),
            None,
        )
        .await
        .unwrap_err();
        assert!(err.contains("not a file"));
    }

    #[tokio::test]
    async fn outline_extracts_typescript_decls() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.ts");
        let mut body = String::from(
            "export function alpha() {}\n\
             export class Beta {}\n\
             export const GAMMA = 1;\n\
             export interface Delta {}\n\
             export type Eps = string;\n\
             export enum Zeta {}\n\
             // padding\n",
        );
        for i in 0..1600 {
            body.push_str(&format!("// pad {i}\n"));
        }
        fs::write(&p, &body).unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        assert!(info.truncated);
        let outline = info.outline.unwrap();
        let names: Vec<&str> = outline.iter().map(|e| e.name.as_str()).collect();
        for expected in ["alpha", "Beta", "GAMMA", "Delta", "Eps", "Zeta"] {
            assert!(
                names.contains(&expected),
                "ts outline missed {expected}: {names:?}"
            );
        }
        let kinds: Vec<&str> = outline.iter().map(|e| e.kind.as_str()).collect();
        for expected in ["function", "class", "const", "interface", "type", "enum"] {
            assert!(
                kinds.contains(&expected),
                "ts outline missed kind {expected}: {kinds:?}"
            );
        }
    }

    #[tokio::test]
    async fn outline_extracts_python_decls() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.py");
        let mut body = String::from(
            "def foo():\n    pass\n\
             async def bar():\n    pass\n\
             class Baz:\n    pass\n",
        );
        for i in 0..1600 {
            body.push_str(&format!("# pad {i}\n"));
        }
        fs::write(&p, &body).unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        let outline = info.outline.unwrap();
        let names: Vec<&str> = outline.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
        assert!(names.contains(&"Baz"));
    }

    #[tokio::test]
    async fn outline_extracts_go_decls_including_methods() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.go");
        let mut body = String::from(
            "func TopLevel() {}\n\
             func (r *Foo) Method() {}\n\
             type Bar struct {}\n",
        );
        for i in 0..1600 {
            body.push_str(&format!("// pad {i}\n"));
        }
        fs::write(&p, &body).unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        let outline = info.outline.unwrap();
        let names: Vec<&str> = outline.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"TopLevel"));
        // Receiver methods: the regex skips `(r *Foo)` and captures `Method`.
        assert!(
            names.contains(&"Method"),
            "go outline missed receiver method: {names:?}"
        );
        assert!(names.contains(&"Bar"));
    }

    #[tokio::test]
    async fn outline_extracts_markdown_headings_at_levels_1_to_3() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.md");
        let mut body = String::from(
            "# h1 title\n\
             ## h2 sub\n\
             ### h3 deep\n\
             #### h4 ignored\n",
        );
        for i in 0..1600 {
            body.push_str(&format!("padding line {i}\n"));
        }
        fs::write(&p, &body).unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        let outline = info.outline.unwrap();
        let names: Vec<&str> = outline.iter().map(|e| e.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("h1 title")));
        assert!(names.iter().any(|n| n.contains("h2 sub")));
        assert!(names.iter().any(|n| n.contains("h3 deep")));
        assert!(
            !names.iter().any(|n| n.contains("h4 ignored")),
            "md outline shouldn't capture h4: {names:?}"
        );
        assert!(outline.iter().all(|e| e.kind == "heading"));
    }

    #[tokio::test]
    async fn outline_empty_for_unknown_extension() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.unknownext");
        let mut body = String::new();
        for i in 0..1600 {
            body.push_str(&format!("line {i}\n"));
        }
        fs::write(&p, &body).unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        assert!(info.truncated);
        assert!(info.outline.unwrap().is_empty());
    }

    #[tokio::test]
    async fn outline_records_line_numbers_correctly() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.ts");
        let mut body = String::from(
            "// preamble\n\
             // padding\n\
             export function alpha() {}\n",
        );
        for i in 0..1600 {
            body.push_str(&format!("// pad {i}\n"));
        }
        fs::write(&p, &body).unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        let entry = info
            .outline
            .unwrap()
            .into_iter()
            .find(|e| e.name == "alpha")
            .expect("alpha in outline");
        // The function declaration is on line 3 of the file (1-indexed,
        // after the two preamble comments). The line-number map must
        // produce exactly that.
        assert_eq!(entry.line, 3);
    }

    // ───── list_project_folders + read_folder_for_attachment (Step 8 Stage 3) ─────

    #[tokio::test]
    async fn list_project_folders_empty_query_returns_alphabetical_dirs() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let out = list_project_folders(cwd, None, 50).await.unwrap();
        let paths: Vec<&str> = out.iter().map(|m| m.path.as_str()).collect();
        // Distinct directory prefixes derived from the file walk.
        assert!(paths.contains(&"src"));
        assert!(paths.contains(&"src/components"));
        assert!(paths.contains(&"src/components/chat"));
        assert!(paths.contains(&"src/lib"));
        // node_modules is gitignored — its dir prefix MUST NOT appear.
        assert!(!paths.iter().any(|p| p.contains("node_modules")));
        // Sorted alphabetically.
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
        // Empty-query returns score = 0.
        assert!(out.iter().all(|m| m.score == 0));
    }

    #[tokio::test]
    async fn list_project_folders_fuzzy_query_ranks_match_high() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let out = list_project_folders(cwd, Some("chat".into()), 10)
            .await
            .unwrap();
        assert!(!out.is_empty(), "expected at least one folder match");
        let top: Vec<&str> = out.iter().take(3).map(|m| m.path.as_str()).collect();
        assert!(
            top.iter().any(|p| p.contains("chat")),
            "expected a `chat` folder in top 3, got {top:?}"
        );
        assert!(out.iter().any(|m| m.score > 0));
    }

    #[tokio::test]
    async fn list_project_folders_item_count_reflects_immediate_children() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let out = list_project_folders(cwd, Some("chat".into()), 10)
            .await
            .unwrap();
        let chat = out
            .iter()
            .find(|m| m.path == "src/components/chat")
            .expect("src/components/chat should be in results");
        // seed_project drops Composer.tsx + AttachmentChip.tsx into chat/.
        assert_eq!(chat.item_count, 2);
    }

    #[tokio::test]
    async fn list_project_folders_returns_empty_for_nonexistent_cwd() {
        let out = list_project_folders(
            "/this/path/does/not/exist/codemux-folders".into(),
            None,
            10,
        )
        .await
        .unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn list_project_folders_excludes_root_entry() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let out = list_project_folders(cwd, None, 50).await.unwrap();
        // The root ("") is filtered out — users don't pick the root
        // through the popup.
        assert!(out.iter().all(|m| !m.path.is_empty()));
    }

    fn seed_folder_tree(root: &Path) {
        // A small tree with predictable depth-1 and depth-2 entries.
        // Walker should pick up these and render in the right order
        // (dirs first, then files; alphabetical within group).
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::create_dir_all(root.join("c")).unwrap();
        fs::write(root.join("a/x.txt"), "x").unwrap();
        fs::write(root.join("a/b/deep.txt"), "deep").unwrap();
        fs::write(root.join("c/y.txt"), "y").unwrap();
        fs::write(root.join("z.txt"), "z").unwrap();
    }

    #[tokio::test]
    async fn read_folder_renders_unicode_tree() {
        let dir = tempdir().unwrap();
        seed_folder_tree(dir.path());
        let info = read_folder_for_attachment(
            dir.path().to_string_lossy().to_string(),
            None,
            2,
        )
        .await
        .unwrap();
        // Header is the directory name; descendants follow.
        let lines: Vec<&str> = info.tree.lines().collect();
        assert!(!lines.is_empty());
        // Each non-header line carries one of the box-drawing prefixes.
        for line in &lines[1..] {
            assert!(
                line.contains("├──") || line.contains("└──") || line.contains("│"),
                "tree line missing box-drawing prefix: {line}"
            );
        }
        // Top-level entries appear: a/, c/, z.txt.
        assert!(info.tree.contains("a/"));
        assert!(info.tree.contains("c/"));
        assert!(info.tree.contains("z.txt"));
        // Depth-2 entries appear because max_depth=2.
        assert!(info.tree.contains("x.txt"));
        assert!(info.tree.contains("y.txt"));
        assert!(info.tree.contains("b/"));
        // Depth-3 deep.txt should NOT appear (b/ is depth 2; deep.txt is depth 3).
        assert!(
            !info.tree.contains("deep.txt"),
            "depth-3 entry leaked into depth-2 tree"
        );
        assert!(info.item_count > 0);
    }

    #[tokio::test]
    async fn read_folder_respects_max_depth_one() {
        let dir = tempdir().unwrap();
        seed_folder_tree(dir.path());
        let info = read_folder_for_attachment(
            dir.path().to_string_lossy().to_string(),
            None,
            1,
        )
        .await
        .unwrap();
        // Only depth-1 entries: a/, c/, z.txt. Depth-2 children
        // x.txt, y.txt, b/ shouldn't appear.
        assert!(info.tree.contains("a/"));
        assert!(info.tree.contains("z.txt"));
        assert!(!info.tree.contains("x.txt"));
        assert!(!info.tree.contains("b/"));
    }

    #[tokio::test]
    async fn read_folder_truncates_at_100_entries() {
        let dir = tempdir().unwrap();
        for i in 0..150 {
            fs::write(dir.path().join(format!("file_{i:03}.txt")), "x").unwrap();
        }
        let info = read_folder_for_attachment(
            dir.path().to_string_lossy().to_string(),
            None,
            1,
        )
        .await
        .unwrap();
        assert_eq!(info.item_count, 150);
        assert!(!info.item_count_lower_bound);
        assert!(
            info.tree.contains("more entries"),
            "truncation marker missing: tree starts with {}",
            info.tree.lines().next().unwrap_or("")
        );
        // The "more" line wraps the truncated count.
        assert!(info.tree.contains("50 more"));
    }

    #[test]
    fn read_folder_bounds_walks_before_sorting_or_rendering() {
        let dir = tempdir().unwrap();
        for i in 0..20 {
            fs::write(dir.path().join(format!("file_{i:03}.txt")), "x").unwrap();
        }
        let info = read_folder_for_attachment_with_limit(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
            4,
        ).unwrap();
        assert!(info.item_count_lower_bound);
        assert!(info.item_count <= 4);
        assert!(info.tree.contains("additional entries may be omitted"));
    }

    #[test]
    fn ignored_files_still_consume_the_folder_visit_budget() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".gitignore"), "*.tmp\n").unwrap();
        for i in 0..20 {
            fs::write(dir.path().join(format!("ignored_{i:03}.tmp")), "x").unwrap();
        }
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_with_limit(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
            5,
        )
        .unwrap();
        assert!(info.item_count_lower_bound);
        assert!(info.item_count <= 5);
        assert!(!info.tree.contains("ignored_"));
        assert!(info.tree.contains("additional entries may be omitted"));
    }

    #[test]
    fn oversized_ignore_rules_are_rejected_before_parsing() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(
            dir.path().join(".gitignore"),
            vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1],
        )
        .unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let error = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("ignore rule file exceeds 1 MiB"));
    }

    #[test]
    fn excessive_ignore_patterns_are_rejected_before_compiling() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".ignore"),
            "x\n".repeat(MAX_ATTACHMENT_IGNORE_RULE_LINES + 1),
        )
        .unwrap();
        let error = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("exceed 10,000 lines"));
    }

    #[test]
    fn ignore_rule_line_budget_counts_only_present_lines() {
        let dir = tempdir().unwrap();
        let file = dir.path().join(".ignore");
        let rules = "# rule\n".repeat(MAX_ATTACHMENT_IGNORE_RULE_LINES);
        let mut lines = 0;
        FolderWalkFilter::compile_rule_bytes(dir.path(), &file, rules.as_bytes(), &mut lines)
            .unwrap();
        assert_eq!(lines, MAX_ATTACHMENT_IGNORE_RULE_LINES);

        let mut lines = 0;
        let too_many = "# rule\n".repeat(MAX_ATTACHMENT_IGNORE_RULE_LINES + 1);
        let error = FolderWalkFilter::compile_rule_bytes(
            dir.path(),
            &file,
            too_many.as_bytes(),
            &mut lines,
        )
        .err()
        .expect("10,001 lines must exceed the limit");
        assert!(error.contains("exceed 10,000 lines"));

        let mut lines = 0;
        FolderWalkFilter::compile_rule_bytes(dir.path(), &file, b"", &mut lines).unwrap();
        assert_eq!(lines, 0);
    }

    #[test]
    fn malformed_ignore_line_keeps_valid_exclusions() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let mut builder = GitignoreBuilder::new(dir.path());
        assert!(builder.add_line(None, "{broken").is_err());
        fs::write(dir.path().join(".gitignore"), b"{broken\n\xff\n*.secret\n").unwrap();
        fs::write(dir.path().join("hidden.secret"), "x").unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.secret"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_ignore_rules_are_rejected_before_parsing() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("rules"), "*.txt\n").unwrap();
        std::os::unix::fs::symlink("rules", dir.path().join(".gitignore")).unwrap();
        let error = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("ignore rules are not a regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn opened_rule_sources_cannot_block_on_fifo_or_follow_symlink() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempdir().unwrap();
        let fifo = dir.path().join("rules.fifo");
        let fifo_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        let reader = open_bounded_file(&fifo, false).unwrap();
        assert!(!reader.metadata().unwrap().is_file());
        let reader = open_bounded_file(&fifo, true).unwrap();
        assert!(!reader.metadata().unwrap().is_file());

        let fifo_link = dir.path().join("fifo.link");
        std::os::unix::fs::symlink(&fifo, &fifo_link).unwrap();
        let reader = open_bounded_file(&fifo_link, true).unwrap();
        assert!(!reader.metadata().unwrap().is_file());

        let target = dir.path().join("target");
        fs::write(&target, "*.secret\n").unwrap();
        let symlink = dir.path().join("rules.link");
        std::os::unix::fs::symlink(&target, &symlink).unwrap();
        assert!(open_bounded_file(&symlink, false).is_err());
    }

    #[test]
    fn worktree_git_file_does_not_block_folder_attachment() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".git"), "gitdir: /tmp/example\n").unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
    }

    #[test]
    fn oversized_worktree_git_file_is_rejected_before_parsing() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".git"),
            vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1],
        )
        .unwrap();
        let error = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("ignore rule file exceeds 1 MiB"));
    }

    #[test]
    fn linked_worktree_exclude_is_bounded_outside_attached_folder() {
        let dir = tempdir().unwrap();
        let worktree = dir.path().join("worktree");
        let git_dir = dir.path().join("gitdir");
        let common_dir = dir.path().join("common");
        fs::create_dir(&worktree).unwrap();
        fs::create_dir(&git_dir).unwrap();
        fs::create_dir_all(common_dir.join("info")).unwrap();
        fs::write(worktree.join(".git"), format!("gitdir: {}\n", git_dir.display())).unwrap();
        fs::write(git_dir.join("commondir"), format!("{}\n", common_dir.display())).unwrap();
        fs::write(
            common_dir.join("info/exclude"),
            vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1],
        )
        .unwrap();
        let error = read_folder_for_attachment_blocking(
            worktree.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("ignore rule file exceeds 1 MiB"));
    }

    #[test]
    fn relative_git_directory_exclude_is_applied() {
        let dir = tempdir().unwrap();
        let worktree = dir.path().join("worktree");
        let git_dir = dir.path().join("gitdir");
        fs::create_dir(&worktree).unwrap();
        fs::create_dir_all(git_dir.join("info")).unwrap();
        fs::write(worktree.join(".git"), "gitdir: ../gitdir\n").unwrap();
        fs::write(git_dir.join("info/exclude"), "*.secret\n").unwrap();
        fs::write(worktree.join("hidden.secret"), "x").unwrap();
        fs::write(worktree.join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            worktree.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.secret"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folder_checks_canonical_parent_rules() {
        let dir = tempdir().unwrap();
        let real_parent = dir.path().join("real");
        let attached = real_parent.join("attached");
        fs::create_dir_all(&attached).unwrap();
        fs::write(
            real_parent.join(".ignore"),
            vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1],
        )
        .unwrap();
        let link = dir.path().join("linked");
        std::os::unix::fs::symlink(&attached, &link).unwrap();
        let error = read_folder_for_attachment_blocking(
            link.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("ignore rule file exceeds 1 MiB"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folder_keeps_supplied_name_in_preview() {
        let dir = tempdir().unwrap();
        let attached = dir.path().join("real");
        fs::create_dir(&attached).unwrap();
        fs::write(attached.join("visible.txt"), "x").unwrap();
        let link = dir.path().join("linked");
        std::os::unix::fs::symlink(&attached, &link).unwrap();
        let info = read_folder_for_attachment_blocking(
            link.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert_eq!(info.tree.lines().next(), Some("linked"));
        assert!(info.tree.contains("visible.txt"));
    }

    #[test]
    fn ignore_matcher_uses_bounded_snapshot_after_file_changes() {
        let dir = tempdir().unwrap();
        let rules = dir.path().join(".ignore");
        fs::write(&rules, "ignored.txt\n").unwrap();
        let mut state = FolderWalkFilter {
            rules: HashMap::new(),
            global_rules: None,
            global_rules_checked: false,
            raw_visits: 0,
            exhausted: false,
            had_error: false,
            fatal_error: None,
            checked_rule_dirs: HashSet::new(),
            rule_bytes: 0,
            rule_lines: 0,
        };
        state.check_rule_files(dir.path(), true).unwrap();
        fs::write(&rules, vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1]).unwrap();
        assert!(state.is_ignored(&dir.path().join("ignored.txt"), false).unwrap());
        assert!(!state.is_ignored(&dir.path().join("visible.txt"), false).unwrap());
    }

    #[test]
    fn folder_preview_keeps_git_exclude_and_parent_ignore_rules() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(project.join(".git/info")).unwrap();
        fs::write(dir.path().join(".ignore"), "*.tmp\n").unwrap();
        fs::write(project.join(".git/info/exclude"), "*.secret\n").unwrap();
        fs::write(project.join("scratch.tmp"), "x").unwrap();
        fs::write(project.join("local.secret"), "x").unwrap();
        fs::write(project.join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            project.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("scratch.tmp"));
        assert!(!info.tree.contains("local.secret"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_git_directory_excludes_files_in_attached_subfolder() {
        let dir = tempdir().unwrap();
        let git_dir = dir.path().join("git-metadata");
        fs::create_dir_all(git_dir.join("info")).unwrap();
        fs::write(git_dir.join("info/exclude"), "*.secret\n").unwrap();
        let project = dir.path().join("project");
        let child = project.join("child");
        fs::create_dir_all(&child).unwrap();
        std::os::unix::fs::symlink(&git_dir, project.join(".git")).unwrap();
        fs::write(child.join("hidden.secret"), "x").unwrap();
        fs::write(child.join("visible.txt"), "x").unwrap();

        let picked = walk_project(&child);
        assert!(picked.paths.iter().any(|path| path == "visible.txt"));
        assert!(!picked.paths.iter().any(|path| path == "hidden.secret"));
        let info = read_folder_for_attachment_blocking(
            child.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.secret"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_worktree_metadata_excludes_files_in_attached_subfolder() {
        let dir = tempdir().unwrap();
        let common = dir.path().join("common-git");
        fs::create_dir_all(common.join("info")).unwrap();
        fs::write(common.join("info/exclude"), "*.secret\n").unwrap();
        let worktree_git = dir.path().join("worktree-git");
        fs::create_dir(&worktree_git).unwrap();
        let commondir_target = dir.path().join("commondir-target");
        fs::write(&commondir_target, common.to_string_lossy().as_bytes()).unwrap();
        std::os::unix::fs::symlink(&commondir_target, worktree_git.join("commondir")).unwrap();
        let pointer = dir.path().join("git-pointer");
        fs::write(&pointer, format!("gitdir: {}\n", worktree_git.display())).unwrap();

        let project = dir.path().join("project");
        let child = project.join("child");
        fs::create_dir_all(&child).unwrap();
        std::os::unix::fs::symlink(&pointer, project.join(".git")).unwrap();
        fs::write(child.join("hidden.secret"), "x").unwrap();
        fs::write(child.join("visible.txt"), "x").unwrap();

        let picked = walk_project(&child);
        assert!(picked.paths.iter().any(|path| path == "visible.txt"));
        assert!(!picked.paths.iter().any(|path| path == "hidden.secret"));
        let info = read_folder_for_attachment_blocking(
            child.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.secret"));
    }

    #[test]
    fn child_gitignore_negation_overrides_parent() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        let child = dir.path().join("child");
        fs::create_dir(&child).unwrap();
        fs::write(child.join(".gitignore"), "!keep.log\n").unwrap();
        fs::write(child.join("keep.log"), "x").unwrap();
        fs::write(child.join("hidden.log"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            2,
        )
        .unwrap();
        assert!(info.tree.contains("keep.log"));
        assert!(!info.tree.contains("hidden.log"));
    }

    #[test]
    fn gitignore_negation_overrides_git_exclude() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git/info")).unwrap();
        fs::write(dir.path().join(".git/info/exclude"), "*.log\n").unwrap();
        fs::write(dir.path().join(".gitignore"), "!keep.log\n").unwrap();
        fs::write(dir.path().join("keep.log"), "x").unwrap();
        fs::write(dir.path().join("hidden.log"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("keep.log"));
        assert!(!info.tree.contains("hidden.log"));
    }

    #[test]
    fn nested_git_repository_stops_parent_gitignore_rules() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        let nested = dir.path().join("nested");
        fs::create_dir_all(nested.join(".git")).unwrap();
        fs::write(nested.join("visible.log"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            2,
        )
        .unwrap();
        assert!(info.tree.contains("visible.log"));
    }

    #[test]
    fn attaching_nested_repository_skips_oversized_outer_gitignore() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(
            dir.path().join(".gitignore"),
            vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1],
        )
        .unwrap();
        fs::write(dir.path().join(".ignore"), "*.tmp\n").unwrap();
        let nested = dir.path().join("nested");
        let child = nested.join("child");
        fs::create_dir_all(nested.join(".git")).unwrap();
        fs::create_dir(&child).unwrap();
        fs::write(child.join("hidden.tmp"), "x").unwrap();
        fs::write(child.join("visible.txt"), "x").unwrap();

        let picked = walk_project(&child);
        assert!(picked.paths.iter().any(|path| path == "visible.txt"));
        assert!(!picked.paths.iter().any(|path| path == "hidden.tmp"));
        let info = read_folder_for_attachment_blocking(
            child.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.tmp"));
    }

    #[test]
    fn jujutsu_workspace_applies_gitignore_rules() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".jj")).unwrap();
        fs::write(dir.path().join(".gitignore"), "*.secret\n").unwrap();
        fs::write(dir.path().join("hidden.secret"), "x").unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.secret"));
    }

    #[test]
    fn oversized_global_git_config_is_rejected_before_matching() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("global-git-config");
        fs::write(&config, vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1]).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::project_files::tests::global_config_preflight_child",
                "--nocapture",
            ])
            .env("CODEMUX_ATTACHMENT_GLOBAL_CONFIG_CHILD", "1")
            .env("GIT_CONFIG_GLOBAL", config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn global_config_preflight_child() {
        if std::env::var_os("CODEMUX_ATTACHMENT_GLOBAL_CONFIG_CHILD").is_none() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let error = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("ignore rule file exceeds 1 MiB"));
    }

    #[test]
    fn oversized_global_exclude_is_loaded_only_for_visited_repositories() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("global-git-config");
        let global_ignore = dir.path().join("global-ignore");
        let system_config = dir.path().join("system-git-config");
        let xdg = dir.path().join("xdg");
        fs::create_dir(&xdg).unwrap();
        fs::write(&global_ignore, vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1])
            .unwrap();
        fs::write(
            &config,
            format!("[core]\nexcludesFile = {}\n", global_ignore.display()),
        )
        .unwrap();
        fs::write(&system_config, "").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::project_files::tests::global_exclude_scope_child",
                "--nocapture",
            ])
            .env("CODEMUX_ATTACHMENT_GLOBAL_EXCLUDE_SCOPE_CHILD", "1")
            .env("GIT_CONFIG_GLOBAL", config)
            .env("GIT_CONFIG_SYSTEM", system_config)
            .env("XDG_CONFIG_HOME", xdg)
            .env("HOME", dir.path())
            .env("USERPROFILE", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn global_exclude_scope_child() {
        if std::env::var_os("CODEMUX_ATTACHMENT_GLOBAL_EXCLUDE_SCOPE_CHILD").is_none() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let standalone = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            2,
        )
        .unwrap();
        assert!(standalone.tree.contains("visible.txt"));

        let nested = dir.path().join("nested");
        fs::create_dir_all(nested.join(".git")).unwrap();
        fs::write(nested.join("repo.txt"), "x").unwrap();
        let error = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            2,
        )
        .unwrap_err();
        assert!(error.contains("ignore rule file exceeds 1 MiB"));

        let empty = dir.path().join("empty");
        fs::create_dir_all(empty.join(".git")).unwrap();
        let info = read_folder_for_attachment_blocking(
            empty.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert_eq!(info.item_count, 0);
        fs::write(empty.join("entry.txt"), "x").unwrap();
        let error = read_folder_for_attachment_blocking(
            empty.to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("ignore rule file exceeds 1 MiB"));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_optional_git_config_uses_picker_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let xdg = dir.path().join("xdg");
        fs::create_dir_all(xdg.join("git")).unwrap();
        fs::write(xdg.join("git/ignore"), "*.secret\n").unwrap();
        let system_config = dir.path().join("unreadable-system-config");
        fs::write(&system_config, "[core]\n").unwrap();
        fs::set_permissions(&system_config, fs::Permissions::from_mode(0o000)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::project_files::tests::unreadable_config_fallback_child",
                "--nocapture",
            ])
            .env("CODEMUX_ATTACHMENT_UNREADABLE_CONFIG_CHILD", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", system_config)
            .env("XDG_CONFIG_HOME", xdg)
            .env("HOME", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_config_fallback_child() {
        if std::env::var_os("CODEMUX_ATTACHMENT_UNREADABLE_CONFIG_CHILD").is_none() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("hidden.secret"), "x").unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let picked = walk_project(dir.path());
        assert!(picked.paths.iter().any(|path| path == "visible.txt"));
        assert!(!picked.paths.iter().any(|path| path == "hidden.secret"));
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.secret"));
    }

    #[test]
    fn non_utf8_global_config_comment_keeps_valid_ignore_rules() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("global-git-config");
        let global_ignore = dir.path().join("global-ignore");
        fs::write(&global_ignore, "*.secret\n").unwrap();
        let mut config_bytes = b"[core]\n# unrelated ".to_vec();
        config_bytes.extend_from_slice(&[0xff, b'\n']);
        config_bytes.extend_from_slice(format!("excludesFile = {}\n", global_ignore.display()).as_bytes());
        fs::write(&config, config_bytes).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::project_files::tests::configured_global_ignore_child",
                "--nocapture",
            ])
            .env("CODEMUX_ATTACHMENT_CONFIGURED_IGNORE_CHILD", "1")
            .env("GIT_CONFIG_GLOBAL", config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn configured_global_ignore_child() {
        if std::env::var_os("CODEMUX_ATTACHMENT_CONFIGURED_IGNORE_CHILD").is_none() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("local.secret"), "x").unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("local.secret"));
    }

    #[test]
    fn invalid_global_excludes_values_use_picker_fallback() {
        let dir = tempdir().unwrap();
        let xdg = dir.path().join("xdg");
        fs::create_dir_all(xdg.join("git")).unwrap();
        fs::write(xdg.join("git/ignore"), "*.secret\n").unwrap();
        let system_config = dir.path().join("system-git-config");
        fs::write(&system_config, "").unwrap();

        let mut undecodable = b"[core]\nexcludesFile = /invalid/".to_vec();
        undecodable.extend_from_slice(&[0xff, b'\n']);
        for (name, contents) in [
            ("inline-comment", b"[core]\nexcludesFile = /invalid/ignore # comment\n".to_vec()),
            ("undecodable", undecodable),
        ] {
            let config = dir.path().join(name);
            fs::write(&config, contents).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "commands::project_files::tests::invalid_global_excludes_fallback_child",
                    "--nocapture",
                ])
                .env("CODEMUX_ATTACHMENT_INVALID_GLOBAL_CHILD", "1")
                .env("GIT_CONFIG_GLOBAL", config)
                .env("GIT_CONFIG_SYSTEM", &system_config)
                .env("XDG_CONFIG_HOME", &xdg)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{name}: stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn invalid_global_excludes_fallback_child() {
        if std::env::var_os("CODEMUX_ATTACHMENT_INVALID_GLOBAL_CHILD").is_none() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("hidden.secret"), "x").unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let picked = walk_project(dir.path());
        assert!(picked.paths.iter().any(|path| path == "visible.txt"));
        assert!(!picked.paths.iter().any(|path| path == "hidden.secret"));
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.secret"));
    }

    #[test]
    fn empty_global_override_falls_through_to_xdg_config() {
        let dir = tempdir().unwrap();
        let override_config = dir.path().join("empty-override");
        fs::write(&override_config, "").unwrap();
        let xdg = dir.path().join("xdg");
        fs::create_dir_all(xdg.join("git")).unwrap();
        let selected_ignore = dir.path().join("selected-ignore");
        fs::write(&selected_ignore, "*.secret\n").unwrap();
        fs::write(
            xdg.join("git/config"),
            format!("[core]\nexcludesFile = {}\n", selected_ignore.display()),
        )
        .unwrap();
        fs::write(xdg.join("git/ignore"), "*.default\n").unwrap();
        let system_config = dir.path().join("system-git-config");
        fs::write(&system_config, "").unwrap();

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::project_files::tests::empty_global_override_fallback_child",
                "--nocapture",
            ])
            .env("CODEMUX_ATTACHMENT_EMPTY_OVERRIDE_CHILD", "1")
            .env("GIT_CONFIG_GLOBAL", override_config)
            .env("GIT_CONFIG_SYSTEM", system_config)
            .env("XDG_CONFIG_HOME", xdg)
            .env("HOME", dir.path())
            .env("USERPROFILE", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn empty_global_override_fallback_child() {
        if std::env::var_os("CODEMUX_ATTACHMENT_EMPTY_OVERRIDE_CHILD").is_none() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("hidden.secret"), "x").unwrap();
        fs::write(dir.path().join("visible.default"), "x").unwrap();
        let picked = walk_project(dir.path());
        assert!(!picked.paths.iter().any(|path| path == "hidden.secret"));
        assert!(picked.paths.iter().any(|path| path == "visible.default"));
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(!info.tree.contains("hidden.secret"));
        assert!(info.tree.contains("visible.default"));
    }

    #[test]
    fn explicit_global_ignore_replaces_xdg_fallback() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("global-git-config");
        let global_ignore = dir.path().join("global-ignore");
        let system_config = dir.path().join("system-git-config");
        let xdg = dir.path().join("xdg");
        fs::create_dir_all(xdg.join("git")).unwrap();
        fs::write(&global_ignore, "*.secret\n").unwrap();
        fs::write(&system_config, vec![b'x'; MAX_ATTACHMENT_IGNORE_FILE_BYTES as usize + 1])
            .unwrap();
        fs::write(xdg.join("git/ignore"), "*.txt\n").unwrap();
        fs::write(
            &config,
            format!("[core]\nexcludesFile = {}\n", global_ignore.display()),
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::project_files::tests::explicit_global_ignore_child",
                "--nocapture",
            ])
            .env("CODEMUX_ATTACHMENT_EXPLICIT_IGNORE_CHILD", "1")
            .env("GIT_CONFIG_GLOBAL", config)
            .env("GIT_CONFIG_SYSTEM", system_config)
            .env("XDG_CONFIG_HOME", xdg)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn explicit_global_ignore_child() {
        if std::env::var_os("CODEMUX_ATTACHMENT_EXPLICIT_IGNORE_CHILD").is_none() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("hidden.secret"), "x").unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
        assert!(!info.tree.contains("hidden.secret"));
    }

    #[cfg(unix)]
    #[test]
    fn null_global_git_config_allows_folder_preview() {
        let dir = tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::project_files::tests::null_global_config_child",
                "--nocapture",
            ])
            .env("CODEMUX_ATTACHMENT_NULL_CONFIG_CHILD", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("XDG_CONFIG_HOME", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn null_global_config_child() {
        if std::env::var_os("CODEMUX_ATTACHMENT_NULL_CONFIG_CHILD").is_none() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
        )
        .unwrap();
        assert!(info.tree.contains("visible.txt"));
    }

    #[test]
    fn bounded_folder_with_no_scanned_entries_is_not_reported_empty() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("file.txt"), "x").unwrap();
        let info = read_folder_for_attachment_with_limit(
            dir.path().to_string_lossy().into_owned(),
            None,
            1,
            0,
        )
        .unwrap();
        assert_eq!(info.item_count, 0);
        assert!(info.item_count_lower_bound);
        assert!(info.tree.contains("additional entries may be omitted"));
    }

    #[test]
    fn read_folder_clamps_untrusted_depth() {
        let dir = tempdir().unwrap();
        let mut nested = dir.path().to_path_buf();
        for depth in 1..=FOLDER_WALK_MAX_DEPTH + 1 {
            nested.push(format!("level_{depth}"));
            fs::create_dir(&nested).unwrap();
        }
        let info = read_folder_for_attachment_blocking(
            dir.path().to_string_lossy().into_owned(),
            None,
            usize::MAX,
        )
        .unwrap();
        assert_eq!(info.item_count, FOLDER_WALK_MAX_DEPTH);
        assert!(!info.tree.contains(&format!("level_{}", FOLDER_WALK_MAX_DEPTH + 1)));
    }

    #[tokio::test]
    async fn read_folder_empty_returns_empty_marker() {
        let dir = tempdir().unwrap();
        let info = read_folder_for_attachment(
            dir.path().to_string_lossy().to_string(),
            None,
            2,
        )
        .await
        .unwrap();
        assert_eq!(info.item_count, 0);
        assert!(info.tree.contains("(empty)"));
    }

    #[tokio::test]
    async fn read_folder_errors_on_non_directory() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("notadir.txt");
        fs::write(&f, "x").unwrap();
        let err = read_folder_for_attachment(
            f.to_string_lossy().to_string(),
            None,
            2,
        )
        .await
        .unwrap_err();
        assert!(err.contains("not a directory"));
    }

    #[tokio::test]
    async fn read_folder_returns_relative_path_when_under_cwd() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();
        let info = read_folder_for_attachment(
            nested.to_string_lossy().to_string(),
            Some(dir.path().to_string_lossy().to_string()),
            2,
        )
        .await
        .unwrap();
        assert_eq!(info.relative_path.as_deref(), Some("a/b"));
    }

    #[tokio::test]
    async fn read_folder_respects_gitignore() {
        let dir = tempdir().unwrap();
        // The `ignore` crate activates .gitignore parsing only when a
        // .git directory exists in the tree (otherwise .gitignore is
        // dormant). Mirroring `seed_project`, create a stub .git so
        // the walker recognises this tempdir as a git repo.
        fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join("node_modules/foo.js"), "x").unwrap();
        fs::write(dir.path().join("real.txt"), "x").unwrap();
        fs::write(dir.path().join(".gitignore"), "node_modules\n").unwrap();
        let info = read_folder_for_attachment(
            dir.path().to_string_lossy().to_string(),
            None,
            2,
        )
        .await
        .unwrap();
        assert!(!info.tree.contains("node_modules"));
        assert!(info.tree.contains("real.txt"));
    }

    #[tokio::test]
    async fn absolute_path_is_returned_alongside_relative() {
        let dir = tempdir().unwrap();
        seed_project(dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let results = list_project_files(cwd.clone(), Some("README".into()), 5)
            .await
            .unwrap();
        let readme = results
            .iter()
            .find(|m| m.path == "README.md")
            .expect("README.md should match");
        assert!(
            readme.absolute_path.ends_with("README.md"),
            "absolute path missing filename: {}",
            readme.absolute_path
        );
        // canonical-cwd prefix
        let canonical = Path::new(&cwd).canonicalize().unwrap();
        assert!(
            readme
                .absolute_path
                .starts_with(canonical.to_string_lossy().as_ref()),
            "absolute path doesn't start with canonical cwd: {} vs {}",
            readme.absolute_path,
            canonical.display()
        );
    }
}
