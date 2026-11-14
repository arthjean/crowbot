use std::ffi::OsString;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crowbot_protocol::transport::{self, MessageReader, MessageWriter};
use crowbot_protocol::{ClientMessage, Endpoint, PROTOCOL_VERSION, ServerMessage};
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// The pause before the client reaches for the daemon again after a connection dropped.
const RECONNECT_DELAY: Duration = Duration::from_millis(200);
/// How often the client looks for a daemon while it waits for the user to retry.
const IDLE_POLL: Duration = Duration::from_secs(1);
/// How often the client looks for the daemon it just started.
const START_POLL: Duration = Duration::from_millis(25);
/// How long a started daemon gets to answer on its endpoint.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// A daemon that must be started again this soon after the last start keeps failing.
const RESTART_GUARD: Duration = Duration::from_secs(10);
const LOG_TAIL_BYTES: u64 = 2048;

type Stream = (MessageReader, MessageWriter);

pub enum ConnectionEvent {
    /// No daemon was listening, so the client is starting one.
    Starting,
    Connected,
    /// The connection dropped. The client reconnects on its own.
    Disconnected(String),
    /// The client cannot attach and waits for the user to retry.
    Failed(String),
    Message(Box<ServerMessage>),
}

/// How the client starts a daemon when none listens.
pub struct DaemonLauncher {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Receives what the daemon writes to its standard error.
    pub log: PathBuf,
}

impl DaemonLauncher {
    /// Starts the daemon as a process of its own: it has no window, it does not
    /// receive the signals of this client's console, and it keeps running after
    /// the client exits.
    fn launch(&self) -> std::io::Result<Child> {
        if let Some(parent) = self.log.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)?;
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        // A long-lived process pins its working folder. The home folder is the
        // one that is never renamed or removed under it.
        if let Some(home) = std::env::home_dir() {
            command.current_dir(home);
        }
        detach(&mut command);
        command.spawn()
    }

    fn log_length(&self) -> u64 {
        std::fs::metadata(&self.log).map_or(0, |metadata| metadata.len())
    }

    /// The last line the daemon logged since `from`, which states why it stopped.
    fn last_log_line(&self, from: u64) -> Option<String> {
        let mut file = std::fs::File::open(&self.log).ok()?;
        let length = file.metadata().ok()?.len();
        let start = from.max(length.saturating_sub(LOG_TAIL_BYTES));
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut tail = Vec::new();
        file.read_to_end(&mut tail).ok()?;
        String::from_utf8_lossy(&tail)
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    }
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    // The daemon gets a console with no window. The agent processes it starts
    // inherit that console instead of opening a window each.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[derive(Clone)]
pub struct Connection {
    outgoing: UnboundedSender<ClientMessage>,
    retry: Arc<Notify>,
}

impl Connection {
    pub fn send(&self, message: ClientMessage) {
        self.outgoing.send(message).ok();
    }

    /// Tries again to attach after a failure.
    pub fn retry(&self) {
        self.retry.notify_one();
    }
}

pub fn start(
    endpoint: Endpoint,
    launcher: Option<DaemonLauncher>,
) -> std::io::Result<(Connection, UnboundedReceiver<ConnectionEvent>)> {
    let (outgoing, outgoing_receiver) = mpsc::unbounded_channel();
    let (events, event_receiver) = mpsc::unbounded_channel();
    let retry = Arc::new(Notify::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let attachment = Attachment {
        endpoint,
        launcher,
        last_launch: None,
        events,
    };
    let retry_signal = retry.clone();
    std::thread::Builder::new()
        .name("daemon-connection".to_owned())
        .spawn(move || runtime.block_on(run(attachment, outgoing_receiver, retry_signal)))?;
    Ok((Connection { outgoing, retry }, event_receiver))
}

struct Attachment {
    endpoint: Endpoint,
    launcher: Option<DaemonLauncher>,
    last_launch: Option<Instant>,
    events: UnboundedSender<ConnectionEvent>,
}

async fn run(
    mut attachment: Attachment,
    mut outgoing: UnboundedReceiver<ClientMessage>,
    retry: Arc<Notify>,
) {
    let mut found: Option<Stream> = None;
    // A daemon that keeps stopping is started again only when the user asks.
    let mut guarded = false;
    loop {
        let attached = match found.take() {
            Some(stream) => Ok(stream),
            None => attachment.attach(guarded).await,
        };
        let (event, refused) = match attached {
            Ok((reader, writer)) => {
                while outgoing.try_recv().is_ok() {}
                match serve(reader, writer, &mut outgoing, &attachment.events).await {
                    Ended::Dropped(reason) => (ConnectionEvent::Disconnected(reason), false),
                    Ended::Refused(reason) => (ConnectionEvent::Failed(reason), true),
                }
            }
            Err(reason) => (ConnectionEvent::Failed(reason), false),
        };
        let failed = matches!(event, ConnectionEvent::Failed(_));
        if attachment.events.send(event).is_err() || outgoing.is_closed() {
            return;
        }
        guarded = !failed;
        if failed {
            found = attachment.wait_for_retry(&retry, !refused).await;
        } else {
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }
}

impl Attachment {
    /// Connects to the daemon, and starts one first when none listens.
    async fn attach(&mut self, guarded: bool) -> Result<Stream, String> {
        if let Some(stream) = self.connect().await? {
            return Ok(stream);
        }
        let Some(launcher) = &self.launcher else {
            return Err(format!("No daemon is listening on {}.", self.endpoint));
        };
        if guarded
            && self
                .last_launch
                .is_some_and(|launched| launched.elapsed() < RESTART_GUARD)
        {
            return Err(with_log_line(
                "The daemon stopped again right after it started.",
                launcher.last_log_line(0),
            ));
        }
        if self.events.send(ConnectionEvent::Starting).is_err() {
            return Err(String::new());
        }
        let log_start = launcher.log_length();
        let mut daemon = launcher.launch().map_err(|error| {
            let program = launcher.program.display();
            match error.kind() {
                ErrorKind::NotFound => {
                    format!("Cannot start the daemon: {program} was not found.")
                }
                _ => format!("Cannot start the daemon ({program}): {error}"),
            }
        })?;
        let launched = Instant::now();
        self.last_launch = Some(launched);
        loop {
            if let Some(stream) = self.connect().await? {
                return Ok(stream);
            }
            if let Ok(Some(status)) = daemon.try_wait() {
                // When two clients each start a daemon, one daemon wins the
                // endpoint and the other exits. The client attaches to the winner.
                if let Some(stream) = self.connect().await? {
                    return Ok(stream);
                }
                return Err(with_log_line(
                    &format!("The daemon stopped as soon as it started ({status})."),
                    launcher.last_log_line(log_start),
                ));
            }
            if launched.elapsed() > START_TIMEOUT {
                return Err(format!(
                    "The daemon started but does not answer on {}.",
                    self.endpoint
                ));
            }
            tokio::time::sleep(START_POLL).await;
        }
    }

    /// `Ok(None)` means that no daemon listens on the endpoint.
    async fn connect(&self) -> Result<Option<Stream>, String> {
        match transport::connect(&self.endpoint).await {
            Ok(stream) => Ok(Some(stream)),
            Err(error) => match error.kind() {
                ErrorKind::NotFound | ErrorKind::ConnectionRefused => Ok(None),
                ErrorKind::PermissionDenied => Err(format!(
                    "Another program is using Crow Bot's endpoint ({}): {error}.",
                    self.endpoint
                )),
                _ => Err(format!(
                    "Cannot reach the daemon on {}: {error}",
                    self.endpoint
                )),
            },
        }
    }

    /// Waits until the user asks to retry. With `watch`, it also returns as soon
    /// as a daemon listens after all. A daemon that refused this client gives
    /// the same answer each time, so it is asked again only on request.
    async fn wait_for_retry(&self, retry: &Notify, watch: bool) -> Option<Stream> {
        loop {
            tokio::select! {
                () = retry.notified() => return None,
                () = tokio::time::sleep(IDLE_POLL), if watch => {
                    if let Ok(Some(stream)) = self.connect().await {
                        return Some(stream);
                    }
                }
            }
        }
    }
}

fn with_log_line(reason: &str, line: Option<String>) -> String {
    match line {
        Some(line) => format!("{reason} Its log ends with: {line}"),
        None => reason.to_owned(),
    }
}

/// The daemon binary that ships next to this client.
pub fn bundled_daemon() -> Option<PathBuf> {
    let client = std::env::current_exe().ok()?;
    Some(sibling_daemon(&client))
}

fn sibling_daemon(client: &Path) -> PathBuf {
    client.with_file_name(format!("crowbotd{}", std::env::consts::EXE_SUFFIX))
}

/// Where a daemon started by the client logs, next to its default database.
pub fn default_daemon_log() -> Option<PathBuf> {
    Some(std::env::home_dir()?.join(".crowbot").join("crowbotd.log"))
}

/// How a connection to the daemon ended.
enum Ended {
    /// The connection was lost. Reconnecting can help.
    Dropped(String),
    /// The daemon answered and refused this client. Reconnecting gives the same answer.
    Refused(String),
}

async fn serve(
    mut reader: MessageReader,
    mut writer: MessageWriter,
    outgoing: &mut UnboundedReceiver<ClientMessage>,
    events: &UnboundedSender<ConnectionEvent>,
) -> Ended {
    let dropped = |reason: &str| Ended::Dropped(reason.to_owned());
    let hello = ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
    };
    if let Err(error) = writer.write(&hello).await {
        return Ended::Dropped(error.to_string());
    }
    match reader.read::<ServerMessage>().await {
        Ok(Some(ServerMessage::Welcome { .. })) => {}
        Ok(Some(ServerMessage::Error { message })) => {
            return Ended::Refused(format!("The daemon refused the connection: {message}"));
        }
        Ok(Some(_)) => return dropped("The daemon did not answer the greeting."),
        Ok(None) => return dropped("The daemon closed the connection."),
        Err(error) => return Ended::Dropped(error.to_string()),
    }
    if events.send(ConnectionEvent::Connected).is_err() {
        return dropped("");
    }
    loop {
        tokio::select! {
            incoming = reader.read::<ServerMessage>() => match incoming {
                Ok(Some(message)) => {
                    if events.send(ConnectionEvent::Message(Box::new(message))).is_err() {
                        return dropped("");
                    }
                }
                Ok(None) => return dropped("The daemon closed the connection."),
                Err(error) => return Ended::Dropped(error.to_string()),
            },
            message = outgoing.recv() => match message {
                Some(message) => {
                    if let Err(error) = writer.write(&message).await {
                        return Ended::Dropped(error.to_string());
                    }
                }
                None => return dropped(""),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    const PATIENCE: Duration = Duration::from_secs(20);

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// Builds the real daemon from the root workspace and returns its path.
    fn daemon_binary() -> &'static Path {
        static DAEMON: OnceLock<PathBuf> = OnceLock::new();
        DAEMON.get_or_init(|| {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
            let output = Command::new(env!("CARGO"))
                .args(["build", "--package", "crowbotd", "--message-format", "json"])
                .arg("--manifest-path")
                .arg(root)
                .stderr(Stdio::inherit())
                .output()
                .unwrap();
            assert!(output.status.success(), "cannot build crowbotd");
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| line.contains(r#""name":"crowbotd""#))
                .find_map(|line| {
                    let start = line.find(r#""executable":""#)? + r#""executable":""#.len();
                    let end = start + line[start..].find('"')?;
                    Some(PathBuf::from(line[start..end].replace(r"\\", r"\")))
                })
                .expect("cargo did not report the crowbotd executable")
        })
    }

    struct Scene {
        dir: PathBuf,
        endpoint: Endpoint,
        daemons: Vec<u32>,
    }

    impl Scene {
        fn new() -> Self {
            let unique = format!(
                "crowbot-client-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let dir = std::env::temp_dir().join(&unique);
            std::fs::create_dir_all(&dir).unwrap();
            let endpoint = if cfg!(windows) {
                Endpoint::new(format!(r"\\.\pipe\{unique}"))
            } else {
                Endpoint::new(dir.join("crowbotd.sock").to_string_lossy().into_owned())
            };
            Self {
                dir,
                endpoint,
                daemons: Vec::new(),
            }
        }

        fn log(&self, name: &str) -> PathBuf {
            self.dir.join(format!("{name}.log"))
        }

        fn launcher(&self, name: &str, program: &Path) -> DaemonLauncher {
            DaemonLauncher {
                program: program.to_path_buf(),
                args: vec![
                    "--endpoint".into(),
                    self.endpoint.address().into(),
                    "--db".into(),
                    self.dir.join("crowbot.db").into(),
                ],
                log: self.log(name),
            }
        }

        fn client(&self, name: &str) -> (Connection, UnboundedReceiver<ConnectionEvent>) {
            self.client_with(Some(self.launcher(name, daemon_binary())))
        }

        fn client_with(
            &self,
            launcher: Option<DaemonLauncher>,
        ) -> (Connection, UnboundedReceiver<ConnectionEvent>) {
            start(self.endpoint.clone(), launcher).unwrap()
        }

        /// The daemons that claimed the endpoint, as their logs tell.
        fn listening(&mut self, names: &[&str]) -> usize {
            names
                .iter()
                .filter(|name| {
                    let log = std::fs::read_to_string(self.log(name)).unwrap_or_default();
                    let pid = log
                        .lines()
                        .find_map(|line| line.strip_prefix("[crowbotd] process "))
                        .and_then(|pid| pid.trim().parse::<u32>().ok());
                    self.daemons.extend(pid);
                    log.contains("[crowbotd] listening on")
                })
                .count()
        }
    }

    impl Drop for Scene {
        fn drop(&mut self) {
            for pid in &self.daemons {
                let pid = pid.to_string();
                let killed = if cfg!(windows) {
                    Command::new("taskkill").args(["/F", "/PID", &pid]).output()
                } else {
                    Command::new("kill").args(["-9", &pid]).output()
                };
                killed.ok();
            }
            std::thread::sleep(Duration::from_millis(100));
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    /// Reads events until the client is connected, and returns whether it had
    /// to start a daemon and how long it took.
    fn until_connected(events: &mut UnboundedReceiver<ConnectionEvent>) -> (bool, Duration) {
        let started = Instant::now();
        let mut launched = false;
        loop {
            assert!(started.elapsed() < PATIENCE, "the client never connected");
            match events.try_recv() {
                Ok(ConnectionEvent::Starting) => launched = true,
                Ok(ConnectionEvent::Connected) => return (launched, started.elapsed()),
                Ok(ConnectionEvent::Failed(reason)) => panic!("the client failed: {reason}"),
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    fn until_failed(events: &mut UnboundedReceiver<ConnectionEvent>) -> String {
        let started = Instant::now();
        loop {
            assert!(started.elapsed() < PATIENCE, "the client never gave up");
            match events.try_recv() {
                Ok(ConnectionEvent::Failed(reason)) => return reason,
                Ok(ConnectionEvent::Connected) => panic!("the client connected"),
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    #[test]
    fn a_client_starts_the_daemon_when_none_listens_and_a_second_client_reuses_it() {
        daemon_binary();
        let mut scene = Scene::new();

        let (_first, mut first_events) = scene.client("first");
        let (launched, took) = until_connected(&mut first_events);
        assert!(launched);
        assert!(took < Duration::from_secs(3), "attached after {took:?}");

        let (_second, mut second_events) = scene.client("second");
        let (launched, _) = until_connected(&mut second_events);
        assert!(!launched);
        assert_eq!(scene.listening(&["first", "second"]), 1);
        assert!(!scene.log("second").exists());
    }

    #[test]
    fn the_daemon_outlives_the_client_that_started_it() {
        daemon_binary();
        let mut scene = Scene::new();
        let (first, mut first_events) = scene.client("first");
        until_connected(&mut first_events);
        // Dropping both ends closes the connection, as closing the window does.
        drop((first, first_events));
        std::thread::sleep(Duration::from_millis(500));

        let (_connection, mut later) = scene.client_with(None);
        let (launched, _) = until_connected(&mut later);
        assert!(!launched);
        assert_eq!(scene.listening(&["first"]), 1);
    }

    #[test]
    fn two_clients_that_start_together_share_one_daemon() {
        daemon_binary();
        let mut scene = Scene::new();
        let (_first, mut first_events) = scene.client("first");
        let (_second, mut second_events) = scene.client("second");
        until_connected(&mut first_events);
        until_connected(&mut second_events);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(scene.listening(&["first", "second"]), 1);
    }

    #[test]
    fn a_missing_daemon_binary_is_reported_and_retried_only_on_request() {
        let scene = Scene::new();
        let missing = scene.dir.join("no-such-crowbotd");
        let (connection, mut events) = scene.client_with(Some(scene.launcher("missing", &missing)));
        let reason = until_failed(&mut events);
        assert!(reason.contains("was not found"), "{reason}");
        assert!(reason.contains("no-such-crowbotd"), "{reason}");

        // No silent loop: nothing more happens until the user retries.
        std::thread::sleep(IDLE_POLL + Duration::from_millis(500));
        assert!(events.try_recv().is_err());
        connection.retry();
        let reason = until_failed(&mut events);
        assert!(reason.contains("was not found"), "{reason}");
    }

    #[test]
    fn a_daemon_that_exits_at_once_is_reported_with_its_reason() {
        let scene = Scene::new();
        let mut launcher = scene.launcher("broken", daemon_binary());
        // A folder is not a database, so the daemon gives up when it opens it.
        launcher
            .args
            .extend(["--db".into(), scene.dir.clone().into()]);
        let (connection, mut events) = scene.client_with(Some(launcher));
        let reason = until_failed(&mut events);
        assert!(reason.contains("stopped as soon as it started"), "{reason}");
        assert!(reason.contains("[crowbotd] failed:"), "{reason}");
        std::thread::sleep(IDLE_POLL + Duration::from_millis(500));
        assert!(events.try_recv().is_err());

        connection.retry();
        let reason = until_failed(&mut events);
        assert!(reason.contains("stopped as soon as it started"), "{reason}");
    }

    /// A daemon that refuses every greeting, and counts the clients that greeted it.
    fn refusing_daemon(endpoint: Endpoint) -> Arc<AtomicU32> {
        let greeted = Arc::new(AtomicU32::new(0));
        let count = greeted.clone();
        let (ready, bound) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let mut listener = transport::Listener::bind(&endpoint).unwrap();
                ready.send(()).unwrap();
                while let Ok((mut reader, mut writer)) = listener.accept().await {
                    if matches!(reader.read::<ClientMessage>().await, Ok(Some(_))) {
                        count.fetch_add(1, Ordering::SeqCst);
                        let refusal = ServerMessage::Error {
                            message: "wrong protocol version".to_owned(),
                        };
                        writer.write(&refusal).await.ok();
                    }
                }
            });
        });
        bound.recv_timeout(PATIENCE).unwrap();
        greeted
    }

    #[test]
    fn a_daemon_that_refuses_the_client_is_asked_again_only_on_request() {
        let scene = Scene::new();
        let greeted = refusing_daemon(scene.endpoint.clone());
        let (connection, mut events) = scene.client_with(None);
        let reason = until_failed(&mut events);
        assert!(reason.contains("refused the connection"), "{reason}");
        assert!(reason.contains("wrong protocol version"), "{reason}");

        // No silent loop: the daemon still listens, and the client leaves it alone.
        std::thread::sleep(IDLE_POLL * 2 + Duration::from_millis(500));
        assert!(events.try_recv().is_err());
        assert_eq!(greeted.load(Ordering::SeqCst), 1);

        connection.retry();
        let reason = until_failed(&mut events);
        assert!(reason.contains("refused the connection"), "{reason}");
        assert_eq!(greeted.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_bundled_daemon_sits_next_to_the_client() {
        let client = Path::new("install").join(format!("crowbot{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(
            sibling_daemon(&client),
            Path::new("install").join(format!("crowbotd{}", std::env::consts::EXE_SUFFIX))
        );
    }
}
