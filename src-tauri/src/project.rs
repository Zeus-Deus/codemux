use std::env;
use std::path::PathBuf;

pub fn current_project_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    if cwd.file_name().and_then(|name| name.to_str()) == Some("src-tauri") {
        if let Some(parent) = cwd.parent() {
            let parent = parent.to_path_buf();
            if parent.join("package.json").exists() {
                return parent;
            }
        }
    }

    cwd
}
