use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const DEFAULT_READ_BYTES: usize = 1024 * 1024;
const MAX_READ_BYTES: usize = 8 * 1024 * 1024;
const MAX_DATA_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FsOperation {
    Exists {
        path: String,
    },
    ListDir {
        path: String,
    },
    ReadText {
        path: String,
        #[serde(rename = "maxBytes")]
        max_bytes: Option<usize>,
    },
    DataDir,
    WriteDataFile {
        name: String,
        contents: String,
    },
    RemoveDataFile {
        name: String,
    },
}

pub(super) fn absolute_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty()
        || path.len() > 32768
        || path.contains('\0')
        || !Path::new(path).is_absolute()
    {
        return Err("Path must be absolute and refer to this machine".into());
    }
    Ok(PathBuf::from(path))
}
fn data_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 128
        || name.contains("..")
        || name.starts_with('.')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err("Data file name must be one segment of letters, digits, dots, underscores or dashes, without '..'".into());
    }
    // Windows device names remain special even with an extension.
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
    {
        return Err("Reserved data file name".into());
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("Plugin data directory must not be a link".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn data_dir(app_data: &Path, plugin: &str) -> Result<PathBuf, String> {
    if !super::super::valid_id(plugin) {
        return Err("Invalid plugin id".into());
    }
    fs::create_dir_all(app_data).map_err(|e| e.to_string())?;
    let app_data = fs::canonicalize(app_data).map_err(|e| e.to_string())?;
    let base = app_data.join("plugin-data");
    private_dir(&base)?;
    let base = fs::canonicalize(base).map_err(|e| e.to_string())?;
    let dir = base.join(plugin);
    private_dir(&dir)?;
    let canonical = fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    if canonical.parent() != Some(base.as_path()) {
        return Err("Plugin data path escapes its directory".into());
    }
    sweep_stale_temps(&canonical)?;
    Ok(canonical)
}
fn sweep_stale_temps(dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|name| name.strip_prefix(".tmp")) else {
            continue;
        };
        // tempfile's default writer name is .tmp plus six alphanumeric chars.
        if suffix.len() != 6 || !suffix.bytes().all(|b| b.is_ascii_alphanumeric()) {
            continue;
        }
        let meta = match fs::symlink_metadata(entry.path()) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if meta.is_file()
            && meta
                .modified()
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age > std::time::Duration::from_secs(86400))
        {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    Ok(())
}
fn data_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    data_name(name)?;
    let path = dir.join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err("Data file must be a regular file, without links".into());
            }
            let resolved = fs::canonicalize(&path).map_err(|e| e.to_string())?;
            if resolved.parent() != Some(dir) {
                return Err("Data file escapes plugin directory".into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(path)
}
fn read_text(path: &Path, max: Option<usize>) -> Result<String, String> {
    let max = max.unwrap_or(DEFAULT_READ_BYTES);
    if max == 0 || max > MAX_READ_BYTES {
        return Err("maxBytes must be between 1 and 8388608".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("readText requires a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err("Text file exceeds maxBytes".into());
    }
    String::from_utf8(bytes).map_err(|_| "File is not UTF-8 text".into())
}
fn execute_at(app_data: &Path, plugin: &str, operation: FsOperation) -> Result<Value, String> {
    match operation {
        FsOperation::Exists { path } => Ok(json!(absolute_path(&path)?
            .try_exists()
            .map_err(|e| e.to_string())?)),
        FsOperation::ListDir { path } => {
            let mut entries = Vec::new();
            for entry in fs::read_dir(absolute_path(&path)?).map_err(|e| e.to_string())? {
                if entries.len() >= 10000 {
                    return Err("Directory exceeds 10000 entries".into());
                }
                let entry = entry.map_err(|e| e.to_string())?;
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                entries.push(json!({"name":entry.file_name().to_string_lossy(), "kind": if kind.is_symlink() {"symlink"} else if kind.is_dir() {"directory"} else if kind.is_file() {"file"} else {"other"}}));
            }
            entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            Ok(json!(entries))
        }
        FsOperation::ReadText { path, max_bytes } => {
            Ok(json!(read_text(&absolute_path(&path)?, max_bytes)?))
        }
        FsOperation::DataDir => Ok(json!(data_dir(app_data, plugin)?.to_string_lossy())),
        FsOperation::WriteDataFile { name, contents } => {
            if contents.len() > MAX_DATA_BYTES {
                return Err("Data file exceeds 1 MiB".into());
            }
            let dir = data_dir(app_data, plugin)?;
            let path = data_path(&dir, &name)?;
            let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                temp.as_file()
                    .set_permissions(fs::Permissions::from_mode(0o600))
                    .map_err(|e| e.to_string())?;
            }
            temp.write_all(contents.as_bytes())
                .map_err(|e| e.to_string())?;
            temp.as_file().sync_all().map_err(|e| e.to_string())?;
            // Persist renames the temporary file over the destination atomically.
            // It never opens the destination, so a link cannot redirect the write.
            data_path(&dir, &name)?;
            temp.persist(&path).map_err(|e| e.to_string())?;
            Ok(json!(path.to_string_lossy()))
        }
        FsOperation::RemoveDataFile { name } => {
            let path = data_path(&data_dir(app_data, plugin)?, &name)?;
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
            Ok(Value::Null)
        }
    }
}
pub(super) fn execute(plugin: &str, operation: FsOperation) -> Result<Value, String> {
    let root = super::super::app_data_dir().ok_or("App data directory is unavailable")?;
    execute_at(&root, plugin, operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plugin_host_data_acquire_sweeps_only_stale_writer_temps() {
        let root = tempfile::tempdir().unwrap();
        let dir = data_dir(root.path(), "demo").unwrap();
        let stale = dir.join(".tmpABC123");
        let recent = dir.join(".tmpNEW123");
        let named = dir.join("policy.json");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400);
        for path in [&stale, &recent, &named] {
            fs::write(path, "fixture").unwrap();
        }
        for path in [&stale, &named] {
            fs::File::open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&named, dir.join(".tmpLNK123")).unwrap();
        assert_eq!(data_dir(root.path(), "demo").unwrap(), dir);
        assert!(!stale.exists());
        assert!(recent.exists());
        assert!(named.exists());
        #[cfg(unix)]
        assert!(fs::symlink_metadata(dir.join(".tmpLNK123"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
    #[test]
    fn plugin_host_data_files_are_bounded_private_and_retained_outside_code() {
        let root = tempfile::tempdir().unwrap();
        let write = |contents: &str| {
            execute_at(
                root.path(),
                "demo",
                FsOperation::WriteDataFile {
                    name: "policy.json".into(),
                    contents: contents.into(),
                },
            )
        };
        let path = PathBuf::from(write("one").unwrap().as_str().unwrap());
        assert!(path.starts_with(root.path().join("plugin-data/demo")));
        write("two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(write(&"x".repeat(MAX_DATA_BYTES + 1)).is_err());
        for name in ["../escape", "a/b", "a\\b", "..", "a..b", "CON.json"] {
            assert!(data_path(path.parent().unwrap(), name).is_err());
        }
        assert!(absolute_path("relative").is_err());
        assert!(read_text(&path, Some(2)).is_err());
        assert_eq!(read_text(&path, Some(3)).unwrap(), "two");
        execute_at(
            root.path(),
            "demo",
            FsOperation::RemoveDataFile {
                name: "policy.json".into(),
            },
        )
        .unwrap();
        assert!(!path.exists());
    }
    #[cfg(unix)]
    #[test]
    fn plugin_host_data_refuses_file_and_directory_links() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("target"), "unchanged").unwrap();
        let dir = data_dir(root.path(), "demo").unwrap();
        symlink(outside.path().join("target"), dir.join("link")).unwrap();
        assert!(execute_at(
            root.path(),
            "demo",
            FsOperation::WriteDataFile {
                name: "link".into(),
                contents: "bad".into()
            }
        )
        .is_err());
        assert!(execute_at(
            root.path(),
            "demo",
            FsOperation::RemoveDataFile {
                name: "link".into()
            }
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("target")).unwrap(),
            "unchanged"
        );
        symlink(outside.path(), root.path().join("plugin-data/linked")).unwrap();
        assert!(data_dir(root.path(), "linked").is_err());
    }
}
