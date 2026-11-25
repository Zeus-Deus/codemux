use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, MetadataExt, OpenOptions, OpenOptionsExt};
use std::fs::File;
use std::io::{self, ErrorKind};
use std::path::Path;

pub(super) struct ProjectSettingsDir {
    root: Dir,
    directory: Dir,
}

impl ProjectSettingsDir {
    pub(super) fn open(root: &Path) -> io::Result<Self> {
        let root = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
        let directory = root.open_dir_nofollow(".claude")?;
        Ok(Self { root, directory })
    }

    fn check_binding(&self) -> io::Result<()> {
        let current = self
            .root
            .open_dir_nofollow(".claude")
            .and_then(|directory| directory.dir_metadata())
            .map_err(|_| super::settings_changed())?;
        let held = self.directory.dir_metadata()?;
        if current.dev() != held.dev() || current.ino() != held.ino() {
            return Err(super::settings_changed());
        }
        Ok(())
    }

    pub(super) fn open_file(&self, name: &str) -> io::Result<File> {
        super::validate_project_settings_name(name)?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK);
        let file = self.directory.open_with(name, &options)?.into_std();
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Settings must be a regular file",
            ));
        }
        self.check_binding()?;
        Ok(file)
    }
}
