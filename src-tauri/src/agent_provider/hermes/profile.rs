//! Version 1 of the read-only Hermes profile layout contract. Never reads profile contents.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Profile {
    pub schema_version: u32,
    pub host: String,
    pub installation: PathBuf,
    pub root: PathBuf,
    pub id: String,
    pub home: PathBuf,
    /// Filesystem identity detects replacement at the same pathname without writing a marker.
    pub identity: String,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub fn default_root() -> PathBuf {
    let home = std::env::var_os("HERMES_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".hermes"));
    if home
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|p| p == "profiles")
    {
        home.parent().and_then(Path::parent).unwrap().to_path_buf()
    } else {
        home
    }
}

fn identity(path: &Path) -> Result<String, String> {
    let m = std::fs::metadata(path)
        .map_err(|_| "repair_required: Hermes profile is missing".to_string())?;
    if !m.is_dir() {
        return Err("repair_required: Hermes profile is not a directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("{}:{}", m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        m.created()
            .map(|t| format!("{:?}", t))
            .map_err(|_| "unsupported: filesystem cannot identify profile replacement".into())
    }
}

/// Canonical executable path for a configured installation (bare names use PATH).
pub fn resolve_installation(installation: &Path) -> Result<PathBuf, String> {
    if installation.components().count() == 1 {
        which::which(installation).map_err(|_| {
            "setup_required: install official Hermes with ACP dependencies".to_string()
        })?
    } else {
        installation.to_path_buf()
    }
    .canonicalize()
    .map_err(|_| "setup_required: Hermes executable is missing".to_string())
}

pub fn resolve(installation: &Path, root: &Path, id: &str) -> Result<Profile, String> {
    if !valid_id(id) {
        return Err("unsupported: invalid Hermes profile ID".into());
    }
    let root = root
        .canonicalize()
        .map_err(|_| "setup_required: select an existing Hermes root".to_string())?;
    let path = if id == "default" {
        root.clone()
    } else {
        root.join("profiles").join(id)
    };
    if id != "default" && root.join("profiles/.deleted").join(id).exists() {
        return Err("repair_required: Hermes profile was deleted; restore it in Hermes".into());
    }
    let home = path
        .canonicalize()
        .map_err(|_| "repair_required: Hermes profile is missing".to_string())?;
    let installation = resolve_installation(installation)?;
    Ok(Profile {
        schema_version: 1,
        host: "local".into(),
        installation,
        root,
        id: id.into(),
        identity: identity(&home)?,
        home,
    })
}

impl Profile {
    pub fn validate(&self) -> Result<(), String> {
        if self.host != "local" || self.schema_version != 1 {
            return Err("unsupported: Hermes binding host/layout version".into());
        }
        if resolve(&self.installation, &self.root, &self.id)? != *self {
            return Err("repair_required: Hermes profile moved or was replaced; restore its original identity".into());
        }
        Ok(())
    }
}

pub fn discover(installation: &Path, root: &Path) -> Result<Vec<Profile>, String> {
    let mut ids = vec!["default".to_string()];
    if let Ok(entries) = std::fs::read_dir(root.join("profiles")) {
        ids.extend(
            entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|id| valid_id(id) && id != "default"),
        );
    }
    ids.sort();
    let mut out = Vec::new();
    for id in ids {
        if id != "default" && root.join("profiles/.deleted").join(&id).exists() {
            continue;
        }
        out.push(resolve(installation, root, &id)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hermes_profile_identity_and_tombstones() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("profiles/coder")).unwrap();
        let binary = std::env::current_exe().unwrap();
        let p = resolve(&binary, root, "coder").unwrap();
        p.validate().unwrap();
        assert!(resolve(&binary, root, "../coder").is_err());
        std::fs::rename(&p.home, root.join("old")).unwrap();
        std::fs::create_dir(&p.home).unwrap();
        assert!(p.validate().unwrap_err().contains("replaced"));
        std::fs::create_dir(root.join("profiles/.deleted")).unwrap();
        std::fs::write(root.join("profiles/.deleted/coder"), "deleted").unwrap();
        assert_eq!(discover(&binary, root).unwrap().len(), 1);
        assert!(resolve(&binary, root, "coder").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn hermes_profiles_are_canonical_and_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("custom-root");
        std::fs::create_dir_all(root.join("profiles/coder")).unwrap();
        std::fs::write(root.join("active_profile"), "coder").unwrap();
        let alias = tmp.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let binary = std::env::current_exe().unwrap();
        assert_eq!(
            resolve(&binary, &alias, "coder").unwrap(),
            resolve(&binary, &root, "coder").unwrap()
        );
        assert_eq!(discover(&binary, &root).unwrap().len(), 2);
        assert!(resolve(&binary, &root, "missing").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("active_profile")).unwrap(),
            "coder"
        );
        for invalid in ["", "/tmp", "../escape", "UPPER", "a.b"] {
            assert!(!valid_id(invalid));
        }
    }
}
