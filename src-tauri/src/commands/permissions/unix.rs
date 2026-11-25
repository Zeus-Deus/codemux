use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::path::Path;

pub(super) struct ProjectSettingsDir(Dir);

impl ProjectSettingsDir {
    pub(super) fn open(root: &Path) -> io::Result<Self> {
        let root = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
        root.open_dir_nofollow(".claude").map(Self)
    }

    pub(super) fn open_file(&self, name: &str) -> io::Result<File> {
        super::validate_project_settings_name(name)?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK);
        let file = self.0.open_with(name, &options)?.into_std();
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Settings must be a regular file",
            ));
        }
        Ok(file)
    }

    pub(super) fn replace(&self, name: &str, original: &[u8], content: &[u8]) -> io::Result<()> {
        let permissions = self.open_file(name)?.metadata()?.permissions();
        let temporary = format!(".settings-{}.tmp", uuid::Uuid::new_v4().simple());
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            .follow(FollowSymlinks::No);
        let mut staged = self.0.open_with(&temporary, &options)?.into_std();
        let result = (|| {
            staged.write_all(content)?;
            staged.set_permissions(permissions)?;
            staged.sync_all()?;

            let mut current = Vec::new();
            self.open_file(name)?
                .take((original.len() as u64).saturating_add(1))
                .read_to_end(&mut current)?;
            if current != original {
                return Err(super::settings_changed());
            }
            self.0.rename(&temporary, &self.0, name)?;
            // Best effort, matching the user settings replacement path.
            let _ = self.0.open(".").and_then(|dir| dir.sync_all());
            Ok(())
        })();
        if result.is_err() {
            let _ = self.0.remove_file(&temporary);
        }
        result
    }
}
