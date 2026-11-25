//! Owner-restricted Windows named pipe for the local attention hook.

use std::ffi::c_void;
use std::io;
use std::mem::{self, MaybeUninit};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::slice;

use rand::{rngs::OsRng, RngCore};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, SECURITY_MAX_SID_SIZE, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(super) struct Listener {
    endpoint: String,
    path: String,
    owner_dacl: Vec<u16>,
    pending: NamedPipeServer,
}

impl Listener {
    pub(super) fn bind() -> io::Result<Self> {
        let endpoint = random_endpoint()?;
        let path = format!(r"\\.\pipe\{endpoint}");
        let owner_dacl = current_user_dacl()?;
        let pending = create_pipe(&path, &owner_dacl, true)?;
        Ok(Self {
            endpoint,
            path,
            owner_dacl,
            pending,
        })
    }

    /// The bare name used by PowerShell's NamedPipeClientStream.
    pub(super) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(super) async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        // Retain the pending instance while awaiting so cancellation leaves
        // its protected endpoint in place.
        self.pending.connect().await?;
        let next = create_pipe(&self.path, &self.owner_dacl, false)?;
        Ok(mem::replace(&mut self.pending, next))
    }
}

fn random_endpoint() -> io::Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut entropy = [0u8; 16];
    OsRng
        .try_fill_bytes(&mut entropy)
        .map_err(io::Error::other)?;
    let mut endpoint = String::from("codemux-hook-");
    for byte in entropy {
        endpoint.push(char::from(HEX[(byte >> 4) as usize]));
        endpoint.push(char::from(HEX[(byte & 0xf) as usize]));
    }
    Ok(endpoint)
}

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: Both SID conversion APIs return LocalAlloc-owned buffers.
        unsafe { LocalFree(self.0) };
    }
}

#[repr(C)]
struct TokenUserBuffer {
    user: TOKEN_USER,
    sid: [u8; SECURITY_MAX_SID_SIZE as usize],
}

fn current_user_dacl() -> io::Result<Vec<u16>> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess yields a valid pseudo-handle, and token is
    // writable storage for the resulting process token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: A successful OpenProcessToken transfers its token handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };

    let mut buffer = MaybeUninit::<TokenUserBuffer>::uninit();
    let mut written = 0;
    // SAFETY: The aligned buffer can hold TOKEN_USER plus the largest SID.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            mem::size_of::<TokenUserBuffer>() as u32,
            &mut written,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Success initializes TOKEN_USER and its SID within buffer. Read
    // only the reported SID pointer, leaving the unused storage uninitialized.
    let sid = unsafe { ptr::addr_of!((*buffer.as_ptr()).user.User.Sid).read() };
    let mut sid_text = ptr::null_mut();
    // SAFETY: sid is valid until buffer goes out of scope and sid_text is a
    // writable output pointer. On success the returned UTF-16 string is NUL
    // terminated and owned by the caller.
    if unsafe { ConvertSidToStringSidW(sid, &mut sid_text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(sid_text.cast());
    let mut length = 0;
    // SAFETY: ConvertSidToStringSidW returns a NUL-terminated wide string.
    let sid = unsafe {
        while *sid_text.add(length) != 0 {
            length += 1;
        }
        slice::from_raw_parts(sid_text, length)
    };

    // Set the owner explicitly because an elevated token's default owner can
    // be an administrator group. The protected DACL grants only the current
    // user access, including creation of subsequent server instances.
    let mut dacl: Vec<u16> = "O:".encode_utf16().collect();
    dacl.extend_from_slice(sid);
    dacl.extend("D:P(A;;GA;;;".encode_utf16());
    dacl.extend_from_slice(sid);
    dacl.extend([u16::from(b')'), 0]);
    Ok(dacl)
}

fn create_pipe(path: &str, owner_dacl: &[u16], first: bool) -> io::Result<NamedPipeServer> {
    let mut descriptor = ptr::null_mut();
    // SAFETY: owner_dacl is the NUL-terminated SDDL created above, and
    // descriptor is writable storage for the Windows-owned result.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            owner_dacl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(descriptor);
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: The descriptor stays allocated and attributes remains live
    // throughout the synchronous CreateNamedPipeW call.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(path, ptr::addr_of_mut!(attributes).cast())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::windows::named_pipe::ClientOptions;

    #[tokio::test]
    async fn first_instance_guard_and_owner_accepts_repeated_connections() {
        let mut listener = Listener::bind().unwrap();
        let suffix = listener.endpoint().strip_prefix("codemux-hook-").unwrap();
        assert_eq!(suffix.len(), 32);
        assert!(suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
        assert!(create_pipe(&listener.path, &listener.owner_dacl, true).is_err());
        let _first_client = ClientOptions::new().open(&listener.path).unwrap();
        let _first_server = listener.accept().await.unwrap();
        let _second_client = ClientOptions::new().open(&listener.path).unwrap();
        let _second_server = listener.accept().await.unwrap();
    }
}
