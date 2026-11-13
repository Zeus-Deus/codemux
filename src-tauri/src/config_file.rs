//! Small, safe replacement of config files Codemux edits or owns.
//!
//! Every change is staged in a uniquely named sibling file, flushed, and
//! renamed over the target, so a reader never sees a half-written file and
//! two writers never share a staging file. An edit of a file someone else
//! also writes (an agent's own config) passes the bytes it was planned from:
//! if the file changed meanwhile, nothing is replaced and the caller can plan
//! again. That check narrows the race with an uncooperative writer; it cannot
//! close it.

use std::io::{ErrorKind, Write as _};
use std::path::Path;

/// The file's bytes, or `None` when it does not exist. Any other read error
/// is returned, so a caller never mistakes an unreadable file for a missing one.
pub fn read_existing(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Write `contents` to `path` unless it already holds exactly them. Returns
/// whether it wrote. Creates the parent directory when needed.
pub fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<bool> {
    let current = read_existing(path)?;
    if current.as_deref() == Some(contents) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    replace_if_unchanged(path, current.as_deref(), contents)?;
    Ok(true)
}

/// Atomically replace `path` with `contents`, provided it still holds
/// `original` (`None`: it must still be missing). The existing file's
/// permissions are kept. A file that changed meanwhile is left alone and
/// reported as an error that says so.
pub fn replace_if_unchanged(path: &Path, original: Option<&[u8]>, contents: &[u8]) -> std::io::Result<()> {
    let parent = parent_of(path)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut staged = tempfile::Builder::new().prefix(&format!(".{name}.")).suffix(".tmp").tempfile_in(parent)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        staged.as_file().set_permissions(metadata.permissions())?;
    }
    staged.write_all(contents)?;
    staged.as_file().sync_all()?;
    ensure_unchanged(path, original)?;
    // Dropping `staged` on any error above removes the staging file.
    staged.persist(path).map_err(|error| error.error)?;
    sync_dir(parent);
    Ok(())
}

/// Delete `path`, provided it still holds `original`.
pub fn remove_if_unchanged(path: &Path, original: &[u8]) -> std::io::Result<()> {
    ensure_unchanged(path, Some(original))?;
    std::fs::remove_file(path)?;
    sync_dir(parent_of(path)?);
    Ok(())
}

fn ensure_unchanged(path: &Path, original: Option<&[u8]>) -> std::io::Result<()> {
    if read_existing(path)?.as_deref() == original {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("{} changed while it was being updated", path.display())))
    }
}

fn parent_of(path: &Path) -> std::io::Result<&Path> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Ok(parent),
        _ => Ok(Path::new(".")),
    }
}

/// Makes a rename durable across power loss. Best effort; Unix only.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(dir) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn unchanged_content_is_never_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        std::fs::write(&path, b"{}").unwrap();
        let before = std::fs::metadata(&path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!write_if_changed(&path, b"{}").unwrap());
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(before.ino(), after.ino());
        }
    }

    #[test]
    fn changed_content_replaces_the_file_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/a.json");
        assert!(write_if_changed(&path, b"one").unwrap());
        assert!(write_if_changed(&path, b"two").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(leftovers(path.parent().unwrap()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn replacement_keeps_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(write_if_changed(&path, b"new").unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_file_that_changed_since_it_was_read_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        std::fs::write(&path, b"theirs").unwrap();
        let error = replace_if_unchanged(&path, Some(b"planned from"), b"ours").unwrap_err();
        assert!(error.to_string().contains("changed while"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"theirs");
        assert!(remove_if_unchanged(&path, b"planned from").is_err());
        assert!(path.exists());
        assert!(leftovers(dir.path()).is_empty(), "the staging file is cleaned up");
        // A file planned as missing must still be missing.
        let fresh = dir.path().join("b.json");
        std::fs::write(&fresh, b"appeared").unwrap();
        assert!(replace_if_unchanged(&fresh, None, b"ours").is_err());
        assert_eq!(std::fs::read(&fresh).unwrap(), b"appeared");
    }

    #[test]
    fn concurrent_writers_never_share_a_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let contents = format!("writer-{i}-").repeat(4096);
                    // Each either wins or sees another's file; none tears.
                    let _ = replace_if_unchanged(&path, read_existing(&path).unwrap().as_deref(), contents.as_bytes());
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        let first = text.split('-').take(2).collect::<Vec<_>>().join("-") + "-";
        assert_eq!(text, first.repeat(4096), "one writer's whole content");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn unreadable_is_not_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_existing(&dir.path().join("absent")).unwrap().is_none());
        // A directory where a file is expected is an error, not "missing".
        assert!(read_existing(dir.path()).is_err());
    }
}
