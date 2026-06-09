use serde::Serialize;
use std::path::Path;
use std::process::Command;

// Commands here either spawn subprocesses (`git check-ignore`, `rg`/`grep`,
// `fd`/`find`, `xdg-open`) or do blocking file I/O. They MUST run off the
// GTK main thread: a sync `#[tauri::command]` runs on the GTK main thread,
// and any wedged subprocess or slow disk freezes the whole UI hard enough
// that even window-close requests can't be processed. The fix is uniform —
// async command + `tokio::task::spawn_blocking`, same as `commands/git.rs`
// (see the note at the top of that file). Frontend-side `invoke()` already
// returns a Promise either way, so no caller changes are needed.

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

// `async fn` so the directory read + `git check-ignore` subprocess run on
// the blocking pool instead of the GTK main thread (see note at top of file).
#[tauri::command]
pub async fn list_directory(
    path: String,
    show_hidden: Option<bool>,
) -> Result<Vec<FileEntry>, String> {
    tokio::task::spawn_blocking(move || list_directory_blocking(&path, show_hidden))
        .await
        .map_err(|e| format!("list_directory task join failed: {e}"))?
}

fn list_directory_blocking(
    path: &str,
    show_hidden: Option<bool>,
) -> Result<Vec<FileEntry>, String> {
    let dir = Path::new(path);
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

// `async fn` so the `rg`/`grep` subprocess runs on the blocking pool instead
// of the GTK main thread (see note at top of file).
#[tauri::command]
pub async fn search_in_files(
    path: String,
    query: String,
    max_results: Option<u32>,
) -> Result<Vec<SearchResult>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = max_results.unwrap_or(100);

    tokio::task::spawn_blocking(move || {
        // Try ripgrep first
        if let Ok(results) = search_with_rg(&path, &query, limit) {
            return Ok(results);
        }

        // Fall back to grep
        search_with_grep(&path, &query, limit)
    })
    .await
    .map_err(|e| format!("search_in_files task join failed: {e}"))?
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

// `async fn` so the `fd`/`find` subprocess runs on the blocking pool instead
// of the GTK main thread (see note at top of file).
#[tauri::command]
pub async fn search_file_names(
    path: String,
    query: String,
    max_results: Option<u32>,
) -> Result<Vec<String>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = max_results.unwrap_or(50);

    tokio::task::spawn_blocking(move || {
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
    })
    .await
    .map_err(|e| format!("search_file_names task join failed: {e}"))?
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

// `async fn` so the `which` + `xdg-open` subprocesses run on the blocking
// pool instead of the GTK main thread (see note at top of file).
#[tauri::command]
pub async fn reveal_in_file_manager(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        if Command::new("which")
            .arg("xdg-open")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            == false
        {
            return Err(
                "xdg-open not found — cannot open file manager. Install xdg-utils.".to_string(),
            );
        }
        Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to open file manager: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("reveal_in_file_manager task join failed: {e}"))?
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

// `async fn` so the metadata stat + file read run on the blocking pool
// instead of the GTK main thread (see note at top of file).
#[tauri::command]
pub async fn read_file(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
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
    })
    .await
    .map_err(|e| format!("read_file task join failed: {e}"))?
}

// `async fn` so the file write runs on the blocking pool instead of the GTK
// main thread (see note at top of file).
#[tauri::command]
pub async fn write_file(path: String, content: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        std::fs::write(&path, &content).map_err(|e| format!("Failed to write file: {e}"))
    })
    .await
    .map_err(|e| format!("write_file task join failed: {e}"))?
}

/// Hard ceiling for clipboard image payloads. Mirrors the soft 5 MB
/// warning the chat composer surfaces — anything larger almost
/// certainly wasn't intended for a prompt and would just blow up the
/// agent's context. Reject at the IPC boundary so a runaway paste
/// can't fill the disk via the temp directory.
const MAX_CLIPBOARD_IMAGE_BYTES: usize = 25 * 1024 * 1024; // 25 MB

/// Resolve a stable extension for a clipboard image MIME type.
///
/// We deliberately keep this list short — these are the formats a
/// browser/OS clipboard realistically hands us. Anything else falls
/// back to `bin` so the file is still written (the agent can sniff
/// the bytes) but the filename doesn't lie about the format.
fn clipboard_image_extension(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

/// Shared write logic for clipboard image payloads.
///
/// Splits out from `save_clipboard_image_bytes` so the
/// `paste_clipboard_image_to_file` command can reuse the validation
/// and disk-write path without going through Tauri's IPC. The
/// clipboard-paste flow reads the OS clipboard server-side, encodes
/// PNG bytes in Rust, and calls this helper directly — avoiding the
/// (slow!) round-trip of shipping the image bytes through the JS
/// boundary just to ship them back.
fn write_clipboard_image_to_disk(bytes: &[u8], mime: &str) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("Clipboard image payload is empty".into());
    }
    if bytes.len() > MAX_CLIPBOARD_IMAGE_BYTES {
        return Err(format!(
            "Clipboard image too large ({:.1} MB, limit is {} MB)",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_CLIPBOARD_IMAGE_BYTES / (1024 * 1024),
        ));
    }
    if !mime.to_ascii_lowercase().starts_with("image/") {
        return Err(format!("Unsupported clipboard MIME type: {mime}"));
    }

    let dir = std::env::temp_dir().join("codemux-clipboard-images");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create clipboard image dir: {e}"))?;

    let ext = clipboard_image_extension(mime);
    let filename = format!("paste-{}.{}", uuid::Uuid::new_v4(), ext);
    let path = dir.join(&filename);

    std::fs::write(&path, bytes)
        .map_err(|e| format!("Failed to write clipboard image: {e}"))?;

    Ok(path.to_string_lossy().to_string())
}

/// Persist a caller-supplied clipboard image payload to a temp file.
///
/// Kept as a stable IPC entry point for callers that already have
/// encoded bytes in hand (and as a unit-testable surface for the
/// validation + write logic). The hot path for the new-workspace
/// dialog's paste flow is `paste_clipboard_image_to_file`, which
/// reads the OS clipboard server-side and bypasses the JS round
/// trip entirely.
///
/// `async fn` so the temp-dir create + file write run on the blocking
/// pool instead of the GTK main thread (see note at top of file).
#[tauri::command]
pub async fn save_clipboard_image_bytes(bytes: Vec<u8>, mime: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || write_clipboard_image_to_disk(&bytes, &mime))
        .await
        .map_err(|e| format!("save_clipboard_image_bytes task join failed: {e}"))?
}

/// Encode a raw RGBA pixel buffer (8 bits per channel) as PNG.
///
/// Extracted from `paste_clipboard_image_to_file` so the encoding
/// logic is unit-testable without an OS clipboard. The `rgba` slice
/// must be exactly `width * height * 4` bytes; we return an error
/// rather than panicking on a mismatch so a misbehaving clipboard
/// source can't crash the app.
fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "Image dimensions overflow".to_string())?;
    if rgba.len() != expected {
        return Err(format!(
            "RGBA length {} does not match width*height*4 = {}",
            rgba.len(),
            expected,
        ));
    }

    let mut out: Vec<u8> = Vec::with_capacity(rgba.len() / 4);
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG header write failed: {e}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("PNG body write failed: {e}"))?;
    drop(writer); // flush

    Ok(out)
}

/// Read the OS clipboard as an image, encode it as PNG, write it to
/// the same temp directory `save_clipboard_image_bytes` uses, and
/// return the absolute path.
///
/// Doing this on the Rust side has two motivations:
///   1. **Speed.** WebKit2GTK serialises `Vec<u8>` across IPC as a
///      JSON number array. A 1920×1080 RGBA buffer (~8 MB) becomes
///      a ~25 MB JSON string, and we'd be shipping it across twice
///      (JS reads from Rust, then Rust writes to disk). Keeping the
///      bytes resident in Rust collapses that to a single small
///      response — just the file path.
///   2. **Correctness.** The plugin's JS surface only exposes raw
///      RGBA pixels (no encoder). Writing those to a `.png` file
///      from JS produces a file with a misleading extension that
///      image viewers and agents cannot decode. Encoding PNG here
///      means the resulting attachment is a valid PNG.
/// `async fn` so the clipboard read (an OS round-trip that can stall),
/// the CPU-heavy PNG encode (~8 MB of RGBA for a 1080p screenshot), and
/// the file write all run on the blocking pool instead of the GTK main
/// thread (see note at top of file).
#[tauri::command]
pub async fn paste_clipboard_image_to_file<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        use tauri_plugin_clipboard_manager::ClipboardExt;

        // `read_image` resolves with an error when the clipboard does
        // not hold an image (e.g. text-only). Surface a stable error
        // string so the frontend can distinguish "no image" from a
        // genuine failure and let the default paste behaviour run.
        let img = app
            .clipboard()
            .read_image()
            .map_err(|e| format!("clipboard read_image failed: {e}"))?;

        let width = img.width();
        let height = img.height();
        let rgba = img.rgba();

        if width == 0 || height == 0 || rgba.is_empty() {
            return Err("Clipboard image is empty".into());
        }

        let png_bytes = encode_rgba_to_png(rgba, width, height)?;
        write_clipboard_image_to_disk(&png_bytes, "image/png")
    })
    .await
    .map_err(|e| format!("paste_clipboard_image_to_file task join failed: {e}"))?
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
    use super::{
        clipboard_image_extension, encode_rgba_to_png, grep_count_pattern, list_directory,
        read_file, save_clipboard_image_bytes, search_in_files, write_file,
        MAX_CLIPBOARD_IMAGE_BYTES,
    };
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
        // Build an absolute, definitely-not-existing path that's valid
        // on any platform — joining a fresh tempdir with a fake subdir
        // yields an absolute path on Linux (`/tmp/.tmpXXXX/missing`)
        // and Windows (`C:\Users\...\Temp\.tmpXXXX\missing`). The
        // hardcoded POSIX `/this/path/...` form failed the early
        // `is_absolute()` check on Windows and surfaced a different
        // error string ("cwd must be absolute") that didn't match this
        // assertion.
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("definitely-not-here");
        let err = grep_count_pattern(
            nonexistent.to_string_lossy().to_string(),
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

    // ── save_clipboard_image_bytes ──────────────────────────────

    /// Helper: clean up any file save_clipboard_image_bytes might
    /// leave behind in the shared temp directory. The command writes
    /// to a stable per-app subdir (so multiple paste-cycles share
    /// it), which means tests can't rely on tempdir isolation — they
    /// must clean up their own artifacts.
    fn cleanup_clipboard_temp(path: &str) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn clipboard_image_extension_maps_known_types() {
        assert_eq!(clipboard_image_extension("image/png"), "png");
        assert_eq!(clipboard_image_extension("image/jpeg"), "jpg");
        assert_eq!(clipboard_image_extension("image/jpg"), "jpg");
        assert_eq!(clipboard_image_extension("image/gif"), "gif");
        assert_eq!(clipboard_image_extension("image/webp"), "webp");
        assert_eq!(clipboard_image_extension("image/bmp"), "bmp");
        assert_eq!(clipboard_image_extension("image/svg+xml"), "svg");
    }

    #[test]
    fn clipboard_image_extension_is_case_insensitive() {
        // Some platforms hand us uppercase MIME types
        // (e.g. macOS clipboard providers). The mapping must match
        // regardless of casing.
        assert_eq!(clipboard_image_extension("IMAGE/PNG"), "png");
        assert_eq!(clipboard_image_extension("Image/Jpeg"), "jpg");
    }

    #[test]
    fn clipboard_image_extension_falls_back_for_unknown() {
        // Unknown but valid image/* types still get persisted — the
        // agent can sniff the bytes — but the filename uses `bin` so
        // it doesn't claim a format we don't recognise.
        assert_eq!(clipboard_image_extension("image/heic"), "bin");
        assert_eq!(clipboard_image_extension("image/x-icon"), "bin");
    }

    #[tokio::test]
    async fn save_clipboard_image_bytes_writes_png_and_returns_path() {
        // Minimal PNG signature — enough to verify the bytes round-trip.
        let payload: Vec<u8> = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let path = save_clipboard_image_bytes(payload.clone(), "image/png".into())
            .await
            .expect("png write should succeed");

        assert!(path.ends_with(".png"), "expected .png extension, got {path}");
        assert!(
            std::path::Path::new(&path).is_absolute(),
            "expected absolute path, got {path}",
        );

        let on_disk = fs::read(&path).expect("written file should be readable");
        assert_eq!(on_disk, payload, "bytes on disk must match payload");

        cleanup_clipboard_temp(&path);
    }

    #[tokio::test]
    async fn save_clipboard_image_bytes_uses_correct_extension_per_mime() {
        // Spot-check that the filename extension follows the MIME.
        // Doesn't re-test every mapping (clipboard_image_extension
        // covers that) — just verifies the command actually wires
        // the helper into the path.
        let cases = [
            ("image/jpeg", ".jpg"),
            ("image/gif", ".gif"),
            ("image/webp", ".webp"),
        ];
        for (mime, expected_ext) in cases {
            let path = save_clipboard_image_bytes(vec![0xff, 0xd8, 0xff], mime.into())
                .await
                .expect("write should succeed");
            assert!(
                path.ends_with(expected_ext),
                "expected {expected_ext} for {mime}, got {path}",
            );
            cleanup_clipboard_temp(&path);
        }
    }

    #[tokio::test]
    async fn save_clipboard_image_bytes_generates_unique_filenames() {
        // Two paste-cycles in a row must not clobber each other —
        // the UUID in the filename guarantees this. Regression
        // guard: if someone "simplifies" the naming and a user
        // pastes twice quickly, the second image would silently
        // overwrite the first attachment.
        let bytes = vec![0x89, 0x50, 0x4e, 0x47];
        let a = save_clipboard_image_bytes(bytes.clone(), "image/png".into())
            .await
            .unwrap();
        let b = save_clipboard_image_bytes(bytes, "image/png".into())
            .await
            .unwrap();
        assert_ne!(a, b, "consecutive saves must yield distinct paths");
        cleanup_clipboard_temp(&a);
        cleanup_clipboard_temp(&b);
    }

    #[tokio::test]
    async fn save_clipboard_image_bytes_rejects_empty_payload() {
        let err = save_clipboard_image_bytes(vec![], "image/png".into())
            .await
            .unwrap_err();
        assert!(
            err.to_lowercase().contains("empty"),
            "expected empty-payload error, got: {err}",
        );
    }

    #[tokio::test]
    async fn save_clipboard_image_bytes_rejects_non_image_mime() {
        // The frontend already filters for image/* before invoking
        // this command, but the IPC boundary must not trust the
        // caller — a misbehaving (or compromised) frontend mustn't
        // be able to write arbitrary blobs to the temp directory
        // via this command.
        let err = save_clipboard_image_bytes(vec![1, 2, 3], "text/plain".into())
            .await
            .unwrap_err();
        assert!(
            err.contains("Unsupported"),
            "expected unsupported-mime error, got: {err}",
        );

        let err = save_clipboard_image_bytes(vec![1, 2, 3], "application/pdf".into())
            .await
            .unwrap_err();
        assert!(
            err.contains("Unsupported"),
            "expected unsupported-mime error, got: {err}",
        );
    }

    #[tokio::test]
    async fn save_clipboard_image_bytes_rejects_oversize_payload() {
        // Use a payload exactly one byte over the cap. Allocating
        // 25 MB + 1 in a test is cheap (Vec::with_capacity is a
        // single allocation) and exercises the exact boundary
        // condition.
        let oversize = vec![0u8; MAX_CLIPBOARD_IMAGE_BYTES + 1];
        let err = save_clipboard_image_bytes(oversize, "image/png".into())
            .await
            .unwrap_err();
        assert!(
            err.contains("too large"),
            "expected oversize error, got: {err}",
        );
    }

    #[tokio::test]
    async fn save_clipboard_image_bytes_writes_under_codemux_clipboard_dir() {
        // The artifacts must land under a clearly-named subdir so
        // operators can wipe the cache without grepping through
        // every UUID in /tmp. Lock the directory name as part of
        // the contract.
        let path = save_clipboard_image_bytes(vec![0x89, 0x50, 0x4e, 0x47], "image/png".into())
            .await
            .unwrap();
        assert!(
            path.contains("codemux-clipboard-images"),
            "expected path under codemux-clipboard-images/, got {path}",
        );
        cleanup_clipboard_temp(&path);
    }

    // ── encode_rgba_to_png ───────────────────────────────────────

    /// Smallest valid PNG signature — used to assert the encoder
    /// produced a real PNG (not raw RGBA dumped to bytes). The
    /// regression we're guarding against: previously the JS side
    /// wrote raw RGBA into a .png file because the plugin didn't
    /// expose a PNG encoder.
    const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

    #[test]
    fn encode_rgba_to_png_produces_valid_png_header() {
        // 2×2 image, all-opaque red pixels.
        let rgba: Vec<u8> = vec![
            255, 0, 0, 255, // (0,0)
            255, 0, 0, 255, // (1,0)
            255, 0, 0, 255, // (0,1)
            255, 0, 0, 255, // (1,1)
        ];
        let out = encode_rgba_to_png(&rgba, 2, 2).expect("encode should succeed");
        assert!(
            out.len() >= 8 && out[..8] == PNG_MAGIC,
            "output must start with PNG magic bytes, got first 8: {:?}",
            &out[..out.len().min(8)],
        );
    }

    #[test]
    fn encode_rgba_to_png_roundtrips_through_decoder() {
        // 3×1 image with three distinct pixels. Decode the encoder
        // output with the same `png` crate and verify the pixels
        // round-trip. This is the strongest correctness assertion:
        // even a bogus encoder that wrote PNG magic followed by
        // garbage would fail this.
        let rgba: Vec<u8> = vec![
            10, 20, 30, 255, //
            40, 50, 60, 255, //
            70, 80, 90, 255, //
        ];
        let out = encode_rgba_to_png(&rgba, 3, 1).expect("encode should succeed");

        let decoder = png::Decoder::new(out.as_slice());
        let mut reader = decoder.read_info().expect("PNG should be parseable");
        let mut decoded = vec![0u8; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut decoded)
            .expect("PNG body should decode");
        assert_eq!(info.width, 3);
        assert_eq!(info.height, 1);
        decoded.truncate(info.buffer_size());
        assert_eq!(
            decoded, rgba,
            "decoded pixels must match the input RGBA",
        );
    }

    #[test]
    fn encode_rgba_to_png_rejects_mismatched_length() {
        // 2×2 needs 16 bytes; supplying 12 must error instead of
        // panicking on the slice index inside the encoder.
        let rgba = vec![0u8; 12];
        let err = encode_rgba_to_png(&rgba, 2, 2).unwrap_err();
        assert!(
            err.contains("does not match"),
            "expected length-mismatch error, got: {err}",
        );
    }

    #[test]
    fn encode_rgba_to_png_rejects_dimension_overflow() {
        // width * height * 4 must not overflow usize. On a 64-bit
        // target the largest single u32 dim is enough — pair two
        // u32::MAX values to force the overflow path.
        let err = encode_rgba_to_png(&[], u32::MAX, u32::MAX).unwrap_err();
        assert!(
            err.contains("overflow") || err.contains("does not match"),
            "expected overflow or mismatch error, got: {err}",
        );
    }

    // ── async file commands (off-main-thread conversion) ─────────
    //
    // These exercise the real command fns end-to-end: actual
    // subprocesses (`git check-ignore`, `rg`/`grep`, `fd`/`find`)
    // against real temp directories, through the async +
    // `spawn_blocking` path the GTK process uses. They double as
    // behavior-preservation guards for the conversion — each command
    // here used to be a sync `#[tauri::command]` that ran on the GTK
    // main thread.

    fn path_str(p: &std::path::Path) -> String {
        p.to_string_lossy().to_string()
    }

    #[tokio::test]
    async fn list_directory_sorts_dirs_first_then_alphabetical() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("zeta-dir")).unwrap();
        fs::write(dir.path().join("alpha.txt"), "a").unwrap();
        fs::write(dir.path().join("Beta.txt"), "b").unwrap();

        let entries = list_directory(path_str(dir.path()), None).await.unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["zeta-dir", "alpha.txt", "Beta.txt"],
            "directories first, then case-insensitive alphabetical"
        );
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].size, None, "directories report no size");
        assert_eq!(entries[1].size, Some(1));
    }

    #[tokio::test]
    async fn list_directory_errors_on_non_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain.txt");
        fs::write(&file, "x").unwrap();
        let err = list_directory(path_str(&file), None).await.unwrap_err();
        assert!(err.contains("Not a directory"), "got: {err}");
    }

    #[tokio::test]
    async fn list_directory_hides_dotfiles_unless_show_hidden() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".hidden"), "h").unwrap();
        fs::write(dir.path().join("visible.txt"), "v").unwrap();

        let default = list_directory(path_str(dir.path()), None).await.unwrap();
        assert!(
            default.iter().all(|e| e.name != ".hidden"),
            "dotfiles must be hidden by default"
        );

        let shown = list_directory(path_str(dir.path()), Some(true))
            .await
            .unwrap();
        assert!(
            shown.iter().any(|e| e.name == ".hidden"),
            "show_hidden=true must surface dotfiles"
        );
    }

    #[tokio::test]
    async fn list_directory_always_skips_git_dir_and_build_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();

        let entries = list_directory(path_str(dir.path()), Some(true))
            .await
            .unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["src"],
            ".git and node_modules must be skipped even with show_hidden"
        );
    }

    /// E2E through the real `git check-ignore` subprocess: entries
    /// matched by `.gitignore` must come back flagged.
    #[tokio::test]
    async fn list_directory_marks_gitignored_entries() {
        let dir = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .arg("init")
            .current_dir(dir.path())
            .output()
            .expect("git must be installed for this test");
        assert!(init.status.success(), "git init failed");

        fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.path().join("ignored.txt"), "x").unwrap();
        fs::write(dir.path().join("kept.txt"), "x").unwrap();

        let entries = list_directory(path_str(dir.path()), None).await.unwrap();
        let ignored = entries
            .iter()
            .find(|e| e.name == "ignored.txt")
            .expect("ignored.txt should be listed");
        assert!(ignored.is_gitignored, "ignored.txt must be flagged");
        let kept = entries
            .iter()
            .find(|e| e.name == "kept.txt")
            .expect("kept.txt should be listed");
        assert!(!kept.is_gitignored, "kept.txt must not be flagged");
    }

    /// E2E through the real `rg` (or `grep` fallback) subprocess.
    #[tokio::test]
    async fn search_in_files_finds_matches_with_positions() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("haystack.txt"),
            "nothing on line one\nthe needle is here\nmore filler\n",
        )
        .unwrap();
        fs::write(dir.path().join("other.txt"), "no match in this one\n").unwrap();

        let results = search_in_files(path_str(dir.path()), "needle".to_string(), None)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "exactly one line matches: {results:?}");
        let r = &results[0];
        assert!(
            r.file_path.ends_with("haystack.txt"),
            "unexpected file: {}",
            r.file_path
        );
        assert_eq!(r.line_number, 2);
        assert_eq!(r.line_content, "the needle is here");
        assert_eq!(
            &r.line_content[r.match_start as usize..r.match_end as usize],
            "needle",
            "match offsets must point at the query"
        );
    }

    #[tokio::test]
    async fn search_in_files_empty_query_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "content\n").unwrap();
        let results = search_in_files(path_str(dir.path()), String::new(), None)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    /// E2E through the real `fd` (or `find` fallback) subprocess.
    /// Unix-only: the Windows CI runner ships neither `fd` nor a
    /// POSIX `find`, and the expected relative paths use `/`.
    #[cfg(unix)]
    #[tokio::test]
    async fn search_file_names_returns_relative_paths() {
        use super::search_file_names;

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("alpha-one.txt"), "").unwrap();
        fs::write(dir.path().join("nested").join("alpha-two.txt"), "").unwrap();
        fs::write(dir.path().join("beta.txt"), "").unwrap();

        let mut results = search_file_names(path_str(dir.path()), "alpha".to_string(), None)
            .await
            .unwrap();
        results.sort();
        assert_eq!(
            results,
            vec!["alpha-one.txt".to_string(), "nested/alpha-two.txt".to_string()],
            "matches must come back relative to the search root"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn search_file_names_empty_query_returns_empty() {
        use super::search_file_names;
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        let results = search_file_names(path_str(dir.path()), String::new(), None)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn write_file_then_read_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        let content = "line one\nline two — utf-8 ✓\n";

        write_file(path_str(&file), content.to_string())
            .await
            .unwrap();
        let back = read_file(path_str(&file)).await.unwrap();
        assert_eq!(back, content);
    }

    #[tokio::test]
    async fn read_file_rejects_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_file(path_str(&dir.path().join("nope.txt")))
            .await
            .unwrap_err();
        assert!(err.contains("Not a file"), "got: {err}");
    }

    #[tokio::test]
    async fn read_file_rejects_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("blob.bin");
        fs::write(&file, [0x00u8, 0x01, 0x02, 0x00]).unwrap();
        let err = read_file(path_str(&file)).await.unwrap_err();
        assert!(err.contains("Binary file"), "got: {err}");
    }

    #[tokio::test]
    async fn read_file_rejects_oversize_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.txt");
        fs::write(&file, vec![b'a'; (super::MAX_FILE_SIZE + 1) as usize]).unwrap();
        let err = read_file(path_str(&file)).await.unwrap_err();
        assert!(err.contains("too large"), "got: {err}");
    }

    /// The point of the whole conversion: a command stuck in blocking
    /// I/O must NOT wedge the thread that drives the async executor
    /// (in production: the GTK main thread).
    ///
    /// Setup: `write_file` against a FIFO with no reader blocks inside
    /// `std::fs::write` until a reader opens the pipe. We drive the
    /// command on a single-threaded runtime and check that a timer on
    /// that same thread still fires while the write is wedged — which
    /// is only possible if the blocking work was shipped off to the
    /// blocking pool.
    ///
    /// A reader thread opens the FIFO after 2 s. It plays two roles:
    /// it lets the write complete, and it un-wedges a regressed
    /// (inline-blocking) implementation so this test fails fast on the
    /// `is_finished` assertion instead of hanging forever.
    #[cfg(unix)]
    #[test]
    fn write_file_does_not_block_the_async_executor_thread() {
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) },
            0,
            "mkfifo failed"
        );

        let reader_path = fifo.clone();
        let reader = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            fs::read(&reader_path).unwrap()
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let task = tokio::spawn(write_file(
                fifo.to_string_lossy().to_string(),
                "payload".to_string(),
            ));

            // This timer shares the executor thread with `task`. With
            // spawn_blocking it fires at ~200 ms while the write is
            // still pending. If the command blocked inline, the
            // executor would be wedged in open(2) until the reader
            // thread drains the FIFO at ~2 s — by which point the task
            // has finished and the assertion below fails.
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(
                !task.is_finished(),
                "write_file completed before the FIFO had a reader — the \
                 blocking write ran inline on the executor thread instead \
                 of the blocking pool"
            );

            task.await
                .unwrap()
                .expect("write should succeed once the reader opens");
        });

        assert_eq!(
            reader.join().unwrap(),
            b"payload",
            "reader must observe the written bytes"
        );
    }
}
