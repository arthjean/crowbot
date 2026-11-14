use std::io;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf,
};

use crate::{Endpoint, WireError, decode, encode};

pub const CLIENT_MESSAGE_LIMIT: usize = 1 << 20;
pub const SERVER_MESSAGE_LIMIT: usize = 64 << 20;

trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> Stream for T {}

type BoxedStream = Box<dyn Stream>;

pub struct MessageReader {
    inner: BufReader<ReadHalf<BoxedStream>>,
    limit: usize,
    line: Vec<u8>,
}

impl MessageReader {
    pub async fn read<T: DeserializeOwned>(&mut self) -> Result<Option<T>, WireError> {
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                if self.line.is_empty() {
                    return Ok(None);
                }
                return Err(WireError::Io(io::ErrorKind::UnexpectedEof.into()));
            }
            let end = available.iter().position(|byte| *byte == b'\n');
            let chunk = &available[..end.unwrap_or(available.len())];
            if self.line.len() + chunk.len() > self.limit {
                return Err(WireError::TooLong { limit: self.limit });
            }
            self.line.extend_from_slice(chunk);
            let consumed = chunk.len() + usize::from(end.is_some());
            self.inner.consume(consumed);
            if end.is_some() {
                let line = std::mem::take(&mut self.line);
                return decode(&line).map(Some);
            }
        }
    }
}

pub struct MessageWriter {
    inner: WriteHalf<BoxedStream>,
}

impl MessageWriter {
    pub async fn write<T: Serialize>(&mut self, message: &T) -> Result<(), WireError> {
        let line = encode(message)?;
        self.inner.write_all(&line).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

fn split(stream: BoxedStream, limit: usize) -> (MessageReader, MessageWriter) {
    let (reader, writer) = tokio::io::split(stream);
    (
        MessageReader {
            inner: BufReader::new(reader),
            limit,
            line: Vec::new(),
        },
        MessageWriter { inner: writer },
    )
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    use super::BoxedStream;
    use crate::Endpoint;

    const ERROR_PIPE_BUSY: i32 = 231;

    pub struct Listener {
        address: String,
        next: NamedPipeServer,
    }

    impl Listener {
        pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
            let next = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(endpoint.address())?;
            Ok(Self {
                address: endpoint.address().to_owned(),
                next,
            })
        }

        pub async fn accept(&mut self) -> io::Result<BoxedStream> {
            let connected = self.next.connect().await;
            let fresh = ServerOptions::new()
                .reject_remote_clients(true)
                .create(&self.address)?;
            let stream = std::mem::replace(&mut self.next, fresh);
            connected?;
            Ok(Box::new(stream))
        }
    }

    pub async fn connect(endpoint: &Endpoint) -> io::Result<BoxedStream> {
        loop {
            match ClientOptions::new().open(endpoint.address()) {
                Ok(stream) => return Ok(Box::new(stream)),
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(unix)]
mod platform {
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
}

pub struct Listener {
    inner: platform::Listener,
}

impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        Ok(Self {
            inner: platform::Listener::bind(endpoint)?,
        })
    }

    pub async fn accept(&mut self) -> io::Result<(MessageReader, MessageWriter)> {
        let stream = self.inner.accept().await?;
        Ok(split(stream, CLIENT_MESSAGE_LIMIT))
    }
}

pub async fn connect(endpoint: &Endpoint) -> io::Result<(MessageReader, MessageWriter)> {
    let stream = platform::connect(endpoint).await?;
    Ok(split(stream, SERVER_MESSAGE_LIMIT))
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;
    use crate::{ClientMessage, ServerMessage};

    fn pair(limit: usize) -> (MessageReader, tokio::io::DuplexStream) {
        let (near, far) = tokio::io::duplex(64);
        let (reader, _) = split(Box::new(near), limit);
        (reader, far)
    }

    #[tokio::test]
    async fn messages_split_across_reads_come_back_whole_and_in_order() {
        let (mut reader, mut far) = pair(1024);
        let first = ClientMessage::SendMessage {
            conversation_id: 1,
            text: "a message longer than the sixty-four byte buffer of the test pipe, twice over"
                .to_owned(),
        };
        let second = ClientMessage::ListConversations;
        let mut bytes = encode(&first).unwrap();
        bytes.extend(encode(&second).unwrap());
        let writing = tokio::spawn(async move {
            far.write_all(&bytes).await.unwrap();
        });
        assert_eq!(reader.read::<ClientMessage>().await.unwrap(), Some(first));
        assert_eq!(reader.read::<ClientMessage>().await.unwrap(), Some(second));
        writing.await.unwrap();
        assert_eq!(reader.read::<ClientMessage>().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_line_over_the_limit_is_refused() {
        let (mut reader, mut far) = pair(16);
        tokio::spawn(async move {
            far.write_all(&[b'x'; 64]).await.ok();
        });
        assert!(matches!(
            reader.read::<ClientMessage>().await,
            Err(WireError::TooLong { limit: 16 })
        ));
    }

    #[tokio::test]
    async fn a_connection_cut_inside_a_message_is_an_error() {
        let (mut reader, mut far) = pair(1024);
        far.write_all(b"{\"type\":").await.unwrap();
        drop(far);
        assert!(matches!(
            reader.read::<ClientMessage>().await,
            Err(WireError::Io(_))
        ));
    }

    #[tokio::test]
    async fn a_client_and_a_listener_exchange_messages() {
        let endpoint = test_endpoint();
        let mut listener = Listener::bind(&endpoint).unwrap();
        let serving = tokio::spawn(async move {
            let (mut reader, mut writer) = listener.accept().await.unwrap();
            let hello = reader.read::<ClientMessage>().await.unwrap();
            writer
                .write(&ServerMessage::Welcome {
                    protocol_version: 1,
                    daemon_version: "test".to_owned(),
                })
                .await
                .unwrap();
            hello
        });
        let (mut reader, mut writer) = connect(&endpoint).await.unwrap();
        writer
            .write(&ClientMessage::Hello {
                protocol_version: 1,
            })
            .await
            .unwrap();
        assert_eq!(
            reader.read::<ServerMessage>().await.unwrap(),
            Some(ServerMessage::Welcome {
                protocol_version: 1,
                daemon_version: "test".to_owned(),
            })
        );
        assert_eq!(
            serving.await.unwrap(),
            Some(ClientMessage::Hello {
                protocol_version: 1
            })
        );
    }

    #[tokio::test]
    async fn a_second_listener_on_the_same_endpoint_is_refused() {
        let endpoint = test_endpoint();
        let _first = Listener::bind(&endpoint).unwrap();
        assert!(Listener::bind(&endpoint).is_err());
    }

    fn test_endpoint() -> Endpoint {
        let unique = format!(
            "crowbot-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        if cfg!(windows) {
            Endpoint::new(format!(r"\\.\pipe\{unique}"))
        } else {
            Endpoint::new(
                std::env::temp_dir()
                    .join(format!("{unique}.sock"))
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }
}
