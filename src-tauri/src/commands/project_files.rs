//! `list_project_files` — fuzzy filename search for the chat composer's
//! `@` mention popup and `+ → File…` picker (Step 8 Stage 1).
//!
//! Audit decision (Step 8 research): `search_project_index` is a
//! lexical *content* index (chunks files into 40-line blocks, scored
//! by term-count). `search_file_names` is filename-only but lacks
//! fuzzy ranking, scoring, and caching. This command lives alongside
//! both — purpose-built for fast filename autocomplete with fuzzy
//! ranking + 60s in-memory cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Step 8 Stage 2 — content tier thresholds. Mirrors the locked
/// research decision: full inline for files ≤200KB AND ≤1500 lines;
/// first-50 + outline for anything larger.
const FULL_CONTENT_BYTES_LIMIT: u64 = 200 * 1024;
const FULL_CONTENT_LINE_LIMIT: usize = 1500;
const TRUNCATED_PREVIEW_LINES: usize = 50;
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

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

struct CacheEntry {
    /// Relative paths (UTF-8 lossy). Pre-sorted alphabetically for the
    /// empty-query path.
    paths: Vec<String>,
    /// Parallel array of absolute paths, same indices as `paths`.
    absolute: Vec<String>,
    scanned_at: Instant,
}

static CACHE: LazyLock<Mutex<HashMap<PathBuf, CacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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

    // The walker holds the lock briefly to read or repopulate the
    // cache. The fuzzy match runs on the cloned inner Vecs so other
    // callers don't block on I/O for the rare cache-miss path.
    let (paths, absolute) = {
        let mut cache = CACHE.lock().unwrap();
        let needs_refresh = cache
            .get(&canonical_cwd)
            .map(|e| e.scanned_at.elapsed() >= CACHE_TTL)
            .unwrap_or(true);
        if needs_refresh {
            let entry = walk_project(&canonical_cwd);
            cache.insert(canonical_cwd.clone(), entry);
        }
        let entry = cache.get(&canonical_cwd).unwrap();
        (entry.paths.clone(), entry.absolute.clone())
    };

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

    // Pre-build a path → index map so result lookup is O(1) instead
    // of an O(N²) scan when the matcher returns many candidates.
    let path_to_idx: HashMap<&str, usize> = paths
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();

    // `match_list` returns items already sorted by score (highest
    // first) per nucleo-matcher's documented contract.
    let scored = pattern.match_list(paths.iter().map(String::as_str), &mut matcher);

    let results: Vec<FileMatch> = scored
        .into_iter()
        .take(limit)
        .filter_map(|(matched_str, score)| {
            path_to_idx.get(matched_str).map(|&idx| FileMatch {
                path: paths[idx].clone(),
                absolute_path: absolute[idx].clone(),
                score,
            })
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
        let rel = abs
            .strip_prefix(cwd)
            .unwrap_or(&abs)
            .to_string_lossy()
            .to_string();
        // Skip the `.git` directory contents — `ignore` already handles
        // top-level `.git`, but be defensive for nested git repos.
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
    pub bytes: u64,
    pub language: Option<String>,
    pub is_text: bool,
    pub content: String,
    pub truncated: bool,
    pub outline: Option<Vec<OutlineEntry>>,
}

/// Read a file for attachment. Full content for small files
/// (≤200KB AND ≤1500 lines); first-50 + outline for larger files.
/// Binary files return `is_text: false` with empty content. Errors
/// only on actual I/O failures (missing path, permission denied) —
/// not on "file too large" or "binary" since both are normal cases
/// the chip needs to render.
#[tauri::command]
pub async fn read_file_for_attachment(
    absolute_path: String,
    cwd: Option<String>,
) -> Result<FileAttachmentInfo, String> {
    let path = Path::new(&absolute_path);
    if !path.is_file() {
        return Err(format!("not a file: {absolute_path}"));
    }

    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("metadata read failed for {absolute_path}: {e}"))?;
    let bytes = metadata.len();

    let raw = std::fs::read(path)
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
                    .map(|rel| rel.to_string_lossy().to_string())
            })
        });

    if is_binary(&raw) {
        return Ok(FileAttachmentInfo {
            absolute_path,
            relative_path,
            line_count: 0,
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

    let fits_inline =
        bytes <= FULL_CONTENT_BYTES_LIMIT && line_count <= FULL_CONTENT_LINE_LIMIT;

    if fits_inline {
        return Ok(FileAttachmentInfo {
            absolute_path,
            relative_path,
            line_count,
            bytes,
            language: language.clone(),
            is_text: true,
            content,
            truncated: false,
            outline: None,
        });
    }

    // Truncated path — preview + outline.
    let preview: String = content
        .lines()
        .take(TRUNCATED_PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let outline = extract_outline(&content, language.as_deref());

    Ok(FileAttachmentInfo {
        absolute_path,
        relative_path,
        line_count,
        bytes,
        language,
        is_text: true,
        content: preview,
        truncated: true,
        outline: Some(outline),
    })
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
    // Pre-compute byte-offset → line-number map by walking the content
    // once. Outline regex matches give us byte offsets via `caps.get(0)
    // .start()`; converting through this map keeps line lookup O(1)
    // per match instead of re-scanning the prefix per match.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            content
                .char_indices()
                .filter_map(|(i, c)| if c == '\n' { Some(i + 1) } else { None }),
        )
        .collect();

    let line_for_offset = |offset: usize| -> usize {
        match line_starts.binary_search(&offset) {
            Ok(idx) => idx + 1,
            Err(idx) => idx, // first index strictly greater → that's our 1-indexed line
        }
    };

    let mut out = Vec::new();
    for caps in spec.re.captures_iter(content) {
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
        let name = name_match.as_str().trim().to_string();
        if name.is_empty() {
            continue;
        }
        let offset = caps.get(0).map(|m| m.start()).unwrap_or(0);
        out.push(OutlineEntry {
            kind,
            name,
            line: line_for_offset(offset),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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

    fn write_lines(path: &Path, n: usize) {
        let content = (1..=n)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, content).unwrap();
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
        assert!(info.line_count >= 1501);
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
        fs::write(&p, b"hello\x00world\x00\x00").unwrap();
        let info =
            read_file_for_attachment(p.to_string_lossy().to_string(), None)
                .await
                .unwrap();
        assert!(!info.is_text);
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

    /// Variable len(write_lines) helper isn't used by these tests but
    /// is kept available for future fixtures (truncation by line count
    /// vs. bytes is already covered above).
    #[allow(dead_code)]
    fn _unused_write_lines_marker(p: &Path, n: usize) {
        write_lines(p, n);
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
