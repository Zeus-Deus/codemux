use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use tokio::net::{UnixListener, UnixStream};

pub(super) struct Listener {
    socket: UnixListener,
    _directory: tempfile::TempDir,
    endpoint: String,
}

impl Listener {
    /// Bind inside an active Tokio runtime. A short path under the system
    /// temporary directory fits Unix socket limits even with a long home path.
    pub(super) fn bind() -> io::Result<Self> {
        let parent = Path::new("/tmp"); // tmp-literal-ok
        let metadata = fs::metadata(parent)?;
        let mode = metadata.mode();
        if !metadata.is_dir() || metadata.uid() != 0 || (mode & 0o022 != 0 && mode & 0o1000 == 0) {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "Hook socket requires a protected system temporary directory",
            ));
        }

        let directory = tempfile::Builder::new()
            .prefix("cmx-hook-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)?;
        let path = directory.path().join("notify.sock");
        let socket = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let endpoint = path
            .into_os_string()
            .into_string()
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "Hook socket path is not UTF-8"))?;
        Ok(Self {
            socket,
            _directory: directory,
            endpoint,
        })
    }

    pub(super) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(super) async fn accept(&mut self) -> io::Result<UnixStream> {
        loop {
            let (stream, _) = self.socket.accept().await?;
            if crate::control_identity::local_peer_credentials(&stream).is_ok() {
                return Ok(stream);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn hook_socket_admits_an_owner_connection() {
        let mut listener = Listener::bind().unwrap();
        let mut client = UnixStream::connect(listener.endpoint()).await.unwrap();
        let mut peer = listener.accept().await.unwrap();
        client.write_all(b"fixture").await.unwrap();
        let mut received = [0; 7];
        peer.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"fixture");
    }

    #[tokio::test]
    async fn hook_socket_is_private_and_cleaned_up_on_drop() {
        let listener = Listener::bind().unwrap();
        let endpoint = Path::new(listener.endpoint()).to_owned();
        let directory = endpoint.parent().unwrap();
        assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
        let socket = fs::symlink_metadata(&endpoint).unwrap();
        assert!(socket.file_type().is_socket());
        assert_eq!(socket.mode() & 0o777, 0o600);

        drop(listener);
        assert!(!directory.exists());
    }
}
