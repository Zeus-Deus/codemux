use super::ProjectScripts;
use rusqlite::Connection;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr;
use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

/// Older Settings rows used the caller's path spelling. Only consult
/// spellings derived from the verified project, rather than resolving old
/// keys that might refer to a symlink since retargeted at this directory.
pub(super) fn read_legacy(conn: &Connection, project_root: &Path) -> Option<ProjectScripts> {
    let short = short_path(project_root);
    let mut statement = conn
        .prepare("SELECT key, value FROM settings WHERE user_id = 'local' AND key GLOB 'project.scripts:*'")
        .ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()?;
    let mut saved: Option<ProjectScripts> = None;
    for row in rows {
        let (key, value) = row.ok()?;
        let candidate = Path::new(key.strip_prefix("project.scripts:")?);
        if matches_project_root(candidate, project_root, short.as_deref()) {
            let scripts: ProjectScripts = serde_json::from_str(&value).ok()?;
            if saved.as_ref().is_some_and(|current| current != &scripts) {
                // Ambiguous legacy approval. A new Settings save uses the
                // canonical key and resolves the ambiguity explicitly.
                return None;
            }
            saved = Some(scripts);
        }
    }
    saved
}

fn matches_project_root(candidate: &Path, long: &Path, short: Option<&Path>) -> bool {
    let candidate: Vec<_> = candidate.components().collect();
    let long: Vec<_> = long.components().collect();
    if candidate.len() != long.len() {
        return false;
    }
    let short = short.map(|path| path.components().collect::<Vec<_>>());
    let short = short.as_ref().filter(|pieces| pieces.len() == long.len());
    candidate
        .iter()
        .zip(long)
        .enumerate()
        .all(|(index, (candidate, long))| {
            same_component(*candidate, long)
                || short.is_some_and(|pieces| same_component(*candidate, pieces[index]))
        })
}

fn same_component(left: Component<'_>, right: Component<'_>) -> bool {
    match (left, right) {
        (Component::Prefix(left), Component::Prefix(right)) => match (left.kind(), right.kind()) {
            (
                Prefix::Disk(a) | Prefix::VerbatimDisk(a),
                Prefix::Disk(b) | Prefix::VerbatimDisk(b),
            ) => a.eq_ignore_ascii_case(&b),
            (
                Prefix::UNC(a, b) | Prefix::VerbatimUNC(a, b),
                Prefix::UNC(c, d) | Prefix::VerbatimUNC(c, d),
            ) => a == c && b == d,
            (left, right) => left == right,
        },
        _ => left == right,
    }
}

fn short_path(path: &Path) -> Option<PathBuf> {
    let wide: Vec<_> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: the input is a NUL-terminated UTF-16 path. A null output
    // with size zero requests the required length, including the NUL.
    let required = unsafe { GetShortPathNameW(wide.as_ptr(), ptr::null_mut(), 0) };
    if required == 0 {
        return None;
    }
    let mut output = vec![0u16; required as usize];
    // SAFETY: the output buffer holds the requested number of UTF-16 units.
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), output.as_mut_ptr(), required) };
    if written == 0 || written >= required {
        return None;
    }
    Some(OsString::from_wide(&output[..written as usize]).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_windows_keys_match_mixed_short_long_and_verbatim_components() {
        let long = Path::new(r"C:\Users\runneradmin\AppData\Local\Temp\.tmpFixture\Project");
        let short = Path::new(r"C:\Users\RUNNER~1\AppData\Local\Temp\TMPFIX~1\Project");
        for alias in [
            r"C:\Users\RUNNER~1\AppData\Local\Temp\.tmpFixture\Project",
            r"\\?\C:\Users\runneradmin\AppData\Local\Temp\.tmpFixture\Project",
            "c:/Users/runneradmin/AppData/Local/Temp/TMPFIX~1/Project",
        ] {
            assert!(
                matches_project_root(Path::new(alias), long, Some(short)),
                "{alias}"
            );
        }
        for unrelated in [
            "C:/Users/runneradmin/AppData/Local/Temp/.tmpFixture/project",
            "C:/Users/runneradmin/AppData/Local/Temp/Link/Project",
            "D:/Users/runneradmin/AppData/Local/Temp/.tmpFixture/Project",
        ] {
            assert!(
                !matches_project_root(Path::new(unrelated), long, Some(short)),
                "{unrelated}"
            );
        }
    }
}
