use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(any(unix, windows))]
use cap_std::fs::MetadataExt;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

// Older snapshots can contain content reached through links outside the project.
const INDEX_SCHEMA_VERSION: u32 = 3;
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
    event
        .paths
        .iter()
        .any(|p| !path_should_skip_for_index(p, project_root))
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
    inner: Arc<Mutex<Arc<ProjectIndexSnapshot>>>,
}

impl Default for ProjectIndexStore {
    fn default() -> Self {
        // Start with an empty index. The actual project root is set later in
        // setup() once the active workspace is known, avoiding the old bug
        // where env::current_dir() (often $HOME) was scanned.
        Self {
            inner: Arc::new(Mutex::new(Arc::new(default_index(PathBuf::new())))),
        }
    }
}

impl ProjectIndexStore {
    /// Load the cached index and keep the same root open for watcher rebuilds.
    pub fn initialize_for_project(&self, project_root: PathBuf) {
        let opened = project_root.canonicalize().and_then(|path| {
            let dir = open_project_dir(&path)?;
            Ok((path, dir))
        });
        let (watch_root, dir) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                eprintln!(
                    "[codemux::index] Failed to open {}: {error}",
                    project_root.display()
                );
                self.replace_snapshot(default_index(project_root));
                return;
            }
        };
        let snapshot =
            load_index(&project_root, &dir).unwrap_or_else(|| default_index(project_root.clone()));
        self.replace_snapshot(snapshot);
        spawn_index_watcher(self.inner.clone(), project_root, watch_root, dir);
    }

    /// Readers retain an immutable generation without cloning its file contents
    /// or holding the store lock while searching.
    pub fn snapshot(&self) -> Arc<ProjectIndexSnapshot> {
        self.inner.lock().unwrap().clone()
    }

    pub fn replace_snapshot(&self, snapshot: ProjectIndexSnapshot) {
        *self.inner.lock().unwrap() = Arc::new(snapshot);
    }

    pub fn status(&self) -> ProjectIndexStatus {
        let snapshot = self.snapshot();
        ProjectIndexStatus {
            project_root: snapshot.project_root.clone(),
            file_count: snapshot.file_count,
            chunk_count: snapshot.chunk_count,
            indexed_at_ms: snapshot.indexed_at_ms,
            watch_enabled: snapshot.watch_enabled,
            semantic_status: snapshot.semantic_status.clone(),
        }
    }
}

pub fn rebuild_index(project_root: Option<String>) -> Result<ProjectIndexSnapshot, String> {
    let root = resolve_project_root(project_root)?;
    let dir = open_project_dir(&root).map_err(|error| {
        format!(
            "Failed to open project directory {}: {error}",
            root.display()
        )
    })?;
    let snapshot = build_index_for_root(&root, &dir)?;
    save_index(&dir, &snapshot)?;
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

fn open_project_dir(path: &Path) -> io::Result<Dir> {
    Dir::open_ambient_dir(path, cap_std::ambient_authority())
}

#[cfg(windows)]
fn pinned_windows_path(dir: &Dir) -> io::Result<PathBuf> {
    // Preserve the verbatim path, including the UNC prefix on network shares.
    winx::file::get_file_path(&dir.try_clone()?.into_std_file())
}

#[cfg(windows)]
fn reopen_watched_dir(dir: &fs::File, allow_rename: bool) -> io::Result<fs::File> {
    open_watched_dir_at(dir, &winx::file::get_file_path(dir)?, allow_rename)
}

#[cfg(windows)]
fn open_watched_dir_at(dir: &fs::File, path: &Path, allow_rename: bool) -> io::Result<fs::File> {
    use crate::windows_file_id::file_id;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let mut options = fs::OpenOptions::new();
    if allow_rename {
        // Idle watchers need only identity and the current name. The default
        // share mode permits reads, writes, and renames by other processes.
        options.access_mode(FILE_READ_ATTRIBUTES);
    } else {
        options
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    // The name can be reassigned between looking it up and opening it.
    // Keep both handles alive until their filesystem identities match.
    if !metadata.is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || file_id(&file)? != file_id(dir)?
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Watched directory was replaced",
        ));
    }
    Ok(file)
}

fn spawn_index_watcher(
    store: Arc<Mutex<Arc<ProjectIndexSnapshot>>>,
    project_root: PathBuf,
    watch_root: PathBuf,
    dir: Dir,
) {
    eprintln!(
        "[codemux::index] Watching project root: {}",
        project_root.display()
    );
    std::thread::spawn(move || {
        let (tx, rx) = channel();
        let mut watcher =
            match RecommendedWatcher::new(tx, Config::default().with_follow_symlinks(false)) {
                Ok(watcher) => watcher,
                Err(error) => {
                    eprintln!("[codemux::index] Failed to create watcher: {error}");
                    return;
                }
            };

        if let Err(error) = watcher.watch(&watch_root, RecursiveMode::Recursive) {
            eprintln!(
                "[codemux::index] Failed to watch {}: {error}",
                project_root.display()
            );
            return;
        }

        // Keep the identity of the watched directory without locking its name
        // between rebuilds.
        #[cfg(windows)]
        let watched = match reopen_watched_dir(&dir.into_std_file(), true) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("[codemux::index] Failed to retain watched directory: {error}");
                return;
            }
        };

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
            if !event_has_relevant_paths(&event, &watch_root) {
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
                            && event_has_relevant_paths(&ev, &watch_root)
                        {
                            deadline = Instant::now() + INDEX_REBUILD_DEBOUNCE;
                        }
                    }
                    Ok(Err(error)) => eprintln!("[codemux::index] Watch error: {error}"),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }

            #[cfg(windows)]
            let dir = match reopen_watched_dir(&watched, false) {
                Ok(pinned) => Dir::from_std_file(pinned),
                Err(error) => {
                    eprintln!(
                        "[codemux::index] Failed to open watched directory for indexing: {error}"
                    );
                    continue;
                }
            };
            match build_index_for_root(&project_root, &dir) {
                Ok(snapshot) => {
                    let _ = save_index(&dir, &snapshot);
                    *store.lock().unwrap() = Arc::new(snapshot);
                }
                Err(error) => {
                    eprintln!("[codemux::index] Failed to refresh index: {error}");
                }
            }
        }
    });
}

const INDEX_FILE_NAME: &str = "index.json";

fn load_index(root_path: &Path, root: &Dir) -> Option<ProjectIndexSnapshot> {
    let dir = open_child_dir(root, Path::new(".codemux")).ok()?;
    let mut contents = Vec::new();
    let (mut file, _) = open_regular_file(&dir, Path::new(INDEX_FILE_NAME)).ok()?;
    file.read_to_end(&mut contents).ok()?;
    let snapshot: ProjectIndexSnapshot = serde_json::from_slice(&contents).ok()?;
    (snapshot.schema_version == INDEX_SCHEMA_VERSION
        && Path::new(&snapshot.project_root) == root_path)
        .then_some(snapshot)
}

fn build_index_for_root(project_root: &Path, root: &Dir) -> Result<ProjectIndexSnapshot, String> {
    let mut files = Vec::new();
    let mut chunks = Vec::new();
    let mut seen_dirs = HashSet::new();
    let mut total_bytes: usize = 0;
    scan_directory(
        root,
        Path::new(""),
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
    dir: &Dir,
    relative_dir: &Path,
    files: &mut Vec<IndexedFile>,
    chunks: &mut Vec<IndexedChunk>,
    seen_dirs: &mut HashSet<(u64, u64)>,
    total_bytes: &mut usize,
) -> Result<(), String> {
    // Cycle detection: track visited directories by (device, inode) identity.
    // Also terminate walks across repeated bind mounts.
    #[cfg(unix)]
    {
        let meta = dir.dir_metadata().map_err(|error| {
            format!(
                "Failed to identify directory {}: {error}",
                relative_dir.display()
            )
        })?;
        if !seen_dirs.insert((meta.dev(), meta.ino())) {
            return Ok(());
        }
    }

    if *total_bytes >= MAX_TOTAL_INDEX_BYTES {
        return Ok(());
    }

    #[cfg(not(windows))]
    let entries = dir.entries();
    #[cfg(windows)]
    let entries = pinned_windows_path(dir).and_then(fs::read_dir);
    let entries = entries.map_err(|error| {
        format!(
            "Failed to read directory {}: {error}",
            relative_dir.display()
        )
    })?;

    for entry in entries {
        if *total_bytes >= MAX_TOTAL_INDEX_BYTES {
            break;
        }
        let entry = entry.map_err(|error| format!("Failed to read directory entry: {error}"))?;
        let name = entry.file_name();
        let name = Path::new(&name);
        let path = relative_dir.join(name);
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let metadata = entry.metadata().map_err(|error| {
                format!("Failed to read attributes for {}: {error}", path.display())
            })?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue;
            }
        }
        let kind = entry
            .file_type()
            .map_err(|error| format!("Failed to read file type for {}: {error}", path.display()))?;
        if kind.is_dir() {
            if !IGNORED_DIRS.contains(&name.to_string_lossy().as_ref()) {
                let child = open_child_dir(dir, name).map_err(|error| {
                    format!("Failed to open directory {}: {error}", path.display())
                })?;
                scan_directory(&child, &path, files, chunks, seen_dirs, total_bytes)?;
            }
            continue;
        }
        if !kind.is_file() || !is_indexable_file(&path) {
            continue;
        }

        if let Some((file, file_chunks)) = index_file(dir, name, &path)? {
            *total_bytes += file.size_bytes as usize;
            files.push(file);
            chunks.extend(file_chunks);
        }
    }

    Ok(())
}

fn index_file(
    dir: &Dir,
    name: &Path,
    path: &Path,
) -> Result<Option<(IndexedFile, Vec<IndexedChunk>)>, String> {
    let (file, metadata) = open_regular_file(dir, name)
        .map_err(|error| format!("Failed to open file {}: {error}", path.display()))?;

    if metadata.len() > MAX_FILE_SIZE_BYTES {
        return Ok(None);
    }

    let mut bytes = Vec::new();
    file.take(MAX_FILE_SIZE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read file {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_SIZE_BYTES || bytes.contains(&0) {
        return Ok(None);
    }

    let content = String::from_utf8(bytes)
        .map_err(|error| format!("Failed to decode file {} as UTF-8: {error}", path.display()))?;

    let relative = path.display().to_string();
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
            size_bytes: content.len() as u64,
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
    if terms.is_empty() || limit == 0 {
        return vec![];
    }

    // A file can contribute many matching chunks. Find its symbols through
    // one lookup instead of linearly scanning every indexed file each time.
    let mut files_by_path = HashMap::with_capacity(snapshot.files.len());
    for file in &snapshot.files {
        files_by_path.entry(file.file_path.as_str()).or_insert(file);
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

            let matched_symbols = files_by_path
                .get(chunk.file_path.as_str())
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

fn save_index(root: &Dir, snapshot: &ProjectIndexSnapshot) -> Result<(), String> {
    #[cfg(windows)]
    let created = pinned_windows_path(root).and_then(|path| fs::create_dir(path.join(".codemux")));
    #[cfg(not(windows))]
    let created = root.create_dir(".codemux");
    if let Err(error) = created {
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(format!("Failed to create index directory: {error}"));
        }
    }
    let dir = open_child_dir(root, Path::new(".codemux"))
        .map_err(|error| format!("Failed to open index directory: {error}"))?;
    persist_index(&dir, snapshot).map_err(|error| format!("Failed to write project index: {error}"))
}

fn persist_index(dir: &Dir, snapshot: &ProjectIndexSnapshot) -> io::Result<()> {
    if let Err(error) = open_regular_file(dir, Path::new(INDEX_FILE_NAME)) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    let staging_name = format!("index-{}.tmp", uuid::Uuid::new_v4());
    #[cfg(windows)]
    let (staging_path, index_path) = {
        let path = pinned_windows_path(dir)?;
        (path.join(&staging_name), path.join(INDEX_FILE_NAME))
    };
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = dir.open_with(&staging_name, &options)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, snapshot)?;
        writer.flush()?;
        drop(writer);
        #[cfg(windows)]
        {
            fs::rename(&staging_path, index_path)
        }
        #[cfg(not(windows))]
        {
            dir.rename(&staging_name, dir, INDEX_FILE_NAME)
        }
    })();
    if result.is_err() {
        #[cfg(windows)]
        let _ = fs::remove_file(staging_path);
        #[cfg(not(windows))]
        let _ = dir.remove_file(&staging_name);
    }
    result
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

fn open_child_dir(parent: &Dir, name: &Path) -> io::Result<Dir> {
    let child = parent.open_dir_nofollow(name)?;
    #[cfg(windows)]
    if child.dir_metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("Unexpected reparse point"));
    }
    Ok(child)
}

fn open_regular_file(dir: &Dir, name: &Path) -> io::Result<(fs::File, fs::Metadata)> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = dir.open_with(name, &options)?.into_std();
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("Unexpected reparse point"));
        }
    }
    if !metadata.is_file() {
        return Err(io::Error::other("Expected a regular file"));
    }
    Ok((file, metadata))
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
    fn cache_is_used_only_for_its_selected_root_and_schema() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let dir = open_project_dir(root).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        let mut snapshot = rebuild_index(Some(root.display().to_string())).unwrap();
        assert_eq!(load_index(root, &dir).unwrap().file_count, 1);

        snapshot.schema_version = INDEX_SCHEMA_VERSION - 1;
        save_index(&dir, &snapshot).unwrap();
        assert!(load_index(root, &dir).is_none());

        snapshot.schema_version = INDEX_SCHEMA_VERSION;
        snapshot.project_root = root.join("previous-location").display().to_string();
        save_index(&dir, &snapshot).unwrap();
        assert!(load_index(root, &dir).is_none());
    }

    #[test]
    fn rebuilding_an_index_preserves_git_excludes() {
        let project = tempfile::tempdir().unwrap();
        let info = project.path().join(".git/info");
        fs::create_dir_all(&info).unwrap();
        fs::write(info.join("exclude"), b"*.local\n").unwrap();
        rebuild_index(Some(project.path().display().to_string())).unwrap();
        assert_eq!(fs::read(info.join("exclude")).unwrap(), b"*.local\n");
    }

    #[cfg(unix)]
    #[test]
    fn scan_and_cache_keep_using_open_directories_after_a_rename() {
        let project = tempfile::tempdir().unwrap();
        let selected = project.path().join("selected");
        let moved = project.path().join("moved");
        fs::create_dir_all(selected.join("src")).unwrap();
        fs::write(selected.join("src/original.rs"), "fn original() {}").unwrap();
        let parent = open_project_dir(project.path()).unwrap();
        let root = open_child_dir(&parent, Path::new("selected")).unwrap();
        fs::rename(&selected, &moved).unwrap();
        fs::create_dir(&selected).unwrap();

        let snapshot = build_index_for_root(&selected, &root).unwrap();
        assert_eq!(snapshot.file_count, 1);
        assert_eq!(snapshot.files[0].file_path, "src/original.rs");
        save_index(&root, &snapshot).unwrap();
        assert!(load_index(&selected, &root).is_some());
        assert!(!selected.join(".codemux").exists());

        let cache = open_child_dir(&root, Path::new(".codemux")).unwrap();
        fs::rename(moved.join(".codemux"), moved.join("saved")).unwrap();
        fs::create_dir(moved.join(".codemux")).unwrap();
        let empty = default_index(selected);
        persist_index(&cache, &empty).unwrap();
        let saved: ProjectIndexSnapshot =
            serde_json::from_slice(&fs::read(moved.join("saved/index.json")).unwrap()).unwrap();
        assert_eq!(saved.file_count, 0);
        assert!(!moved.join(".codemux/index.json").exists());
    }

    #[cfg(windows)]
    #[test]
    fn watched_directory_can_be_renamed_between_scans() {
        let project = tempfile::tempdir().unwrap();
        let selected = project.path().join("selected");
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("original.rs"), "fn original() {}").unwrap();
        let watched =
            reopen_watched_dir(&open_project_dir(&selected).unwrap().into_std_file(), true)
                .unwrap();
        fs::rename(&selected, project.path().join("moved")).unwrap();
        fs::create_dir(&selected).unwrap();
        assert_eq!(
            open_watched_dir_at(&watched, &selected, false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let pinned = Dir::from_std_file(reopen_watched_dir(&watched, false).unwrap());
        let snapshot = build_index_for_root(&selected, &pinned).unwrap();
        assert_eq!(snapshot.file_count, 1);
        assert_eq!(snapshot.files[0].file_path, "original.rs");
        save_index(&pinned, &snapshot).unwrap();
        assert!(!selected.join(".codemux").exists());
    }

    #[cfg(unix)]
    #[test]
    fn selected_root_can_be_an_alias_but_its_children_must_be_regular_files_or_directories() {
        use std::os::unix::{fs::symlink, net::UnixListener};

        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        let root = temp.path().join("selected");
        let sibling = temp.path().join("sibling");
        fs::create_dir_all(actual.join("src")).unwrap();
        fs::create_dir(&sibling).unwrap();
        symlink(&actual, &root).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(sibling.join("notes.md"), "notes").unwrap();
        symlink(sibling.join("notes.md"), root.join("file-link.md")).unwrap();
        symlink(&sibling, root.join("dir-link")).unwrap();
        let _socket = UnixListener::bind(root.join("socket.txt")).unwrap();

        let snapshot = rebuild_index(Some(root.display().to_string())).unwrap();
        assert_eq!(snapshot.file_count, 1);
        assert_eq!(snapshot.files[0].file_path, "src/main.rs");
        assert!(load_index(&root, &open_project_dir(&root).unwrap()).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn cache_requires_a_local_directory_and_regular_file() {
        use std::os::unix::fs::symlink;

        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let handle = open_project_dir(root).unwrap();
        let other = tempfile::tempdir().unwrap();
        let snapshot = default_index(root.to_owned());
        let sentinel = serde_json::to_vec(&snapshot).unwrap();
        let other_file = other.path().join("index.json");
        fs::write(&other_file, &sentinel).unwrap();
        let dir = root.join(".codemux");
        symlink(other.path(), &dir).unwrap();
        assert!(load_index(root, &handle).is_none());
        assert!(save_index(&handle, &snapshot).is_err());

        fs::remove_file(&dir).unwrap();
        fs::create_dir(&dir).unwrap();
        symlink(&other_file, dir.join("index.json")).unwrap();
        assert!(load_index(root, &handle).is_none());
        assert!(save_index(&handle, &snapshot).is_err());
        assert_eq!(fs::read(other_file).unwrap(), sentinel);
    }

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

    #[test]
    fn index_reads_share_storage_and_keep_replaced_snapshots_alive() {
        let store = ProjectIndexStore::default();
        let mut initial = default_index(PathBuf::from("/first"));
        initial.chunk_count = 1;
        initial.chunks.push(IndexedChunk {
            chunk_id: "first.rs:1-1".into(),
            file_path: "first.rs".into(),
            line_start: 1,
            line_end: 1,
            text: "original needle".into(),
        });
        store.replace_snapshot(initial);

        let first = store.snapshot();
        let second = store.snapshot();
        assert_eq!(first.chunks.as_ptr(), second.chunks.as_ptr());
        assert_eq!(store.status().chunk_count, 1);
        assert_eq!(
            search_index(&store, "needle", None)[0].file_path,
            "first.rs"
        );

        store.replace_snapshot(default_index(PathBuf::from("/second")));
        assert_eq!(first.project_root, "/first");
        assert_eq!(first.chunks[0].text, "original needle");
        assert_eq!(store.status().project_root, "/second");
        assert!(search_index(&store, "needle", None).is_empty());
    }
}
