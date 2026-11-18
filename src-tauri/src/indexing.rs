use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex, Weak};
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
fn path_should_skip_for_index(path: &Path, project_root: &Path, cache: &IndexCache) -> bool {
    let stripped = match path.strip_prefix(project_root) {
        Ok(s) => s,
        Err(_) => return true,
    };
    if cache.excludes(stripped) {
        return true;
    }
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
fn event_has_relevant_paths(
    event: &notify::Event,
    project_root: &Path,
    cache: &IndexCache,
) -> bool {
    event
        .paths
        .iter()
        .any(|p| !path_should_skip_for_index(p, project_root, cache))
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

#[cfg(unix)]
type DirectoryId = (u64, u64);
#[cfg(windows)]
type DirectoryId = (u64, u128);

fn directory_id(dir: &Dir) -> io::Result<DirectoryId> {
    #[cfg(unix)]
    {
        let metadata = dir.dir_metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        crate::windows_file_id::file_id(&dir.try_clone()?.into_std_file())
    }
}

struct ProjectRoot {
    path: PathBuf,
    id: Option<DirectoryId>,
}

impl ProjectRoot {
    fn new(path: &Path, dir: &Dir) -> Self {
        Self {
            path: path.to_owned(),
            id: directory_id(dir).ok(),
        }
    }

    fn same_directory(&self, other: &Self) -> bool {
        self.id.is_some() && self.id == other.id && self.path == other.path
    }
}

struct ActiveIndex {
    snapshot: Arc<ProjectIndexSnapshot>,
    // The allocation identifies one watch registration, even if a user
    // switches away and back to exactly the same directory.
    binding: Option<Arc<ProjectRoot>>,
    watcher: Option<RecommendedWatcher>,
}

impl ActiveIndex {
    fn is_bound_to(&self, binding: &Arc<ProjectRoot>) -> bool {
        self.binding
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, binding))
    }

    fn watches(&self, project: &ProjectRoot) -> bool {
        self.watcher.is_some()
            && self
                .binding
                .as_ref()
                .is_some_and(|active| active.same_directory(project))
    }
}

struct IndexState {
    active: Mutex<ActiveIndex>,
    // Serialize scans and changes of selection without blocking search readers.
    writer: Mutex<()>,
}

pub struct ProjectIndexStore {
    inner: Arc<IndexState>,
}

impl Default for ProjectIndexStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(IndexState {
                active: Mutex::new(ActiveIndex {
                    snapshot: Arc::new(default_index(PathBuf::new())),
                    binding: None,
                    watcher: None,
                }),
                writer: Mutex::new(()),
            }),
        }
    }
}

impl ProjectIndexStore {
    pub fn initialize_for_project(&self, project_root: PathBuf) {
        let _writer = self.inner.writer.lock().unwrap();
        let dir = match open_project_dir(&project_root) {
            Ok(dir) => dir,
            Err(error) => {
                eprintln!(
                    "[codemux::index] Failed to open {}: {error}",
                    project_root.display()
                );
                self.replace_snapshot(default_index(project_root));
                return;
            }
        };
        let binding = Arc::new(ProjectRoot::new(&project_root, &dir));
        if self.inner.active.lock().unwrap().watches(&binding) {
            return;
        }
        let cache = IndexCache::for_project(&project_root, &dir);
        let cached = cache.load(&project_root, &dir);
        let pending = self.restore_cached_index(binding.clone(), cached);
        let weak = Arc::downgrade(&self.inner);
        std::thread::spawn(move || {
            let witness = match retain_for_watching(&dir) {
                Ok(witness) => witness,
                Err(error) => {
                    eprintln!("[codemux::index] Failed to retain watched directory: {error}");
                    if let (Some(state), Some(expected)) = (weak.upgrade(), pending.as_ref()) {
                        refresh_index(&state, &binding, || Ok(dir), &cache, Some(expected));
                    }
                    return;
                }
            };
            let registered = watch_project(&binding, &dir);
            drop(dir);
            let events = {
                let Some(state) = weak.upgrade() else { return };
                let mut active = state.active.lock().unwrap();
                if !active.is_bound_to(&binding) {
                    return;
                }
                registered.map(|(watcher, events)| {
                    active.watcher = Some(watcher);
                    events
                })
            };
            if let (Some(state), Some(expected)) = (weak.upgrade(), pending.as_ref()) {
                refresh_index(
                    &state,
                    &binding,
                    || open_for_watched_scan(&witness),
                    &cache,
                    Some(expected),
                );
            }
            if let Some(events) = events {
                watch_changes(&weak, &binding, &cache, witness, events);
            }
        });
    }

    fn restore_cached_index(
        &self,
        binding: Arc<ProjectRoot>,
        cached: Option<ProjectIndexSnapshot>,
    ) -> Option<Arc<ProjectIndexSnapshot>> {
        let missing = cached.is_none();
        let snapshot = Arc::new(cached.unwrap_or_else(|| default_index(binding.path.clone())));
        self.select(ActiveIndex {
            snapshot: snapshot.clone(),
            binding: Some(binding),
            watcher: None,
        });
        missing.then_some(snapshot)
    }

    fn select(&self, active: ActiveIndex) {
        let previous = std::mem::replace(&mut *self.inner.active.lock().unwrap(), active);
        drop(previous);
    }

    pub fn snapshot(&self) -> Arc<ProjectIndexSnapshot> {
        self.inner.active.lock().unwrap().snapshot.clone()
    }

    fn replace_snapshot(&self, snapshot: ProjectIndexSnapshot) {
        self.select(ActiveIndex {
            snapshot: Arc::new(snapshot),
            binding: None,
            watcher: None,
        });
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

pub fn rebuild_index(
    store: &ProjectIndexStore,
    project_root: Option<String>,
) -> Result<Arc<ProjectIndexSnapshot>, String> {
    let _writer = store.inner.writer.lock().unwrap();
    let root = resolve_project_root(project_root)?;
    let dir = open_project_dir(&root).map_err(|error| {
        format!(
            "Failed to open project directory {}: {error}",
            root.display()
        )
    })?;
    let cache = IndexCache::for_project(&root, &dir);
    let binding = Arc::new(ProjectRoot::new(&root, &dir));
    let reuse = store.inner.active.lock().unwrap().watches(&binding);
    // A fresh registration buffers concurrent edits until its snapshot is published.
    let registration = if reuse {
        None
    } else {
        match retain_for_watching(&dir) {
            Ok(witness) => {
                watch_project(&binding, &dir).map(|(watcher, events)| (watcher, (witness, events)))
            }
            Err(error) => {
                eprintln!("[codemux::index] Failed to retain watched directory: {error}");
                None
            }
        }
    };
    let rebuild = prepare_rebuild(&root, &dir, &cache)?;
    drop(dir);
    let (watcher, events) = registration.unzip();
    let publication = if reuse {
        Publication::Manual
    } else {
        Publication::Select {
            binding: binding.clone(),
            watcher,
        }
    };
    let snapshot = rebuild.publish(&store.inner, publication);
    if let Some((witness, events)) = events {
        let weak = Arc::downgrade(&store.inner);
        std::thread::spawn(move || watch_changes(&weak, &binding, &cache, witness, events));
    }
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

fn register_index_watcher(
    root: &Path,
) -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<notify::Event>>)> {
    let (tx, events) = channel();
    let mut watcher = RecommendedWatcher::new(tx, Config::default().with_follow_symlinks(false))?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok((watcher, events))
}

fn retain_for_watching(dir: &Dir) -> io::Result<fs::File> {
    let handle = dir.try_clone()?.into_std_file();
    #[cfg(windows)]
    {
        reopen_watched_dir(&handle, true)
    }
    #[cfg(unix)]
    {
        Ok(handle)
    }
}

fn open_for_watched_scan(witness: &fs::File) -> io::Result<Dir> {
    #[cfg(windows)]
    let handle = reopen_watched_dir(witness, false)?;
    #[cfg(unix)]
    let handle = witness.try_clone()?;
    Ok(Dir::from_std_file(handle))
}

struct ProjectEvents {
    root: PathBuf,
    rx: Receiver<notify::Result<notify::Event>>,
}

fn watch_project(project: &ProjectRoot, _dir: &Dir) -> Option<(RecommendedWatcher, ProjectEvents)> {
    let register = || -> notify::Result<_> {
        // Keep the selected directory pinned until the OS watcher has opened it.
        #[cfg(windows)]
        let root = pinned_windows_path(_dir)?;
        #[cfg(unix)]
        let root = project.path.canonicalize()?;
        let (watcher, rx) = register_index_watcher(&root)?;
        if let Some(id) = project.id {
            if directory_id(&open_project_dir(&root)?)? != id {
                return Err(notify::Error::generic(
                    "Watched directory changed during registration",
                ));
            }
        }
        Ok((watcher, ProjectEvents { root, rx }))
    };
    register()
        .map_err(|error| {
            eprintln!(
                "[codemux::index] Failed to watch {}: {error}",
                project.path.display()
            );
        })
        .ok()
}

fn watch_changes(
    store: &Weak<IndexState>,
    binding: &Arc<ProjectRoot>,
    cache: &IndexCache,
    witness: fs::File,
    events: ProjectEvents,
) {
    let rx = &events.rx;
    let is_current = || {
        store
            .upgrade()
            .is_some_and(|state| state.active.lock().unwrap().is_bound_to(binding))
    };
    while let Ok(event) = rx.recv() {
        if !is_current() {
            return;
        }
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
        if !event_has_relevant_paths(&event, &events.root, cache) {
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
                    if !is_current() {
                        return;
                    }
                    if !ev.paths.is_empty()
                        && matches!(
                            ev.kind,
                            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                        )
                        && event_has_relevant_paths(&ev, &events.root, cache)
                    {
                        deadline = Instant::now() + INDEX_REBUILD_DEBOUNCE;
                    }
                }
                Ok(Err(error)) => eprintln!("[codemux::index] Watch error: {error}"),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }

        let Some(state) = store.upgrade() else { return };
        refresh_index(
            &state,
            binding,
            || open_for_watched_scan(&witness),
            cache,
            None,
        );
    }
}

fn refresh_index(
    store: &IndexState,
    binding: &Arc<ProjectRoot>,
    open: impl FnOnce() -> io::Result<Dir>,
    cache: &IndexCache,
    expected: Option<&Arc<ProjectIndexSnapshot>>,
) {
    let _writer = store.writer.lock().unwrap();
    let baseline = {
        let active = store.active.lock().unwrap();
        if !active.is_bound_to(binding)
            || expected.is_some_and(|snapshot| !Arc::ptr_eq(&active.snapshot, snapshot))
        {
            return;
        }
        active.snapshot.clone()
    };
    let dir = match open() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("[codemux::index] Failed to reopen watched directory: {error}");
            return;
        }
    };
    let rebuild = prepare_rebuild(&binding.path, &dir, cache);
    drop(dir);
    match rebuild {
        Ok(rebuild) => {
            rebuild.publish(
                store,
                Publication::Watch {
                    binding,
                    expected: &baseline,
                },
            );
        }
        Err(error) => eprintln!("[codemux::index] Failed to refresh index: {error}"),
    }
}

fn index_cache_dir() -> io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(root) = crate::test_isolation::isolated_root() {
        return Ok(root
            .join("cache")
            .join(crate::APP_DIR_NAME)
            .join("project-index"));
    }
    let cache = dirs::cache_dir()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("No application cache directory"))?;
    Ok(cache.join(crate::APP_DIR_NAME).join("project-index"))
}

fn index_cache_path(cache_dir: &Path, root_path: &Path, root: &Dir) -> io::Result<PathBuf> {
    let mut hash = Sha256::new();
    let (volume, directory) = directory_id(root)?;
    hash.update(volume.to_le_bytes());
    hash.update(directory.to_le_bytes());
    // The selected path distinguishes aliases. The handle distinguishes a worktree
    // replaced at the same path while an old watcher is publishing its last scan.
    hash.update(root_path.as_os_str().as_encoded_bytes());
    Ok(cache_dir.join(format!("{:x}.index", hash.finalize())))
}

/// Cache files normally live outside the workspace. When indexing a parent
/// such as the home directory, skip the cache subtree and keep the index in memory.
enum IndexCache {
    File(PathBuf),
    ExcludedSubtree(PathBuf),
    Unavailable,
}

impl IndexCache {
    fn for_project(project: &Path, dir: &Dir) -> Self {
        index_cache_dir()
            .and_then(|base| Self::at(&base, project, dir))
            .unwrap_or_else(|error| {
                eprintln!("[codemux::index] Disk cache unavailable: {error}");
                Self::Unavailable
            })
    }

    fn at(base: &Path, project: &Path, dir: &Dir) -> io::Result<Self> {
        if let Ok(relative) = base.strip_prefix(project) {
            return Ok(Self::ExcludedSubtree(relative.to_owned()));
        }
        let root = project.canonicalize()?;
        let cache_dir = resolve_cache_dir(base)?;
        if let Ok(relative) = cache_dir.strip_prefix(&root) {
            return Ok(Self::ExcludedSubtree(relative.to_owned()));
        }
        if root.starts_with(&cache_dir) {
            return Ok(Self::ExcludedSubtree(PathBuf::new()));
        }
        index_cache_path(&cache_dir, project, dir).map(Self::File)
    }

    fn excludes(&self, relative: &Path) -> bool {
        matches!(self, Self::ExcludedSubtree(subtree) if relative.starts_with(subtree))
    }

    fn load(&self, project: &Path, root: &Dir) -> Option<ProjectIndexSnapshot> {
        match self {
            Self::File(path) => load_cached_index(path, project, root),
            _ => None,
        }
    }
}

// A fresh installation may have no cache directory yet. Resolve the existing
// ancestor before testing containment so an alias to the workspace is recognized.
fn resolve_cache_dir(path: &Path) -> io::Result<PathBuf> {
    match path.canonicalize() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let Some((parent, name)) = path.parent().zip(path.file_name()) else {
                return Err(error);
            };
            Ok(resolve_cache_dir(parent)?.join(name))
        }
        result => result,
    }
}

#[derive(Serialize, Deserialize)]
struct CachedIndex<T> {
    generation: Option<(i64, i64)>,
    snapshot: T,
}

// A Unix inode may be reused after its checkout is deleted. On filesystems
// without birth time, store the current change time with each saved snapshot
// so an ordinary directory edit never creates a second cache file.
fn cache_generation(_root: &Dir) -> io::Result<Option<(i64, i64)>> {
    #[cfg(unix)]
    {
        let meta = _root.dir_metadata()?;
        let birth = meta
            .created()
            .ok()
            .and_then(|t| t.into_std().duration_since(std::time::UNIX_EPOCH).ok());
        Ok(Some(match birth {
            Some(t) => (t.as_secs() as i64, i64::from(t.subsec_nanos())),
            None => (meta.ctime(), meta.ctime_nsec()),
        }))
    }
    #[cfg(windows)]
    {
        Ok(None)
    } // The cache key already includes the full Windows file ID.
}

fn load_cached_index(
    cache_path: &Path,
    root_path: &Path,
    root: &Dir,
) -> Option<ProjectIndexSnapshot> {
    let dir = open_project_dir(cache_path.parent()?).ok()?;
    let (mut file, _) = open_regular_file(&dir, Path::new(cache_path.file_name()?)).ok()?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents).ok()?;
    let cached: CachedIndex<ProjectIndexSnapshot> = serde_json::from_slice(&contents).ok()?;
    (cached.generation == cache_generation(root).ok()?
        && cached.snapshot.schema_version == INDEX_SCHEMA_VERSION
        && cached.snapshot.project_root == root_path.to_string_lossy())
    .then_some(cached.snapshot)
}

#[cfg(test)]
fn rebuild_for_root(
    store: &ProjectIndexStore,
    project_root: &Path,
    root: &Dir,
    cache: &IndexCache,
) -> Result<Arc<ProjectIndexSnapshot>, String> {
    let rebuild = prepare_rebuild(project_root, root, cache)?;
    Ok(rebuild.publish(
        &store.inner,
        Publication::Select {
            binding: Arc::new(ProjectRoot::new(project_root, root)),
            watcher: None,
        },
    ))
}

enum Publication<'a> {
    Manual,
    Select {
        binding: Arc<ProjectRoot>,
        watcher: Option<RecommendedWatcher>,
    },
    Watch {
        binding: &'a Arc<ProjectRoot>,
        expected: &'a Arc<ProjectIndexSnapshot>,
    },
}

struct PreparedIndex<'a> {
    snapshot: ProjectIndexSnapshot,
    cache: Option<(tempfile::NamedTempFile, &'a Path)>,
}

impl PreparedIndex<'_> {
    fn publish(
        self,
        store: &IndexState,
        publication: Publication<'_>,
    ) -> Arc<ProjectIndexSnapshot> {
        let mut active = store.active.lock().unwrap();
        let retired = match publication {
            Publication::Watch { binding, expected } => {
                if !active.is_bound_to(binding) || !Arc::ptr_eq(&active.snapshot, expected) {
                    return active.snapshot.clone();
                }
                None
            }
            Publication::Select { binding, watcher } => {
                active.binding = Some(binding);
                std::mem::replace(&mut active.watcher, watcher)
            }
            Publication::Manual => None,
        };
        if let Some((file, path)) = self.cache {
            if let Err(error) = file.persist(path) {
                eprintln!("[codemux::index] Failed to save disk cache: {error}");
            }
        }
        let snapshot = Arc::new(self.snapshot);
        let previous = std::mem::replace(&mut active.snapshot, snapshot.clone());
        drop(active);
        drop(retired);
        drop(previous);
        snapshot
    }
}

fn prepare_rebuild<'a>(
    project_root: &Path,
    root: &Dir,
    cache: &'a IndexCache,
) -> Result<PreparedIndex<'a>, String> {
    let mut files = Vec::new();
    let mut chunks = Vec::new();
    let mut seen_dirs = HashSet::new();
    let mut total_bytes: usize = 0;
    scan_directory(
        root,
        Path::new(""),
        cache,
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

    let snapshot = ProjectIndexSnapshot {
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
    };
    let staged = match cache {
        IndexCache::File(path) => match stage_index(path, root, &snapshot) {
            Ok(file) => Some((file, path.as_path())),
            Err(error) => {
                eprintln!("[codemux::index] Failed to stage disk cache: {error}");
                None
            }
        },
        _ => None,
    };
    Ok(PreparedIndex {
        snapshot,
        cache: staged,
    })
}

fn scan_directory(
    dir: &Dir,
    relative_dir: &Path,
    cache: &IndexCache,
    files: &mut Vec<IndexedFile>,
    chunks: &mut Vec<IndexedChunk>,
    seen_dirs: &mut HashSet<(u64, u64)>,
    total_bytes: &mut usize,
) -> Result<(), String> {
    if cache.excludes(relative_dir) {
        return Ok(());
    }
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
        if cache.excludes(&path) {
            continue;
        }
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
                scan_directory(&child, &path, cache, files, chunks, seen_dirs, total_bytes)?;
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

fn stage_index(
    path: &Path,
    root: &Dir,
    snapshot: &ProjectIndexSnapshot,
) -> io::Result<tempfile::NamedTempFile> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing cache directory"))?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    let mut staged = tempfile::Builder::new()
        .suffix(".tmp")
        .tempfile_in(directory)?;
    {
        let mut writer = BufWriter::new(staged.as_file_mut());
        serde_json::to_writer_pretty(
            &mut writer,
            &CachedIndex {
                generation: cache_generation(root)?,
                snapshot,
            },
        )?;
        writer.flush()?;
    }
    Ok(staged)
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

    fn wait_for_indexed_text(store: &ProjectIndexStore, query: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while search_index(store, query, None).is_empty() {
            assert!(
                Instant::now() < deadline,
                "index did not refresh for {query}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn startup_rebuilds_an_old_cache_without_a_file_change() {
        if crate::test_isolation::isolated(
            "indexing::tests::startup_rebuilds_an_old_cache_without_a_file_change",
        ) {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::write(root.join("main.rs"), "fn searchable() {}").unwrap();
        let dir = open_project_dir(&root).unwrap();
        let cache = IndexCache::for_project(&root, &dir);
        let IndexCache::File(path) = &cache else {
            panic!("expected a private cache path")
        };
        let mut old = default_index(root.clone());
        old.schema_version = INDEX_SCHEMA_VERSION - 1;
        stage_index(path, &dir, &old)
            .unwrap()
            .persist(path)
            .unwrap();

        let store = ProjectIndexStore::default();
        assert!(cache.load(&root, &dir).is_none());
        store.initialize_for_project(root.clone());
        wait_for_indexed_text(&store, "searchable");
        let hits = search_index(&store, "searchable", None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_path, "main.rs");
        let cached = cache.load(&root, &dir).unwrap();
        drop(store);
        let restored = ProjectIndexStore::default();
        let binding = Arc::new(ProjectRoot::new(&root, &dir));
        assert!(restored
            .restore_cached_index(binding, Some(cached))
            .is_none());
        assert_eq!(search_index(&restored, "searchable", None).len(), 1);
        drop(dir);
        project.close().unwrap();
    }

    #[test]
    fn late_startup_scan_preserves_a_manual_rebuild_in_memory_and_on_disk() {
        if crate::test_isolation::isolated(
            "indexing::tests::late_startup_scan_preserves_a_manual_rebuild_in_memory_and_on_disk",
        ) {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let source = root.join("main.rs");
        fs::write(&source, "fn original() {}").unwrap();
        let dir = open_project_dir(root).unwrap();
        let cache = IndexCache::for_project(root, &dir);
        let store = ProjectIndexStore::default();
        let binding = Arc::new(ProjectRoot::new(root, &dir));
        let expected = store.restore_cached_index(binding.clone(), None).unwrap();
        let late = prepare_rebuild(root, &dir, &cache).unwrap();

        fs::write(source, "fn newer() {}").unwrap();
        let published = rebuild_index(&store, Some(root.display().to_string())).unwrap();
        late.publish(
            &store.inner,
            Publication::Watch {
                binding: &binding,
                expected: &expected,
            },
        );
        assert!(Arc::ptr_eq(&published, &store.snapshot()));
        assert_eq!(search_index(&store, "newer", None).len(), 1);
        assert_eq!(
            search_snapshot(&cache.load(root, &dir).unwrap(), "newer", 1).len(),
            1
        );
    }

    #[test]
    fn editing_a_watched_file_refreshes_the_searchable_index() {
        if crate::test_isolation::isolated(
            "indexing::tests::editing_a_watched_file_refreshes_the_searchable_index",
        ) {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let source = root.join("main.rs");
        fs::write(&source, "fn original() {}").unwrap();
        let store = ProjectIndexStore::default();
        store.initialize_for_project(root);
        wait_for_indexed_text(&store, "original");
        fs::write(&source, "fn updated() {}").unwrap();
        wait_for_indexed_text(&store, "updated");
        assert!(search_index(&store, "original", None).is_empty());
        let hits = search_index(&store, "updated", None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_path, "main.rs");
        drop(store);
        project.close().unwrap();
    }

    #[test]
    fn replacing_a_watched_checkout_retires_queued_events_and_watches_the_replacement() {
        if crate::test_isolation::isolated(
            "indexing::tests::replacing_a_watched_checkout_retires_queued_events_and_watches_the_replacement",
        ) {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        let base = project.path().canonicalize().unwrap();
        let selected = base.join("selected");
        let moved = base.join("moved");
        fs::create_dir(&selected).unwrap();
        let source = selected.join("main.rs");
        fs::write(&source, "fn former() {}").unwrap();
        let store = ProjectIndexStore::default();
        let path = selected.display().to_string();
        rebuild_index(&store, Some(path.clone())).unwrap();
        let old_binding = {
            let active = store.inner.active.lock().unwrap();
            assert!(active.watcher.is_some());
            active.binding.clone().unwrap()
        };
        let old_dir = open_project_dir(&selected).unwrap();
        let old_cache = IndexCache::for_project(&selected, &old_dir);
        let old_watch = retain_for_watching(&old_dir).unwrap();
        drop(old_dir);

        fs::rename(&selected, &moved).unwrap();
        fs::create_dir(&selected).unwrap();
        fs::write(&source, "fn replacement() {}").unwrap();
        let rebuilt = rebuild_index(&store, Some(path)).unwrap();
        assert_eq!(search_index(&store, "replacement", None).len(), 1);

        // Deliver the old worker's queued refresh only after the new snapshot is current.
        let mut opened = false;
        refresh_index(
            &store.inner,
            &old_binding,
            || {
                opened = true;
                open_for_watched_scan(&old_watch)
            },
            &old_cache,
            None,
        );
        assert!(!opened, "retired watcher attempted to reopen its root");
        assert!(Arc::ptr_eq(&rebuilt, &store.snapshot()));
        assert!(search_index(&store, "former", None).is_empty());
        drop(old_watch);

        fs::write(&source, "fn edited() {}").unwrap();
        wait_for_indexed_text(&store, "edited");
        assert!(search_index(&store, "replacement", None).is_empty());
        drop(store);
        project.close().unwrap();
    }

    #[test]
    fn cache_is_used_only_for_its_selected_root_and_schema() {
        let project = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let root = project.path();
        let dir = open_project_dir(root).unwrap();
        let cache = IndexCache::at(cache_dir.path(), root, &dir).unwrap();
        let IndexCache::File(path) = &cache else {
            panic!("expected a private cache path")
        };
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        let mut snapshot = Arc::unwrap_or_clone(
            rebuild_for_root(&ProjectIndexStore::default(), root, &dir, &cache).unwrap(),
        );
        assert_eq!(cache.load(root, &dir).unwrap().file_count, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }

        let stale = CachedIndex {
            generation: Some((i64::MIN, 0)),
            snapshot: &snapshot,
        };
        fs::write(path, serde_json::to_vec(&stale).unwrap()).unwrap();
        assert!(cache.load(root, &dir).is_none());

        snapshot.schema_version = INDEX_SCHEMA_VERSION - 1;
        stage_index(path, &dir, &snapshot)
            .unwrap()
            .persist(path)
            .unwrap();
        assert!(cache.load(root, &dir).is_none());

        snapshot.schema_version = INDEX_SCHEMA_VERSION;
        snapshot.project_root = root.join("previous-location").display().to_string();
        stage_index(path, &dir, &snapshot)
            .unwrap()
            .persist(path)
            .unwrap();
        assert!(cache.load(root, &dir).is_none());
    }

    #[test]
    fn rebuilding_an_index_leaves_a_clean_linked_worktree_removable() {
        if crate::test_isolation::isolated(
            "indexing::tests::rebuilding_an_index_leaves_a_clean_linked_worktree_removable",
        ) {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        let repo = project.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let git = |directory: &Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(directory)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        git(&repo, &["init", "-q"]);
        fs::write(repo.join("main.rs"), "fn main() {}").unwrap();
        git(&repo, &["add", "main.rs"]);
        git(&repo, &["commit", "-qm", "initial"]);
        let exclude = repo.join(".git/info/exclude");
        fs::write(&exclude, "*.local\n").unwrap();
        git(
            &repo,
            &["worktree", "add", "--detach", "../checkout", "HEAD"],
        );
        let checkout = project.path().join("checkout");
        let snapshot = rebuild_index(
            &ProjectIndexStore::default(),
            Some(checkout.display().to_string()),
        )
        .unwrap();
        assert_eq!(snapshot.file_count, 1);
        let dir = open_project_dir(&checkout).unwrap();
        assert_eq!(
            IndexCache::for_project(&checkout, &dir)
                .load(&checkout, &dir)
                .unwrap()
                .file_count,
            1
        );
        assert!(git(
            &checkout,
            &["status", "--porcelain", "--untracked-files=all"]
        )
        .is_empty());
        assert!(!checkout.join(".codemux").exists());
        assert_eq!(fs::read(&exclude).unwrap(), b"*.local\n");
        crate::git::ensure_worktree_removable(&checkout).unwrap();
    }

    #[test]
    fn a_workspace_containing_the_cache_skips_its_contents_and_events() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let dir = open_project_dir(&root).unwrap();
        let cache_dir = root.join("user-cache/project-index");
        let cache = IndexCache::at(&cache_dir, &root, &dir).unwrap();
        fs::write(root.join("main.rs"), "fn main() {}").unwrap();
        assert_eq!(
            rebuild_for_root(&ProjectIndexStore::default(), &root, &dir, &cache)
                .unwrap()
                .file_count,
            1
        );
        assert!(!cache_dir.exists());
        fs::create_dir_all(&cache_dir).unwrap();
        let cached = cache_dir.join("source.json");
        fs::write(&cached, "{}").unwrap();
        let snapshot =
            rebuild_for_root(&ProjectIndexStore::default(), &root, &dir, &cache).unwrap();
        assert_eq!(snapshot.file_count, 1);
        assert_eq!(snapshot.files[0].file_path, "main.rs");
        let event = notify::Event::new(EventKind::Any).add_path(cached);
        assert!(!event_has_relevant_paths(&event, &root, &cache));
        assert!(event_has_relevant_paths(
            &event.add_path(root.join("main.rs")),
            &root,
            &cache
        ));
    }

    #[cfg(unix)]
    #[test]
    fn renamed_directory_uses_its_own_scan_and_cache() {
        let project = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let selected = project.path().join("selected");
        let moved = project.path().join("moved");
        fs::create_dir_all(selected.join("src")).unwrap();
        fs::write(selected.join("src/original.rs"), "fn original() {}").unwrap();
        let parent = open_project_dir(project.path()).unwrap();
        let root = open_child_dir(&parent, Path::new("selected")).unwrap();
        let original = IndexCache::at(cache_dir.path(), &selected, &root).unwrap();
        fs::rename(&selected, &moved).unwrap();
        fs::create_dir(&selected).unwrap();

        let snapshot =
            rebuild_for_root(&ProjectIndexStore::default(), &selected, &root, &original).unwrap();
        assert_eq!(snapshot.file_count, 1);
        assert_eq!(snapshot.files[0].file_path, "src/original.rs");
        assert_eq!(original.load(&selected, &root).unwrap().file_count, 1);
        let replacement = open_project_dir(&selected).unwrap();
        let cache = IndexCache::at(cache_dir.path(), &selected, &replacement).unwrap();
        assert!(cache.load(&selected, &replacement).is_none());
        assert_eq!(
            rebuild_for_root(
                &ProjectIndexStore::default(),
                &selected,
                &replacement,
                &cache
            )
            .unwrap()
            .file_count,
            0
        );
        assert_eq!(original.load(&selected, &root).unwrap().file_count, 1);
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
        let snapshot = rebuild_for_root(
            &ProjectIndexStore::default(),
            &selected,
            &pinned,
            &IndexCache::Unavailable,
        )
        .unwrap();
        assert_eq!(snapshot.file_count, 1);
        assert_eq!(snapshot.files[0].file_path, "original.rs");
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

        let cache_dir = tempfile::tempdir().unwrap();
        let handle = open_project_dir(&root).unwrap();
        let cache = IndexCache::at(cache_dir.path(), &root, &handle).unwrap();
        let snapshot =
            rebuild_for_root(&ProjectIndexStore::default(), &root, &handle, &cache).unwrap();
        assert_eq!(snapshot.file_count, 1);
        assert_eq!(snapshot.files[0].file_path, "src/main.rs");
        assert!(cache.load(&root, &handle).is_some());
    }

    #[test]
    fn legacy_workspace_cache_is_ignored_and_left_in_place() {
        let project = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let root = project.path();
        let dir = open_project_dir(root).unwrap();
        let old_dir = root.join(".codemux");
        fs::create_dir(&old_dir).unwrap();
        let path = old_dir.join("index.json");
        let old = serde_json::to_vec(&default_index(root.to_owned())).unwrap();
        fs::write(&path, &old).unwrap();
        let cache = IndexCache::at(cache_dir.path(), root, &dir).unwrap();
        assert!(cache.load(root, &dir).is_none());
        fs::write(root.join("main.rs"), "fn main() {}").unwrap();
        assert_eq!(
            rebuild_for_root(&ProjectIndexStore::default(), root, &dir, &cache)
                .unwrap()
                .file_count,
            1
        );
        assert_eq!(fs::read(&path).unwrap(), old);
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
