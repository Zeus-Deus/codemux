use std::{fs::File, io, os::windows::io::AsRawHandle};
use windows_sys::Win32::Storage::FileSystem::{
    FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
};

/// Return the volume serial number and full 128-bit file ID of an open handle.
pub(crate) fn file_id(handle: &File) -> io::Result<(u64, u128)> {
    let mut info = std::mem::MaybeUninit::<FILE_ID_INFO>::uninit();
    // SAFETY: the handle stays open and the buffer has the required size and alignment.
    let success = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileIdInfo,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful call initialized the buffer.
    let info = unsafe { info.assume_init() };
    Ok((
        info.VolumeSerialNumber,
        u128::from_le_bytes(info.FileId.Identifier),
    ))
}
