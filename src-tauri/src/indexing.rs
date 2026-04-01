use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const INDEX_SCHEMA_VERSION: u32 = 1;
const MAX_FILE_SIZE_BYTES: u64 = 512 * 1024;
const CHUNK_LINE_COUNT: usize = 40;
const DEFAULT_SEARCH_LIMIT: usize = 12;
/// Stop indexing after accumulating this many bytes of file content.
const MAX_TOTAL_INDEX_BYTES: usize = 50 * 1024 * 1024;
const TEXT_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "svelte", "md", "json", "toml", "yaml", "yml", "py", "go",
    "java", "c", "cpp", "h", "hpp", "css", "html", "txt", "sh",
];
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".svelte-kit",
    "dist",
    "build",
    ".codemux",
];

/// Debounce period: wait this long after the last relevant file change before rebuilding the index.
const INDEX_REBUILD_DEBOUNCE: Duration = Duration::from_secs(2);

/// Returns true if this path is under .codemux or any IGNORED_DIR under project_root.
/// Events that only touch such paths are skipped to avoid feedback loops and unnecessary rebuilds.
fn path_should_skip_for_index(path: &Path, project_root: &Path) -> bool {
    let stripped = match path.strip_prefix(project_root) {
        Ok(s) => s,
        Err(_) => return true,
    };
    for component in stripped.components() {
        if let std::path::Component::Normal(c) = component {
            if let Some(name) = c.to_str() {
                if IGNORED_DIRS.iter().any(|d| *d == name) {
                    return true;
                }
            }
        }
    }
    false
}

/// True if the event contains at least one path that should trigger an index rebuild.
fn event_has_relevant_paths(event: &notify::Event, project_root: &Path) -> bool {
    event.paths.iter().any(|p| !path_should_skip_for_index(p, project_root))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedChunk {
    pub chunk_id: String,
    pub file_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFile {
    pub file_path: String,
    pub language: Option<String>,
    pub size_bytes: u64,
    pub modified_at_ms: u64,
    pub symbol_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectIndexSnapshot {
    pub schema_version: u32,
    pub project_root: String,
    pub indexing_strategy: String,
    pub semantic_status: String,
    pub file_count: usize,
    pub chunk_count: usize,
    pub indexed_at_ms: u64,
    pub watch_enabled: bool,
    pub files: Vec<IndexedFile>,
    pub chunks: Vec<IndexedChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectIndexStatus {
    pub project_root: String,
    pub file_count: usize,
    pub chunk_count: usize,
    pub indexed_at_ms: u64,
    pub watch_enabled: bool,
    pub semantic_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSearchResult {
    pub file_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub score: usize,
    pub snippet: String,
    pub matched_symbols: Vec<String>,
}

pub struct ProjectIndexStore {
    inner: Arc<Mutex<ProjectIndexSnapshot>>,
}

impl Default for ProjectIndexStore {
    fn default() -> Self {
        // Start with an empty index. The actual project root is set later in
        // setup() once the active workspace is known, avoiding the old bug
        // where env::current_dir() (often $HOME) was scanned.
        Self {
            inner: Arc::new(Mutex::new(default_index(PathBuf::new()))),
        }
    }
}

impl ProjectIndexStore {
    /// Load (or build) the index for a specific project directory.
    pub fn initialize_for_project(&self, project_root: PathBuf) {
        let snapshot = load_or_default_index(Some(project_root.display().to_string()))
            .unwrap_or_else(|_| default_index(project_root));
        *self.inner.lock().unwrap() = snapshot;
    }

    pub fn snapshot(&self) -> ProjectIndexSnapshot {
        self.inner.lock().unwrap().clone()
    }

    pub fn replace_snapshot(&self, snapshot: ProjectIndexSnapshot) {
        *self.inner.lock().unwrap() = snapshot;
    }

    pub fn status(&self) -> ProjectIndexStatus {
        let snapshot = self.snapshot();
        ProjectIndexStatus {
            project_root: snapshot.project_root,
            file_count: snapshot.file_count,
            chunk_count: snapshot.chunk_count,
            indexed_at_ms: snapshot.indexed_at_ms,
            watch_enabled: snapshot.watch_enabled,
            semantic_status: snapshot.semantic_status,
        }
    }
}

pub fn rebuild_index(project_root: Option<String>) -> Result<ProjectIndexSnapshot, String> {
    let root = resolve_project_root(project_root)?;
    let snapshot = build_index_for_root(&root)?;
    save_index(&snapshot)?;
    Ok(snapshot)
}

pub fn search_index(
    store: &ProjectIndexStore,
    query: &str,
    limit: Option<usize>,
) -> Vec<IndexSearchResult> {
    let snapshot = store.snapshot();
    search_snapshot(&snapshot, query, limit.unwrap_or(DEFAULT_SEARCH_LIMIT))
}

pub fn spawn_index_watcher(store: tauri::State<'_, ProjectIndexStore>) {
    let project_root = PathBuf::from(store.snapshot().project_root);
    if project_root.as_os_str().is_empty() || !project_root.is_dir() {
        eprintln!("[codemux::index] No valid project root — skipping index watcher");
        return;
    }
    eprintln!(
        "[codemux::index] Watching project root: {}",
        project_root.display()
    );
    let store = store.inner.clone();

    std::thread::spawn(move || {
        let (tx, rx) = channel();
        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("[codemux::index] Failed to create watcher: {error}");
                return;
            }
        };

        if let Err(error) = watcher.watch(&project_root, RecursiveMode::Recursive) {
            eprintln!(
                "[codemux::index] Failed to watch {}: {error}",
                project_root.display()
            );
            return;
        }

        loop {
            let event = match rx.recv() {
                Ok(ev) => ev,
                Err(_) => break,
            };
            let event = match event {
                Ok(ev) => ev,
                Err(error) => {
                    eprintln!("[codemux::index] Watch error: {error}");
                    continue;
                }
            };
            if event.paths.is_empty() {
                continue;
            }
            if !matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                continue;
            }
            if !event_has_relevant_paths(&event, &project_root) {
                continue;
            }

            // Debounce: wait INDEX_REBUILD_DEBOUNCE of quiet before rebuilding.
            // If another relevant event arrives, reset the timer.
            let mut deadline = Instant::now() + INDEX_REBUILD_DEBOUNCE;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(Ok(ev)) => {
                        if !ev.paths.is_empty()
                            && matches!(
                                ev.kind,
                                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                            )
                            && event_has_relevant_paths(&ev, &project_root)
                        {
                            deadline = Instant::now() + INDEX_REBUILD_DEBOUNCE;
                        }
                    }
                    Ok(Err(error)) => eprintln!("[codemux::index] Watch error: {error}"),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }

            let current_root = project_root.clone();
            match build_index_for_root(&current_root) {
                Ok(snapshot) => {
                    let _ = save_index(&snapshot);
                    *store.lock().unwrap() = snapshot;
                }
                Err(error) => {
                    eprintln!("[codemux::index] Failed to refresh index: {error}");
                }
            }
        }
    });
}

fn load_or_default_index(project_root: Option<String>) -> Result<ProjectIndexSnapshot, String> {
    let root = resolve_project_root(project_root)?;
    let path = index_path(&root);
    if !path.exists() {
        return Ok(default_index(root));
    }

    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read index file {}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("Failed to parse index file {}: {error}", path.display()))
}

fn build_index_for_root(project_root: &Path) -> Result<ProjectIndexSnapshot, String> {
    let mut files = Vec::new();
    let mut chunks = Vec::new();
    let mut seen_dirs = HashSet::new();
    let mut total_bytes: usize = 0;
    scan_directory(
        project_root,
        project_root,
        &mut files,
        &mut chunks,
        &mut seen_dirs,
        &mut total_bytes,
    )?;

    if total_bytes >= MAX_TOTAL_INDEX_BYTES {
        eprintln!(
            "[codemux::index] Index size cap reached ({} bytes), some files were skipped",
            total_bytes
        );
    }

    Ok(ProjectIndexSnapshot {
        schema_version: INDEX_SCHEMA_VERSION,
        project_root: project_root.display().to_string(),
        indexing_strategy: "local_lexical_chunk_index".into(),
        semantic_status: "evaluated: start lexical first, add optional embeddings later if needed"
            .into(),
        file_count: files.len(),
        chunk_count: chunks.len(),
        indexed_at_ms: current_time_ms(),
        watch_enabled: true,
        files,
        chunks,
    })
}

fn scan_directory(
    project_root: &Path,
    dir: &Path,
    files: &mut Vec<IndexedFile>,
    chunks: &mut Vec<IndexedChunk>,
    seen_dirs: &mut HashSet<(u64, u64)>,
    total_bytes: &mut usize,
) -> Result<(), String> {
    // Cycle detection: track visited directories by (device, inode) identity.
    // This prevents infinite recursion when symlinks create cycles.
    #[cfg(unix)]
    {
        if let Ok(meta) = fs::metadata(dir) {
            let key = (meta.dev(), meta.ino());
            if !seen_dirs.insert(key) {
                return Ok(());
            }
        }
    }

    if *total_bytes >= MAX_TOTAL_INDEX_BYTES {
        return Ok(());
    }

    let entries = fs::read_dir(dir)
        .map_err(|error| format!("Failed to read directory {}: {error}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|error| format!("Failed to read directory entry: {error}"))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if path.is_dir() {
            if IGNORED_DIRS.iter().any(|ignored| *ignored == file_name) {
                continue;
            }
            scan_directory(project_root, &path, files, chunks, seen_dirs, total_bytes)?;
            continue;
        }

        if *total_bytes >= MAX_TOTAL_INDEX_BYTES {
            return Ok(());
        }

        if !is_indexable_file(&path) {
            continue;
        }

        if let Some((file, file_chunks)) = index_file(project_root, &path)? {
            *total_bytes += file.size_bytes as usize;
            files.push(file);
            chunks.extend(file_chunks);
        }
    }

    Ok(())
}

fn index_file(
    project_root: &Path,
    path: &Path,
) -> Result<Option<(IndexedFile, Vec<IndexedChunk>)>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Failed to read metadata for {}: {error}", path.display()))?;

    if metadata.len() > MAX_FILE_SIZE_BYTES {
        return Ok(None);
    }

    let bytes = fs::read(path)
        .map_err(|error| format!("Failed to read file {}: {error}", path.display()))?;
    if bytes.iter().any(|byte| *byte == 0) {
        return Ok(None);
    }

    let content = String::from_utf8(bytes)
        .map_err(|error| format!("Failed to decode file {} as UTF-8: {error}", path.display()))?;

    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string();
    let modified_at_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let language = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_string());
    let symbol_names = extract_symbols(&content);
    let chunks = build_chunks(&relative, &content);

    Ok(Some((
        IndexedFile {
            file_path: relative,
            language,
            size_bytes: metadata.len(),
            modified_at_ms,
            symbol_names,
        },
        chunks,
    )))
}

fn build_chunks(file_path: &str, content: &str) -> Vec<IndexedChunk> {
    let lines = content.lines().collect::<Vec<_>>();
    let mut chunks = Vec::new();

    for (index, chunk_lines) in lines.chunks(CHUNK_LINE_COUNT).enumerate() {
        let line_start = index * CHUNK_LINE_COUNT + 1;
        let line_end = line_start + chunk_lines.len().saturating_sub(1);
        chunks.push(IndexedChunk {
            chunk_id: format!("{}:{}-{}", file_path, line_start, line_end),
            file_path: file_path.to_string(),
            line_start,
            line_end,
            text: chunk_lines.join("\n"),
        });
    }

    chunks
}

fn extract_symbols(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            [
                "fn ",
                "struct ",
                "enum ",
                "trait ",
                "class ",
                "interface ",
                "type ",
                "const ",
            ]
            .iter()
            .find_map(|prefix| {
                trimmed.strip_prefix(prefix).map(|rest| {
                    rest.split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                        .next()
                        .unwrap_or_default()
                        .to_string()
                })
            })
        })
        .filter(|symbol| !symbol.is_empty())
        .take(128)
        .collect()
}

fn search_snapshot(
    snapshot: &ProjectIndexSnapshot,
    query: &str,
    limit: usize,
) -> Vec<IndexSearchResult> {
    let terms = query
        .split_whitespace()
        .map(|term| term.to_lowercase())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return vec![];
    }

    let mut results = snapshot
        .chunks
        .iter()
        .filter_map(|chunk| {
            let haystack = chunk.text.to_lowercase();
            let score = terms
                .iter()
                .filter(|term| haystack.contains(term.as_str()))
                .count();
            if score == 0 {
                return None;
            }

            let matched_symbols = snapshot
                .files
                .iter()
                .find(|file| file.file_path == chunk.file_path)
                .map(|file| {
                    file.symbol_names
                        .iter()
                        .filter(|symbol| {
                            let symbol_lower = symbol.to_lowercase();
                            terms.iter().any(|term| symbol_lower.contains(term))
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            Some(IndexSearchResult {
                file_path: chunk.file_path.clone(),
                line_start: chunk.line_start,
                line_end: chunk.line_end,
                score,
                snippet: chunk.text.lines().take(8).collect::<Vec<_>>().join("\n"),
                matched_symbols,
            })
        })
        .collect::<Vec<_>>();

    results.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.file_path.cmp(&right.file_path))
    });
    results.truncate(limit);
    results
}

fn save_index(snapshot: &ProjectIndexSnapshot) -> Result<(), String> {
    let root = PathBuf::from(&snapshot.project_root);
    let dir = root.join(".codemux");
    fs::create_dir_all(&dir).map_err(|error| {
        format!(
            "Failed to create index directory {}: {error}",
            dir.display()
        )
    })?;
    let path = index_path(&root);
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|error| format!("Failed to serialize project index: {error}"))?;
    fs::write(&path, json).map_err(|error| {
        format!(
            "Failed to write project index file {}: {error}",
            path.display()
        )
    })
}

fn default_index(project_root: PathBuf) -> ProjectIndexSnapshot {
    ProjectIndexSnapshot {
        schema_version: INDEX_SCHEMA_VERSION,
        project_root: project_root.display().to_string(),
        indexing_strategy: "local_lexical_chunk_index".into(),
        semantic_status: "evaluated: start lexical first, add optional embeddings later if needed"
            .into(),
        file_count: 0,
        chunk_count: 0,
        indexed_at_ms: 0,
        watch_enabled: true,
        files: vec![],
        chunks: vec![],
    }
}

fn resolve_project_root(project_root: Option<String>) -> Result<PathBuf, String> {
    match project_root {
        Some(root) => Ok(PathBuf::from(root)),
        None => Err("No project root specified".into()),
    }
}

fn index_path(project_root: &Path) -> PathBuf {
    project_root.join(".codemux").join("index.json")
}

fn is_indexable_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            TEXT_EXTENSIONS
                .iter()
                .any(|candidate| candidate == &extension)
        })
        .unwrap_or(false)
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_snapshot_returns_ranked_matches() {
        let snapshot = ProjectIndexSnapshot {
            schema_version: INDEX_SCHEMA_VERSION,
            project_root: "/tmp/project".into(),
            indexing_strategy: "lexical".into(),
            semantic_status: "deferred".into(),
            file_count: 1,
            chunk_count: 1,
            indexed_at_ms: 1,
            watch_enabled: true,
            files: vec![IndexedFile {
                file_path: "src/main.rs".into(),
                language: Some("rs".into()),
                size_bytes: 32,
                modified_at_ms: 1,
                symbol_names: vec!["build_index".into()],
            }],
            chunks: vec![IndexedChunk {
                chunk_id: "src/main.rs:1-3".into(),
                file_path: "src/main.rs".into(),
                line_start: 1,
                line_end: 3,
                text: "fn build_index() {\n  // lexical search\n}".into(),
            }],
        };

        let results = search_snapshot(&snapshot, "build lexical", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].score, 2);
        assert_eq!(results[0].matched_symbols, vec!["build_index"]);
    }
}
