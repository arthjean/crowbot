use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use super::BoxedStream;
use crate::Endpoint;

pub struct Listener {
    inner: UnixListener,
    owner: u32,
    path: PathBuf,
    /// The device and the inode of the socket file this listener bound.
    file: (u64, u64),
    /// The lock of the socket, held for the life of the listener.
    _lock: std::fs::File,
}

/// A listener that ends removes its socket file, while it still holds the
/// lock: no other daemon can have bound the path since, and a client then
/// finds no socket rather than one that refuses it.
impl Drop for Listener {
    fn drop(&mut self) {
        let ours = std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.file);
        if ours {
            std::fs::remove_file(&self.path).ok();
        }
    }
}

impl Listener {
    /// Claims the socket. A socket file outlives the process that bound it,
    /// so a stale one is removed first, and only by the holder of the lock
    /// file `<socket>.lock`: two daemons that start at once cannot both pass
    /// the test of a live socket, and the second cannot remove the socket
    /// that the first has bound and does not listen on yet. The lock file is
    /// never removed, so that every daemon locks the same file.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        if let Some(length) = endpoint.too_long() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the socket path {endpoint} is {length} bytes long, over the limit of {}",
                    crate::SOCKET_PATH_LIMIT
                ),
            ));
        }
        let path = Path::new(endpoint.address());
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&lock_path)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(io::ErrorKind::AddrInUse.into()),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        if std::fs::symlink_metadata(path).is_ok() {
            // A program that answers holds no lock: an older daemon, or
            // another program. It is not removed.
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(io::ErrorKind::AddrInUse.into());
            }
            std::fs::remove_file(path)?;
        }
        let inner = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(Self {
            inner,
            owner: metadata.uid(),
            path: path.to_owned(),
            file: (metadata.dev(), metadata.ino()),
            _lock: lock,
        })
    }

    pub async fn accept(&mut self) -> io::Result<BoxedStream> {
        loop {
            let (stream, _) = self.inner.accept().await?;
            let same_user = stream
                .peer_cred()
                .is_ok_and(|peer| peer.uid() == self.owner);
            if same_user {
                return Ok(Box::new(stream));
            }
        }
    }
}

/// Connects to the daemon, and checks that it runs as this user before
/// anything is sent: a socket another account bound could read every message.
pub async fn connect(endpoint: &Endpoint) -> io::Result<BoxedStream> {
    let stream = UnixStream::connect(endpoint.address()).await?;
    // SAFETY: `geteuid` has no preconditions.
    let user = unsafe { libc::geteuid() };
    let server = stream.peer_cred()?.uid();
    if server != user {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the program on {endpoint} runs as another account (uid {server})"),
        ));
    }
    Ok(Box::new(stream))
}
