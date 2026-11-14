use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crowbot_protocol::transport::{self, MessageReader, MessageWriter};
use crowbot_protocol::{
    ClientMessage, ConversationSnapshot, Endpoint, Event, PROTOCOL_VERSION, QueueState,
    ServerMessage,
};

const PATIENCE: Duration = Duration::from_secs(20);

static NEXT: AtomicU32 = AtomicU32::new(0);

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

pub fn test_endpoint(dir: &Path) -> Endpoint {
    if cfg!(windows) {
        Endpoint::new(format!(r"\\.\pipe\{}", unique("crowbotd-test")))
    } else {
        Endpoint::new(dir.join("crowbotd.sock").to_string_lossy().into_owned())
    }
}

pub fn test_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(unique("crowbotd-test"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fake_agent() -> &'static Path {
    static AGENT: OnceLock<PathBuf> = OnceLock::new();
    AGENT.get_or_init(|| {
        let name = format!("fake_acp_agent{}", std::env::consts::EXE_SUFFIX);
        let agent = Path::new(env!("CARGO_BIN_EXE_crowbotd"))
            .with_file_name("examples")
            .join(name);
        if !agent.exists() {
            let built = Command::new(env!("CARGO"))
                .args(["build", "-p", "crowbotd", "--example", "fake_acp_agent"])
                .status()
                .unwrap();
            assert!(built.success(), "cannot build the scripted agent");
        }
        agent
    })
}

fn daemon_command(dir: &Path, endpoint: &Endpoint) -> Command {
    let agent = serde_json::json!({
        "command": fake_agent(),
        "env": { "FAKE_AGENT_HEARTBEAT": dir.join("heartbeat") },
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_crowbotd"));
    command
        .arg("--db")
        .arg(dir.join("crowbot.db"))
        .arg("--cwd")
        .arg(dir)
        .args(["--endpoint", endpoint.address()])
        .args(["--agent", &agent.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

/// Runs a daemon that is expected to exit on its own.
pub fn run_daemon(dir: &Path, endpoint: &Endpoint) -> Output {
    daemon_command(dir, endpoint)
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

pub struct Daemon {
    dir: PathBuf,
    endpoint: Endpoint,
    child: Option<Child>,
}

impl Daemon {
    pub fn start() -> Self {
        let dir = test_dir();
        let endpoint = test_endpoint(&dir);
        let mut daemon = Self {
            dir,
            endpoint,
            child: None,
        };
        daemon.spawn();
        daemon
    }

    /// Starts the daemon again on the same database and endpoint.
    pub fn spawn(&mut self) {
        assert!(self.child.is_none(), "the daemon is still running");
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("daemon.log"))
            .unwrap();
        self.child = Some(
            daemon_command(&self.dir, &self.endpoint)
                .stderr(log)
                .spawn()
                .unwrap(),
        );
    }

    /// Ends the daemon the way a crash does: at once, with no chance to clean up.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }

    pub async fn client(&self) -> Client {
        Client::connect(&self.endpoint).await
    }

    /// The counter that the scripted agent rewrites while its process is alive.
    pub fn heartbeat(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("heartbeat")).ok()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.kill();
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

pub struct Client {
    reader: MessageReader,
    writer: MessageWriter,
}

impl Client {
    pub async fn connect(endpoint: &Endpoint) -> Self {
        let (reader, writer) = connect(endpoint).await;
        let mut client = Self { reader, writer };
        client
            .send(ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
            })
            .await;
        match client.next().await {
            ServerMessage::Welcome { .. } => client,
            other => panic!("expected a welcome, got {other:?}"),
        }
    }

    pub async fn send(&mut self, message: ClientMessage) {
        self.writer.write(&message).await.unwrap();
    }

    pub async fn next(&mut self) -> ServerMessage {
        self.next_within(PATIENCE)
            .await
            .expect("the daemon sent nothing in time")
    }

    pub async fn next_within(&mut self, patience: Duration) -> Option<ServerMessage> {
        match tokio::time::timeout(patience, self.reader.read::<ServerMessage>()).await {
            Ok(message) => Some(message.unwrap().expect("the daemon closed the connection")),
            Err(_) => None,
        }
    }

    /// Reads messages until one gives a value, and returns that value.
    pub async fn until<T>(&mut self, mut pick: impl FnMut(ServerMessage) -> Option<T>) -> T {
        loop {
            if let Some(found) = pick(self.next().await) {
                return found;
            }
        }
    }

    /// Reads messages until an event of the conversation matches.
    pub async fn until_event(&mut self, matches: impl Fn(&Event) -> bool) -> Event {
        self.until(|message| match message {
            ServerMessage::ConversationEvent { event, .. } if matches(&event) => Some(event),
            _ => None,
        })
        .await
    }

    /// Reads messages until the queue matches.
    pub async fn until_queue(&mut self, matches: impl Fn(&QueueState) -> bool) -> QueueState {
        self.until(|message| match message {
            ServerMessage::QueueChanged { queue, .. } if matches(&queue) => Some(queue),
            _ => None,
        })
        .await
    }

    pub async fn until_error(&mut self) -> String {
        self.until(|message| match message {
            ServerMessage::Error { message } => Some(message),
            _ => None,
        })
        .await
    }

    /// Reads messages until the reply to the running turn holds `text`.
    pub async fn until_reply(&mut self, text: &str) {
        self.until_event(
            |event| matches!(event, Event::MessageText(reply) if reply.contains(text)),
        )
        .await;
    }

    /// Reads messages until the running turn ends, and returns how it ended.
    pub async fn until_turn_ends(&mut self) -> Event {
        self.until_event(Event::ends_turn).await
    }

    /// Creates a conversation and opens it.
    pub async fn new_conversation(&mut self) -> i64 {
        self.send(ClientMessage::CreateConversation).await;
        let id = self
            .until(|message| match message {
                ServerMessage::ConversationCreated(summary) => Some(summary.id),
                _ => None,
            })
            .await;
        self.open(id).await;
        id
    }

    pub async fn open(&mut self, conversation_id: i64) -> ConversationSnapshot {
        self.send(ClientMessage::OpenConversation { conversation_id })
            .await;
        self.until(|message| match message {
            ServerMessage::ConversationOpened(snapshot) => Some(snapshot),
            _ => None,
        })
        .await
    }

    pub async fn say(&mut self, conversation_id: i64, text: &str) {
        self.send(ClientMessage::SendMessage {
            conversation_id,
            text: text.to_owned(),
        })
        .await;
    }
}

/// Connects to a daemon that may still be starting.
pub async fn connect(endpoint: &Endpoint) -> (MessageReader, MessageWriter) {
    let started = Instant::now();
    loop {
        match transport::connect(endpoint).await {
            Ok(connection) => return connection,
            Err(error) if started.elapsed() < PATIENCE => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("cannot reach the daemon on {endpoint}: {error}"),
        }
    }
}

pub fn user_texts(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::UserText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub fn queued_texts(queue: &QueueState) -> Vec<&str> {
    queue
        .messages
        .iter()
        .map(|message| message.text.as_str())
        .collect()
}
