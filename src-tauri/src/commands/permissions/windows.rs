use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FileRenameInformation, NtCreateFile, NtSetInformationFile, FILE_CREATE, FILE_DIRECTORY_FILE,
    FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
    FILE_RENAME_INFORMATION_0, FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{RtlNtStatusToDosError, UNICODE_STRING};
use windows_sys::Win32::Storage::FileSystem::{
    FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_TRAVERSE, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0};

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

pub(super) struct ProjectSettingsDir(File);

impl ProjectSettingsDir {
    pub(super) fn open(root: &Path) -> io::Result<Self> {
        // The user-selected root may itself be a link. Resolve it once,
        // then open every descendant relative to a directory handle.
        let root = OpenOptions::new()
            .access_mode(FILE_GENERIC_READ | FILE_TRAVERSE)
            .share_mode(SHARE_ALL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(root)?;
        let dir = open_relative(
            &root,
            ".claude",
            FILE_GENERIC_READ | FILE_TRAVERSE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            FILE_DIRECTORY_FILE,
        )?;
        let metadata = dir.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "The project settings directory must be a regular directory",
            ));
        }
        Ok(Self(dir))
    }

    pub(super) fn open_file(&self, name: &str) -> io::Result<File> {
        self.open_regular_file(name, SHARE_ALL)
    }

    fn open_regular_file(&self, name: &str, share: u32) -> io::Result<File> {
        super::validate_project_settings_name(name)?;
        let file = open_relative(
            &self.0,
            name,
            FILE_GENERIC_READ,
            share,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        )?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Settings must be a regular file",
            ));
        }
        Ok(file)
    }

    pub(super) fn replace(&self, name: &str, original: &[u8], content: &[u8]) -> io::Result<()> {
        let permissions = self.open_file(name)?.metadata()?.permissions();
        if permissions.readonly() {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "Settings file is read-only",
            ));
        }
        let temporary = format!(".settings-{}.tmp", uuid::Uuid::new_v4().simple());
        let mut staged = open_relative(
            &self.0,
            &temporary,
            FILE_GENERIC_WRITE | DELETE,
            0,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE,
        )?;
        let result = (|| {
            staged.write_all(content)?;
            staged.set_permissions(permissions)?;
            staged.sync_all()?;

            // Disallow writes or deletion while comparing bytes. The checked
            // handle must close before the filesystem can replace its name.
            let current = self.open_regular_file(name, FILE_SHARE_READ)?;
            let mut bytes = Vec::new();
            current
                .take((original.len() as u64).saturating_add(1))
                .read_to_end(&mut bytes)?;
            if bytes != original {
                return Err(super::settings_changed());
            }
            rename_in_place(&staged, name)
        })();
        if result.is_err() {
            discard(&staged);
        }
        result
    }
}

/// All callers provide a fixed child name or an exclusive staging basename.
fn open_relative(
    parent: &File,
    name: &str,
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
) -> io::Result<File> {
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    let length = (wide.len() * size_of::<u16>()) as u16;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: wide.as_mut_ptr(),
    };
    const OBJ_CASE_INSENSITIVE: u32 = 0x40;
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut status_block = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    let mut handle = ptr::null_mut();
    // SAFETY: the live parent handle and UTF-16 string are borrowed for the
    // synchronous call. The handle and status block are writable out-pointers.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut status_block,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            disposition,
            options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            ptr::null(),
            0,
        )
    };
    check_status(status)?;
    // SAFETY: NtCreateFile transferred ownership of the successful handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn rename_in_place(file: &File, name: &str) -> io::Result<()> {
    super::validate_project_settings_name(name)?;
    // A bare name and a null RootDirectory keep the renamed file in the
    // directory where its live handle was created. No pathname is reopened.
    let wide: Vec<u16> = name.encode_utf16().collect();
    let name_bytes = wide.len() * size_of::<u16>();
    let size = size_of::<FILE_RENAME_INFORMATION>() + name_bytes;
    let mut buffer = vec![0u64; size.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut status_block = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    // SAFETY: the buffer is aligned and large enough for the variable-length
    // rename record. The staging handle was opened with DELETE access.
    let status = unsafe {
        (*info).Anonymous = FILE_RENAME_INFORMATION_0 { ReplaceIfExists: 1 };
        (*info).RootDirectory = ptr::null_mut();
        (*info).FileNameLength = name_bytes as u32;
        ptr::copy_nonoverlapping(
            wide.as_ptr(),
            ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            wide.len(),
        );
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status_block,
            info.cast(),
            size as u32,
            FileRenameInformation,
        )
    };
    check_status(status)
}

fn discard(file: &File) {
    let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    // SAFETY: the staging handle is live and has DELETE access, and the
    // supplied pointer and size describe its deletion record.
    unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            ptr::addr_of!(info).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        );
    }
}

fn check_status(status: i32) -> io::Result<()> {
    if status >= 0 {
        return Ok(());
    }
    // SAFETY: the conversion takes an NTSTATUS value and no pointers.
    let error = unsafe { RtlNtStatusToDosError(status) };
    Err(io::Error::from_raw_os_error(error as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn project_settings_directory_cannot_be_renamed_while_held() {
        let project = tempfile::tempdir().unwrap();
        let settings = project.path().join(".claude");
        let moved = project.path().join("moved");
        fs::create_dir(&settings).unwrap();
        let selected = ProjectSettingsDir::open(project.path()).unwrap();
        assert!(fs::rename(&settings, &moved).is_err());
        drop(selected);
        fs::rename(settings, moved).unwrap();
    }
}
