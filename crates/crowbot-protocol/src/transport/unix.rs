use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::Path;

use tokio::net::{UnixListener, UnixStream};

use super::BoxedStream;
use crate::Endpoint;

pub struct Listener {
    inner: UnixListener,
    owner: u32,
}

impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        let path = Path::new(endpoint.address());
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(io::ErrorKind::AddrInUse.into());
            }
            std::fs::remove_file(path)?;
        }
        if let Some(parent) = path.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let inner = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        let owner = std::fs::metadata(path)?.uid();
        Ok(Self { inner, owner })
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

pub async fn connect(endpoint: &Endpoint) -> io::Result<BoxedStream> {
    Ok(Box::new(UnixStream::connect(endpoint.address()).await?))
}
