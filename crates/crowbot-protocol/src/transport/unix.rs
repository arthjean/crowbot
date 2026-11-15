use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use tokio::net::{UnixListener, UnixStream};

use super::BoxedStream;
use crate::Endpoint;

pub struct Listener {
    inner: UnixListener,
    owner: u32,
    /// The lock of the socket, held for the life of the listener.
    _lock: std::fs::File,
}

impl Listener {
    /// Claims the socket. A socket file outlives the process that bound it,
    /// so a stale one is removed first, and only by the holder of the lock
    /// file `<socket>.lock`: two daemons that start at once cannot both pass
    /// the test of a live socket, and the second cannot remove the socket
    /// that the first has bound and does not listen on yet. The lock file is
    /// never removed, so that every daemon locks the same file.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
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
        let owner = std::fs::metadata(path)?.uid();
        Ok(Self {
            inner,
            owner,
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
