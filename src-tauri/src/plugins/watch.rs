//! One recursive watcher on the plugins root. Changes are debounced and sent
//! to the main window as `plugins_changed { id }`, where `id` is the folder
//! that changed (or null when the root itself changed shape).

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

pub const CHANGED_EVENT: &str = "plugins_changed";
const DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Changed {
    pub id: Option<String>,
}

/// The folder (first path segment below root) an event path belongs to.
/// `None` for the root itself; staging/dot folders are ignored entirely.
pub fn changed_folder(root: &Path, path: &Path) -> Option<Option<String>> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = rel.components();
    let Some(first) = parts.next() else {
        return Some(None);
    };
    let name = first.as_os_str().to_string_lossy().to_string();
    if name.starts_with('.') {
        // `.staging-<id>` swaps land as a rename into `<id>`, which is reported
        // on its own; the staging churn itself is noise.
        return None;
    }
    Some(Some(name))
}

/// Start watching `root` (created if missing). The returned watcher must be
/// kept alive; dropping it stops the worker thread.
pub fn start(
    root: PathBuf,
    mut publish: impl FnMut(Vec<Changed>) + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let _ = std::fs::create_dir_all(&root);
    let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
    let (tx, rx) = mpsc::channel::<Event>();
    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<Event>| {
            if let Ok(event) = event {
                if !matches!(event.kind, EventKind::Access(_)) {
                    let _ = tx.send(event);
                }
            }
        },
        Config::default(),
    )?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    std::thread::Builder::new()
        .name("plugins-watch".into())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut events = vec![first];
                while let Ok(more) = rx.recv_timeout(DEBOUNCE) {
                    events.push(more);
                }
                let mut ids = BTreeSet::<Option<String>>::new();
                for event in events {
                    for path in &event.paths {
                        let folder = changed_folder(&root, path)
                            .or_else(|| changed_folder(&canonical, path));
                        if let Some(folder) = folder {
                            ids.insert(folder);
                        }
                    }
                }
                if !ids.is_empty() {
                    publish(ids.into_iter().map(|id| Changed { id }).collect());
                }
            }
        })
        .map_err(|e| notify::Error::generic(&e.to_string()))?;
    Ok(watcher)
}
