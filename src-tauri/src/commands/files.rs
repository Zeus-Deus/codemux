use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
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
        if ignored.contains(&name) {
            continue;
        }
        if is_dir && SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        entries.push(FileEntry {
            name,
            path: entry_path.to_string_lossy().to_string(),
            is_dir,
            size,
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

    let output = Command::new("git")
        .args(["check-ignore", "--stdin"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
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
    let output = Command::new("rg")
        .args([
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
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("rg not found: {e}"))?;

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
    let output = Command::new("grep")
        .args(["-rn", "--include=*", "-i", query, path])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("grep failed: {e}"))?;

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
    let output = Command::new("fd")
        .args([
            "--type",
            "f",
            "--max-results",
            &limit.to_string(),
            query,
            path,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("fd not found: {e}"))?;

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
    if Command::new("which")
        .arg("xdg-open")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        == false
    {
        return Err("xdg-open not found — cannot open file manager. Install xdg-utils.".to_string());
    }
    Command::new("xdg-open")
        .arg(&path)
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
    let output = Command::new("find")
        .args([
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
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("find failed: {e}"))?;

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
