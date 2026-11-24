use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use super::BoxedStream;
use crate::Endpoint;

pub struct Listener {
    inner: UnixListener,
    owner: u32,
    /// The socket file, with no file for an abstract socket.
    _file: Option<SocketFile>,
}

struct SocketFile {
    path: PathBuf,
    /// The device and the inode of the socket file this listener bound.
    id: (u64, u64),
    /// The lock of the socket, held for the life of the listener.
    _lock: std::fs::File,
}

/// A listener that ends removes its socket file, while it still holds the
/// lock: no other daemon can have bound the path since, and a client then
/// finds no socket rather than one that refuses it.
impl Drop for SocketFile {
    fn drop(&mut self) {
        let ours = std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.id);
        if ours {
            std::fs::remove_file(&self.path).ok();
        }
    }
}

/// The account this process runs as.
fn own_user() -> u32 {
    // SAFETY: `geteuid` has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(target_os = "linux")]
fn abstract_address(name: &str) -> io::Result<std::os::unix::net::SocketAddr> {
    use std::os::linux::net::SocketAddrExt;
    std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
}

impl Listener {
    /// Claims the socket. A socket file outlives the process that bound it,
    /// so a stale one is removed first, and only by the holder of the lock
    /// file `<socket>.lock`: two daemons that start at once cannot both pass
    /// the test of a live socket, and the second cannot remove the socket
    /// that the first has bound and does not listen on yet. The lock file is
    /// never removed, so that every daemon locks the same file.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        if let Some(name) = endpoint.abstract_name() {
            // A name that another socket holds is refused by the system.
            let listener = std::os::unix::net::UnixListener::bind_addr(&abstract_address(name)?)?;
            listener.set_nonblocking(true)?;
            return Ok(Self {
                inner: UnixListener::from_std(listener)?,
                owner: own_user(),
                _file: None,
            });
        }
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
            _file: Some(SocketFile {
                path: path.to_owned(),
                id: (metadata.dev(), metadata.ino()),
                _lock: lock,
            }),
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
    connect_as(endpoint, own_user()).await
}

/// Connects as `user`: the account the daemon must run as. Tests give
/// another one, so that the check runs with no second account.
async fn connect_as(endpoint: &Endpoint, user: u32) -> io::Result<BoxedStream> {
    #[cfg(target_os = "linux")]
    let stream = match endpoint.abstract_name() {
        Some(name) => {
            let stream = std::os::unix::net::UnixStream::connect_addr(&abstract_address(name)?)?;
            stream.set_nonblocking(true)?;
            UnixStream::from_std(stream)?
        }
        None => UnixStream::connect(endpoint.address()).await?,
    };
    #[cfg(not(target_os = "linux"))]
    let stream = UnixStream::connect(endpoint.address()).await?;
    let server = stream.peer_cred()?.uid();
    if server != user {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the program on {endpoint} runs as another account (uid {server})"),
        ));
    }
    Ok(Box::new(stream))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use super::*;
    use crate::scratch::Scratch;

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }

    fn me() -> u32 {
        // SAFETY: `geteuid` has no preconditions.
        unsafe { libc::geteuid() }
    }

    #[tokio::test]
    async fn the_socket_and_its_lock_are_private_and_so_is_a_folder_the_listener_made() {
        let scratch = Scratch::new("crowbot-unix-test");
        let folder = scratch.join("made");
        let socket = folder.join("crowbotd.sock");
        let endpoint = Endpoint::new(socket.to_string_lossy().into_owned());
        let _listener = Listener::bind(&endpoint).unwrap();
        assert_eq!(mode(&socket), 0o600);
        assert_eq!(mode(&folder.join("crowbotd.sock.lock")), 0o600);
        assert_eq!(mode(&folder), 0o700);
    }

    /// The listener is told it runs as another account, so the connection of
    /// the test, from the real one, comes from another user.
    #[tokio::test]
    async fn a_peer_of_another_user_is_dropped_before_anything_is_read() {
        let scratch = Scratch::new("crowbot-unix-test");
        let endpoint = scratch.endpoint();
        let mut listener = Listener::bind(&endpoint).unwrap();
        listener.owner = me().wrapping_add(1);
        let accepting = tokio::spawn(async move {
            let accepted = listener.accept().await.map(|_| ());
            (accepted, listener)
        });
        let address = endpoint.address().to_owned();
        let ended = tokio::task::spawn_blocking(move || {
            let mut peer = std::os::unix::net::UnixStream::connect(address).unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            // The listener never reads this: it closes the connection first.
            peer.write_all(b"{\"type\":\"hello\",\"protocol_version\":1}\n")
                .ok();
            let mut answer = [0_u8; 1];
            match peer.read(&mut answer) {
                Ok(0) => Ok(()),
                Ok(_) => Err("the listener answered".to_owned()),
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => Ok(()),
                Err(error) => Err(format!("the connection stayed open: {error}")),
            }
        })
        .await
        .unwrap();
        assert_eq!(ended, Ok(()));
        // The listener goes on waiting for a peer of its own account.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!accepting.is_finished(), "the foreign peer was accepted");
        accepting.abort();
    }

    #[tokio::test]
    async fn a_peer_of_the_same_user_is_accepted() {
        let scratch = Scratch::new("crowbot-unix-test");
        let endpoint = scratch.endpoint();
        let mut listener = Listener::bind(&endpoint).unwrap();
        let accepting = tokio::spawn(async move { listener.accept().await.map(|_| ()) });
        let _peer = UnixStream::connect(endpoint.address()).await.unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(10), accepting).await;
        assert!(matches!(accepted, Ok(Ok(Ok(())))), "{accepted:?}");
    }

    /// The client is told it runs as another account, so the server of the
    /// test, from the real one, runs as another user.
    #[tokio::test]
    async fn a_client_closes_a_server_of_another_user_before_it_sends_anything() {
        let scratch = Scratch::new("crowbot-unix-test");
        let path = scratch.join("server.sock");
        let server = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let endpoint = Endpoint::new(path.to_string_lossy().into_owned());
        let refused = connect_as(&endpoint, me().wrapping_add(1)).await;
        let error = refused.err().expect("the client kept the connection");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("another account"), "{error}");
        let (mut accepted, _) = server.accept().unwrap();
        accepted
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut received = Vec::new();
        accepted.read_to_end(&mut received).unwrap();
        assert!(received.is_empty(), "the client sent {received:?}");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn an_abstract_socket_leaves_no_file_refuses_a_second_listener_and_ends_with_its_listener()
     {
        let name = format!("crowbot-unix-test-{}", std::process::id());
        let endpoint = Endpoint::abstract_socket(&name);
        let mut listener = Listener::bind(&endpoint).unwrap();
        assert!(Listener::bind(&endpoint).is_err());
        let accepting = tokio::spawn(async move {
            let accepted = listener.accept().await.map(|_| ());
            (accepted, listener)
        });
        let _peer = connect(&endpoint).await.unwrap();
        let (accepted, listener) = tokio::time::timeout(Duration::from_secs(10), accepting)
            .await
            .unwrap()
            .unwrap();
        assert!(accepted.is_ok());
        assert!(!Path::new(endpoint.address()).exists());
        drop(listener);
        assert!(connect(&endpoint).await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_peer_of_another_user_on_an_abstract_socket_is_dropped() {
        let name = format!("crowbot-unix-test-other-{}", std::process::id());
        let endpoint = Endpoint::abstract_socket(&name);
        let mut listener = Listener::bind(&endpoint).unwrap();
        listener.owner = me().wrapping_add(1);
        let accepting = tokio::spawn(async move { listener.accept().await.map(|_| ()) });
        let refused = connect_as(&endpoint, me()).await;
        let mut peer = match refused {
            Ok(peer) => peer,
            Err(error) => panic!("cannot connect: {error}"),
        };
        let mut answer = [0_u8; 1];
        let read = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::io::AsyncReadExt::read(&mut peer, &mut answer),
        )
        .await
        .unwrap();
        assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!accepting.is_finished(), "the foreign peer was accepted");
        accepting.abort();
    }

    #[tokio::test]
    async fn a_stale_socket_with_no_lock_holder_is_replaced() {
        let scratch = Scratch::new("crowbot-unix-test");
        let endpoint = scratch.endpoint();
        drop(std::os::unix::net::UnixListener::bind(endpoint.address()).unwrap());
        assert!(Path::new(endpoint.address()).exists());
        let mut listener = Listener::bind(&endpoint).unwrap();
        let accepting = tokio::spawn(async move { listener.accept().await.map(|_| ()) });
        let _peer = UnixStream::connect(endpoint.address()).await.unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(10), accepting).await;
        assert!(matches!(accepted, Ok(Ok(Ok(())))), "{accepted:?}");
    }
}
