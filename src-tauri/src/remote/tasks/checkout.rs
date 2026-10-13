//! Receiver-issued checkout identity and Linux directory-handle startup binding.
use serde::{Deserialize, Serialize};
use std::os::unix::{fs::{MetadataExt, OpenOptionsExt}, io::AsRawFd};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct CheckoutIdentity {
    pub workspace_id: String,
    pub canonical_path: PathBuf,
    pub device: u64,
    pub inode: u64,
}
pub(super) struct BoundCheckout {
    directory: std::fs::File,
    registry: Option<rusqlite::Connection>,
}
impl CheckoutIdentity {
    pub fn capture(workspace_id: String, path: &Path) -> Result<Self, String> {
        let canonical_path=path.canonicalize().map_err(|e|format!("checkout path: {e}"))?;
        let directory=open_directory(&canonical_path)?;
        let meta=directory.metadata().map_err(|e|e.to_string())?;
        Ok(Self{workspace_id,canonical_path,device:meta.dev(),inode:meta.ino()})
    }
    pub fn open_registered(&self, root: &Path, path: &Path) -> Result<BoundCheckout, String> {
        // Serialize registry removal/rebind with the actual native startup.
        let conn=rusqlite::Connection::open(super::super::config::database_path(root)).map_err(|e|e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(|e|e.to_string())?;
        conn.execute_batch("BEGIN IMMEDIATE").map_err(|e|e.to_string())?;
        let registered: String=conn.query_row("SELECT path FROM workspaces WHERE id=?1",[&self.workspace_id],|r|r.get(0)).map_err(|_|"Validated checkout was unregistered before native startup".to_string())?;
        if Path::new(&registered)!=path {return Err("Validated workspace registration changed before native startup".into());}
        let current=Self::capture(self.workspace_id.clone(),path)?;
        if current!=*self {return Err("Validated canonical checkout identity changed before native startup".into());}
        let directory=open_directory(&self.canonical_path)?;
        let meta=directory.metadata().map_err(|e|e.to_string())?;
        if (meta.dev(),meta.ino())!=(self.device,self.inode) {return Err("Checkout was replaced while acquiring native startup handle".into());}
        let bound=BoundCheckout{directory,registry:Some(conn)};
        // Refuse platforms without the stable parent-held FD path. Never fall
        // back to the mutable registered name when this facility is absent.
        if !bound.cwd().is_dir() {return Err("Stable checkout directory handle is unavailable on this host".into());}
        Ok(bound)
    }
}
fn open_directory(path: &Path) -> Result<std::fs::File,String> {
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY|libc::O_NOFOLLOW).open(path).map_err(|e|format!("Open validated checkout: {e}"))
}
impl BoundCheckout {
    pub fn cwd(&self) -> PathBuf {PathBuf::from(format!("/proc/{}/fd/{}",std::process::id(),self.directory.as_raw_fd()))}
    pub fn release_registry(&mut self) -> Result<(),String> {
        if let Some(conn)=self.registry.take() {conn.execute_batch("ROLLBACK").map_err(|e|e.to_string())?;}
        Ok(())
    }
}
