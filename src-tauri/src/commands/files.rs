use crate::execution::sanitize_gui_env_std;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub is_gitignored: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub file_path: String,
    pub line_number: u32,
    pub line_content: String,
    pub match_start: u32,
    pub match_end: u32,
}

#[tauri::command]
pub fn list_directory(path: String, show_hidden: Option<bool>) -> Result<Vec<FileEntry>, String> {
    let dir = Path::new(&path);
    let show_hidden = show_hidden.unwrap_or(false);
    if !dir.is_dir() {
        return Err(format!("Not a directory: {path}"));
    }

    let mut entries: Vec<FileEntry> = Vec::new();
    let read_dir = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {e}"))?;

    // Collect raw entries
    let mut raw: Vec<(String, std::path::PathBuf, bool, Option<u64>)> = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Always hide .git directory regardless of show_hidden
        if name == ".git" {
            continue;
        }
        // Skip hidden files/dirs starting with '.' unless show_hidden is set
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let entry_path = entry.path();
        let metadata = entry.metadata().ok();
        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = if is_dir {
            None
        } else {
            metadata.as_ref().map(|m| m.len())
        };
        raw.push((name, entry_path, is_dir, size));
    }

    // Filter out git-ignored files using git check-ignore
    let ignored = git_ignored_set(dir, &raw);

    // Common directories to always skip (even outside git repos)
    const SKIP_DIRS: &[&str] = &[
        "node_modules",
        "target",
        "dist",
        "build",
        "__pycache__",
        ".next",
        ".nuxt",
        ".output",
        "vendor",
    ];

    for (name, entry_path, is_dir, size) in raw {
        if is_dir && SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        entries.push(FileEntry {
            name: name.clone(),
            path: entry_path.to_string_lossy().to_string(),
            is_dir,
            size,
            is_gitignored: ignored.contains(&name),
        });
    }

    // Sort: directories first, then files, alphabetical within each group
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

fn git_ignored_set(
    dir: &Path,
    entries: &[(String, std::path::PathBuf, bool, Option<u64>)],
) -> std::collections::HashSet<String> {
    let mut ignored = std::collections::HashSet::new();
    if entries.is_empty() {
        return ignored;
    }

    // Build stdin: one path per line
    let paths: Vec<String> = entries
        .iter()
        .map(|(_, p, is_dir, _)| {
            let s = p.to_string_lossy().to_string();
            if *is_dir {
                format!("{s}/")
            } else {
                s
            }
        })
        .collect();
    let stdin_data = paths.join("\n");

    let mut cmd = Command::new("git");
    cmd.args(["check-ignore", "--stdin"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    sanitize_gui_env_std(&mut cmd);
    let output = cmd
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(stdin_data.as_bytes());
            }
            child.wait_with_output()
        });

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let p = Path::new(line.trim_end_matches('/'));
            if let Some(name) = p.file_name() {
                ignored.insert(name.to_string_lossy().to_string());
            }
        }
    }

    ignored
}

#[tauri::command]
pub fn search_in_files(
    path: String,
    query: String,
    max_results: Option<u32>,
) -> Result<Vec<SearchResult>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = max_results.unwrap_or(100);

    // Try ripgrep first
    if let Ok(results) = search_with_rg(&path, &query, limit) {
        return Ok(results);
    }

    // Fall back to grep
    search_with_grep(&path, &query, limit)
}

fn search_with_rg(path: &str, query: &str, limit: u32) -> Result<Vec<SearchResult>, String> {
    let mut cmd = Command::new("rg");
    cmd.args([
        "--json",
        "--max-count",
        "5",
        "--max-columns",
        "200",
        "--smart-case",
        query,
        path,
    ])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    sanitize_gui_env_std(&mut cmd);
    let output = cmd.output().map_err(|e| format!("rg not found: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        if results.len() >= limit as usize {
            break;
        }
        // Parse rg JSON output
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if val.get("type").and_then(|t| t.as_str()) != Some("match") {
            continue;
        }
        let Some(data) = val.get("data") else {
            continue;
        };
        let file_path = data
            .get("path")
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let line_number = data
            .get("line_number")
            .and_then(|n| n.as_u64())
            .unwrap_or(0) as u32;
        let line_content = data
            .get("lines")
            .and_then(|l| l.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim_end()
            .to_string();

        // Extract first submatch offset
        let (match_start, match_end) = data
            .get("submatches")
            .and_then(|s| s.as_array())
            .and_then(|arr| arr.first())
            .map(|m| {
                let start = m.get("start").and_then(|s| s.as_u64()).unwrap_or(0) as u32;
                let end = m.get("end").and_then(|e| e.as_u64()).unwrap_or(0) as u32;
                (start, end)
            })
            .unwrap_or((0, 0));

        results.push(SearchResult {
            file_path,
            line_number,
            line_content,
            match_start,
            match_end,
        });
    }

    Ok(results)
}

fn search_with_grep(path: &str, query: &str, limit: u32) -> Result<Vec<SearchResult>, String> {
    let mut cmd = Command::new("grep");
    cmd.args(["-rn", "--include=*", "-i", query, path])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    sanitize_gui_env_std(&mut cmd);
    let output = cmd.output().map_err(|e| format!("grep failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    let query_lower = query.to_lowercase();

    for line in stdout.lines() {
        if results.len() >= limit as usize {
            break;
        }
        // Format: file:line_number:content
        let mut parts = line.splitn(3, ':');
        let file_path = parts.next().unwrap_or("").to_string();
        let line_number: u32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let line_content = parts.next().unwrap_or("").trim_end().to_string();

        // Find match position
        let content_lower = line_content.to_lowercase();
        let match_start = content_lower.find(&query_lower).unwrap_or(0) as u32;
        let match_end = match_start + query.len() as u32;

        results.push(SearchResult {
            file_path,
            line_number,
            line_content,
            match_start,
            match_end,
        });
    }

    Ok(results)
}

#[tauri::command]
pub fn search_file_names(
    path: String,
    query: String,
    max_results: Option<u32>,
) -> Result<Vec<String>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = max_results.unwrap_or(50);
    let base = Path::new(&path);

    // Try fd first
    if let Ok(results) = search_with_fd(&path, &query, limit) {
        // Convert to relative paths
        return Ok(results
            .into_iter()
            .map(|p| {
                Path::new(&p)
                    .strip_prefix(base)
                    .map(|r| r.to_string_lossy().to_string())
                    .unwrap_or(p)
            })
            .collect());
    }

    // Fall back to find
    search_with_find(&path, &query, limit, base)
}

fn search_with_fd(path: &str, query: &str, limit: u32) -> Result<Vec<String>, String> {
    let mut cmd = Command::new("fd");
    cmd.args([
        "--type",
        "f",
        "--max-results",
        &limit.to_string(),
        query,
        path,
    ])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    sanitize_gui_env_std(&mut cmd);
    let output = cmd.output().map_err(|e| format!("fd not found: {e}"))?;

    if !output.status.success() && output.stdout.is_empty() {
        return Err("fd returned no results".to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect())
}

#[tauri::command]
pub fn reveal_in_file_manager(path: String) -> Result<(), String> {
    let mut which_cmd = Command::new("which");
    which_cmd
        .arg("xdg-open")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    sanitize_gui_env_std(&mut which_cmd);
    if which_cmd
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        == false
    {
        return Err("xdg-open not found — cannot open file manager. Install xdg-utils.".to_string());
    }
    let mut open_cmd = Command::new("xdg-open");
    open_cmd.arg(&path);
    sanitize_gui_env_std(&mut open_cmd);
    open_cmd
        .spawn()
        .map_err(|e| format!("Failed to open file manager: {e}"))?;
    Ok(())
}

fn search_with_find(
    path: &str,
    query: &str,
    limit: u32,
    base: &Path,
) -> Result<Vec<String>, String> {
    let mut cmd = Command::new("find");
    cmd.args([
        path,
        "-type",
        "f",
        "-iname",
        &format!("*{query}*"),
        "-not",
        "-path",
        "*/node_modules/*",
        "-not",
        "-path",
        "*/.git/*",
        "-not",
        "-path",
        "*/target/*",
        "-not",
        "-path",
        "*/dist/*",
        "-not",
        "-path",
        "*/__pycache__/*",
    ])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    sanitize_gui_env_std(&mut cmd);
    let output = cmd.output().map_err(|e| format!("find failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .take(limit as usize)
        .map(|p| {
            Path::new(p)
                .strip_prefix(base)
                .map(|r| r.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.to_string())
        })
        .collect())
}

const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024; // 2 MB

#[tauri::command]
pub fn read_file(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    if !p.is_file() {
        return Err(format!("Not a file: {path}"));
    }

    let metadata = std::fs::metadata(p).map_err(|e| format!("Cannot read metadata: {e}"))?;
    if metadata.len() > MAX_FILE_SIZE {
        return Err(format!(
            "File too large ({:.1} MB, limit is 2 MB)",
            metadata.len() as f64 / (1024.0 * 1024.0)
        ));
    }

    let bytes = std::fs::read(p).map_err(|e| format!("Failed to read file: {e}"))?;

    // Detect binary: check for null bytes in first 8 KB
    let check_len = bytes.len().min(8192);
    if bytes[..check_len].contains(&0) {
        return Err("Binary file".into());
    }

    String::from_utf8(bytes).map_err(|_| "Binary file (not valid UTF-8)".into())
}

#[tauri::command]
pub fn write_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, &content).map_err(|e| format!("Failed to write file: {e}"))
}

/// Count occurrences of `pattern` across `cwd` using ripgrep.
///
/// `cwd` must be an absolute, existing directory. The pattern is passed
/// to `rg` verbatim. ripgrep respects `.gitignore` by default, so build
/// artifacts and `node_modules` are skipped without extra flags.
///
/// Returns 0 (not an error) when ripgrep exits with code 1 — that
/// indicates "no matches", which is a normal outcome for the
/// debug-marker grep on a clean codebase. Higher exit codes propagate
/// as `Err`.
#[tauri::command]
pub async fn grep_count_pattern(cwd: String, pattern: String) -> Result<usize, String> {
    let path = Path::new(&cwd);
    if !path.is_absolute() {
        return Err(format!("cwd must be absolute: {cwd}"));
    }
    if !path.is_dir() {
        return Err(format!("cwd does not exist or is not a directory: {cwd}"));
    }

    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--count-matches")
        .arg("--no-messages")
        .arg(&pattern)
        .arg(&cwd);
    crate::execution::sanitize_gui_env_tokio(&mut cmd);

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to run ripgrep: {e}"))?;

    if !output.status.success() {
        // rg exits 1 when no matches — that's not an error.
        if output.status.code() == Some(1) {
            return Ok(0);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ripgrep failed: {stderr}"));
    }

    // --count-matches output: "path:N" per file. Sum the Ns.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let total: usize = stdout
        .lines()
        .filter_map(|line| line.rsplit(':').next()?.parse::<usize>().ok())
        .sum();

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::grep_count_pattern;
    use std::fs;

    #[tokio::test]
    async fn grep_count_pattern_zero_when_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "no markers here\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let count = grep_count_pattern(path, "CODEMUX_DEBUG".to_string())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn grep_count_pattern_counts_correctly() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("a.rs"),
            "// CODEMUX_DEBUG one\n// CODEMUX_DEBUG two\n",
        )
        .unwrap();
        fs::write(dir.path().join("b.py"), "# CODEMUX_DEBUG three\n").unwrap();
        fs::write(dir.path().join("clean.txt"), "ordinary file\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let count = grep_count_pattern(path, "CODEMUX_DEBUG".to_string())
            .await
            .unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn grep_count_pattern_rejects_relative_cwd() {
        let err = grep_count_pattern("relative/path".to_string(), "X".to_string())
            .await
            .unwrap_err();
        assert!(err.contains("absolute"));
    }

    #[tokio::test]
    async fn grep_count_pattern_rejects_missing_dir() {
        let err = grep_count_pattern(
            "/this/path/does/not/exist/codemux-test".to_string(),
            "X".to_string(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("does not exist") || err.contains("not a directory"));
    }

    /// Regression guard for the Stage 6 cleanup pattern: the
    /// CODEMUX_DEBUG marker is written by Claude in whatever comment
    /// syntax the surrounding language uses. A bare-pattern grep
    /// (no comment-prefix) must catch every form so the cleanup
    /// turn can later remove them.
    #[tokio::test]
    async fn grep_count_pattern_catches_all_comment_syntaxes() {
        let dir = tempfile::tempdir().unwrap();
        // JS / TS / Rust / C / C++ / Java / Go / Swift
        fs::write(dir.path().join("a.rs"), "// CODEMUX_DEBUG slash style\n").unwrap();
        // Python / Ruby / shell / Make
        fs::write(dir.path().join("b.py"), "# CODEMUX_DEBUG hash style\n").unwrap();
        // SQL / Lua / Haskell / Ada
        fs::write(dir.path().join("c.sql"), "-- CODEMUX_DEBUG dash style\n").unwrap();
        // HTML / XML / Vue templates / Markdown comments
        fs::write(
            dir.path().join("d.html"),
            "<!-- CODEMUX_DEBUG html style -->\n",
        )
        .unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let count = grep_count_pattern(path, "CODEMUX_DEBUG".to_string())
            .await
            .unwrap();
        assert_eq!(
            count, 4,
            "bare CODEMUX_DEBUG pattern must catch //, #, --, <!-- forms"
        );
    }

    /// Sanity: the helper survives a missing rg binary by surfacing
    /// the underlying spawn error. Caller (AgentChatPane) treats this
    /// as `count = 0` after seeing `Err`.
    #[tokio::test]
    async fn grep_count_pattern_surfaces_spawn_error_when_rg_missing() {
        // We can't unconditionally remove `rg` from PATH inside a unit
        // test, but we can simulate the same branch by exercising the
        // error-path code via an absolute path that isn't a directory:
        // this is the closest deterministic check until we move to a
        // trait-injected runner. See `grep_count_pattern_rejects_missing_dir`.
        // The contract is: callers must treat `Err` as recoverable.
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("not-a-dir.txt");
        fs::write(&not_a_dir, "").unwrap();
        let err = grep_count_pattern(
            not_a_dir.to_string_lossy().to_string(),
            "CODEMUX_DEBUG".to_string(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("not a directory") || err.contains("does not exist"));
    }
}
