use std::ffi::OsString;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crowbot_protocol::transport::{self, MessageReader, MessageWriter};
use crowbot_protocol::{
    ClientMessage, EXIT_DATABASE_HELD, EXIT_ENDPOINT_TOO_LONG, Endpoint, Folders, PROTOCOL_VERSION,
    SOCKET_PATH_LIMIT, ServerMessage,
};
use tokio::sync::Notify;

use crate::service::{Service, StartFailure};
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
/// What the window says of a daemon that was told to stop.
pub const STOPPED: &str = "The daemon was stopped.";

type Stream = (MessageReader, MessageWriter);

pub enum ConnectionEvent {
    /// No daemon was listening, so the client is starting one.
    Starting,
    Connected,
    /// The connection dropped. The client reconnects on its own.
    Disconnected(String),
    /// The client cannot attach and waits for the user to retry.
    Failed(String),
    Incompatible {
        daemon_protocol: u32,
        daemon_version: Option<String>,
    },
    Message(Box<ServerMessage>),
}

/// How the client starts a daemon when none listens.
pub struct DaemonLauncher {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Receives what the daemon writes to its standard error.
    pub log: PathBuf,
    /// The systemd user unit that runs the default daemon, on Linux only.
    /// When it is installed, the client starts the unit instead of `program`.
    pub service: Option<Service>,
}

impl DaemonLauncher {
    /// The unit to start, when one is installed and the user's systemd runs.
    fn service(&self) -> Option<Service> {
        self.service.clone().filter(Service::available)
    }

    /// Starts the daemon as a process of its own: it has no window, it does not
    /// receive the signals of this client's console or session, and it keeps
    /// running after the client exits.
    fn launch(&self) -> std::io::Result<Child> {
        if let Some(parent) = self.log.parent() {
            private_folder(parent)?;
        }
        let log = private_file().create(true).append(true).open(&self.log)?;
        restrict(&self.log)?;
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

/// The log can hold what the daemon says of the user's conversations: on Unix
/// only its user may enter a folder this client creates for it.
#[cfg(unix)]
fn private_folder(folder: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
}

#[cfg(not(unix))]
fn private_folder(folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)
}

/// Options that create a file only its user may read, on Unix.
fn private_file() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

/// Makes a log that an older client created readable by its user only.
#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn restrict(_path: &Path) -> std::io::Result<()> {
    Ok(())
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

/// The daemon leads a session of its own: the end of the terminal or of the
/// process group that started the client does not reach it.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: `setsid` is async-signal-safe and touches no memory of the
    // parent, as a function run between fork and exec must.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
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

    #[cfg(test)]
    pub fn for_test() -> (Self, UnboundedReceiver<ClientMessage>) {
        let (outgoing, sent) = mpsc::unbounded_channel();
        let connection = Self {
            outgoing,
            retry: Arc::new(Notify::new()),
        };
        (connection, sent)
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
                    Ended::Stopped => (ConnectionEvent::Failed(STOPPED.to_owned()), false),
                    Ended::Refused(reason) => (ConnectionEvent::Failed(reason), true),
                    Ended::Incompatible {
                        daemon_protocol,
                        daemon_version,
                    } => (
                        ConnectionEvent::Incompatible {
                            daemon_protocol,
                            daemon_version,
                        },
                        true,
                    ),
                }
            }
            Err(reason) => (ConnectionEvent::Failed(reason), false),
        };
        let failed = matches!(
            event,
            ConnectionEvent::Failed(_) | ConnectionEvent::Incompatible { .. }
        );
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
        let service = launcher.service();
        // Under the unit, systemd restarts a daemon that crashed and stops
        // one that keeps crashing (its start limit), and the start then
        // fails with that reason: the client does not guess.
        if guarded
            && service.is_none()
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
        self.last_launch = Some(Instant::now());
        match service {
            Some(service) => self.start_service(service).await,
            None => self.start_process().await,
        }
    }

    /// Starts the daemon as a process of its own, then waits for it to answer.
    async fn start_process(&self) -> Result<Stream, String> {
        let Some(launcher) = &self.launcher else {
            return Err(String::new());
        };
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
        // The daemon claims its endpoint once it is ready, so the client
        // waits for it to answer, or to exit.
        let mut held = false;
        loop {
            if let Some(stream) = self.connect().await? {
                return Ok(stream);
            }
            if !held && let Ok(Some(status)) = daemon.try_wait() {
                // When two clients each start a daemon, one daemon wins and
                // the other exits: at the database, which the winner may still
                // migrate, so the client waits for the winner; or at the
                // endpoint, which the winner then holds.
                if status.code() == Some(i32::from(EXIT_DATABASE_HELD)) {
                    held = true;
                    continue;
                }
                if let Some(stream) = self.connect().await? {
                    return Ok(stream);
                }
                return Err(self.exited(
                    status.code(),
                    &format!("The daemon stopped as soon as it started ({status})."),
                    launcher.last_log_line(log_start),
                ));
            }
            if launched.elapsed() > START_TIMEOUT {
                return Err(if held {
                    with_log_line(
                        "Another daemon uses the database and did not answer in time.",
                        launcher.last_log_line(log_start),
                    )
                } else {
                    format!(
                        "The daemon started but does not answer on {}.",
                        self.endpoint
                    )
                });
            }
            tokio::time::sleep(START_POLL).await;
        }
    }

    /// Asks the user's systemd to start the daemon's unit. The daemon tells
    /// systemd that it is ready once it claimed its endpoint, so the start
    /// returns when the daemon answers, or when it failed.
    async fn start_service(&self, service: Service) -> Result<Stream, String> {
        let unit = service.clone();
        let started = tokio::task::spawn_blocking(move || unit.start())
            .await
            .map_err(|error| format!("Cannot start the daemon's unit: {error}"))?;
        let held = match started {
            Ok(()) => false,
            Err(StartFailure { status, detail }) => {
                if let Some(stream) = self.connect().await? {
                    return Ok(stream);
                }
                if status != Some(i32::from(EXIT_DATABASE_HELD)) {
                    let shown =
                        status.map_or_else(String::new, |code| format!(", exit status {code}"));
                    return Err(self.exited(
                        status,
                        &format!("The daemon's systemd unit did not start ({detail}{shown})."),
                        service.last_log_line(),
                    ));
                }
                true
            }
        };
        let waited = Instant::now();
        loop {
            if let Some(stream) = self.connect().await? {
                return Ok(stream);
            }
            if waited.elapsed() > START_TIMEOUT {
                return Err(if held {
                    with_log_line(
                        "Another daemon uses the database and did not answer in time.",
                        service.last_log_line(),
                    )
                } else {
                    format!(
                        "The daemon's unit started but does not answer on {}.",
                        self.endpoint
                    )
                });
            }
            tokio::time::sleep(START_POLL).await;
        }
    }

    /// What the window says of a daemon that exited with `code`.
    fn exited(&self, code: Option<i32>, reason: &str, line: Option<String>) -> String {
        if code == Some(i32::from(EXIT_ENDPOINT_TOO_LONG)) {
            return format!(
                "The daemon cannot listen on {}: the path is {} bytes long, over the limit of {SOCKET_PATH_LIMIT} bytes. Start Crow Bot with a shorter --endpoint.",
                self.endpoint,
                self.endpoint.address().len()
            );
        }
        with_log_line(reason, line)
    }

    /// `Ok(None)` means that no daemon listens on the endpoint. No daemon
    /// can listen on a socket path that is too long: the one the client
    /// starts says why.
    async fn connect(&self) -> Result<Option<Stream>, String> {
        if self.endpoint.too_long().is_some() {
            return Ok(None);
        }
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

/// Where a daemon started by the client logs: beside the database that the
/// daemon arguments name, so that a sandbox keeps its log, or else in the
/// state folder (`~/.local/state/crowbot` on Linux, `~/.crowbot` elsewhere).
pub fn daemon_log(daemon_args: &[OsString]) -> Option<PathBuf> {
    let database = daemon_args
        .iter()
        .zip(daemon_args.iter().skip(1))
        .filter(|(flag, _)| *flag == "--db")
        .map(|(_, path)| Path::new(path))
        .next_back();
    match database {
        Some(database) => {
            let folder = database.parent().unwrap_or_else(|| Path::new(""));
            // The daemon runs in the home folder, where a relative path leads.
            let folder = if folder.is_absolute() {
                folder.to_owned()
            } else {
                std::env::home_dir()?.join(folder)
            };
            Some(folder.join("crowbotd.log"))
        }
        None => Some(Folders::local()?.log()),
    }
}

/// How a connection to the daemon ended.
enum Ended {
    /// The connection was lost. Reconnecting can help.
    Dropped(String),
    /// The daemon was told to stop. It is not started again until the user
    /// asks, or until a daemon listens again on its own.
    Stopped,
    /// The daemon answered and refused this client. Reconnecting gives the same answer.
    Refused(String),
    Incompatible {
        daemon_protocol: u32,
        daemon_version: Option<String>,
    },
}

const LEGACY_VERSION_REFUSAL: &str = "this daemon speaks protocol version ";

fn legacy_daemon_protocol(refusal: &str) -> Option<u32> {
    let digits: String = refusal
        .strip_prefix(LEGACY_VERSION_REFUSAL)?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
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
        Ok(Some(ServerMessage::Incompatible {
            protocol_version,
            daemon_version,
        })) => {
            return Ended::Incompatible {
                daemon_protocol: protocol_version,
                daemon_version: Some(daemon_version),
            };
        }
        Ok(Some(ServerMessage::Error { message })) => {
            return match legacy_daemon_protocol(&message) {
                Some(daemon_protocol) => Ended::Incompatible {
                    daemon_protocol,
                    daemon_version: None,
                },
                None => Ended::Refused(format!("The daemon refused the connection: {message}")),
            };
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
                Ok(Some(ServerMessage::Stopping)) => return Ended::Stopped,
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
                service: None,
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

    /// Another daemon holds the database and does not listen yet, as one that
    /// still migrates it: the daemon this client starts exits, and the client
    /// waits for the holder instead of failing.
    #[test]
    fn a_client_waits_for_the_daemon_that_holds_the_database() {
        daemon_binary();
        let scene = Scene::new();
        let lock = std::fs::File::create(scene.dir.join("crowbot.db.lock")).unwrap();
        lock.lock().unwrap();
        let (_connection, mut events) = scene.client("loser");

        let started = Instant::now();
        let mut launched = false;
        let mut seen = |events: &mut UnboundedReceiver<ConnectionEvent>| {
            while let Ok(event) = events.try_recv() {
                match event {
                    ConnectionEvent::Starting => launched = true,
                    ConnectionEvent::Failed(reason) => panic!("the client failed: {reason}"),
                    _ => {}
                }
            }
        };
        while !std::fs::read_to_string(scene.log("loser"))
            .unwrap_or_default()
            .contains("another crowbotd")
        {
            assert!(
                started.elapsed() < PATIENCE,
                "the daemon never found the lock"
            );
            seen(&mut events);
            std::thread::sleep(Duration::from_millis(5));
        }
        // The holder is still busy a moment after the other daemon exited.
        std::thread::sleep(Duration::from_millis(500));
        seen(&mut events);
        assert!(launched);
        answering_daemon(
            scene.endpoint.clone(),
            ServerMessage::Welcome {
                protocol_version: PROTOCOL_VERSION,
                daemon_version: "holder".to_owned(),
            },
        );
        until_connected(&mut events);
        drop(lock);
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
        answering_daemon(
            endpoint,
            ServerMessage::Error {
                message: "the first message must be a hello".to_owned(),
            },
        )
    }

    fn answering_daemon(endpoint: Endpoint, answer: ServerMessage) -> Arc<AtomicU32> {
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
                        writer.write(&answer).await.ok();
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
        assert!(reason.contains("must be a hello"), "{reason}");

        // No silent loop: the daemon still listens, and the client leaves it alone.
        std::thread::sleep(IDLE_POLL * 2 + Duration::from_millis(500));
        assert!(events.try_recv().is_err());
        assert_eq!(greeted.load(Ordering::SeqCst), 1);

        connection.retry();
        let reason = until_failed(&mut events);
        assert!(reason.contains("refused the connection"), "{reason}");
        assert_eq!(greeted.load(Ordering::SeqCst), 2);
    }

    fn until_incompatible(
        events: &mut UnboundedReceiver<ConnectionEvent>,
    ) -> (u32, Option<String>) {
        let started = Instant::now();
        loop {
            assert!(started.elapsed() < PATIENCE, "the client never gave up");
            match events.try_recv() {
                Ok(ConnectionEvent::Incompatible {
                    daemon_protocol,
                    daemon_version,
                }) => return (daemon_protocol, daemon_version),
                Ok(ConnectionEvent::Connected) => panic!("the client connected"),
                Ok(ConnectionEvent::Failed(reason)) => panic!("the client failed: {reason}"),
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    #[test]
    fn a_daemon_of_another_protocol_version_is_reported_with_its_version_and_not_retried() {
        let scene = Scene::new();
        let greeted = answering_daemon(
            scene.endpoint.clone(),
            ServerMessage::Incompatible {
                protocol_version: PROTOCOL_VERSION + 1,
                daemon_version: "9.9.9".to_owned(),
            },
        );
        let (connection, mut events) = scene.client_with(None);
        assert_eq!(
            until_incompatible(&mut events),
            (PROTOCOL_VERSION + 1, Some("9.9.9".to_owned()))
        );

        std::thread::sleep(IDLE_POLL * 2 + Duration::from_millis(500));
        assert!(events.try_recv().is_err());
        assert_eq!(greeted.load(Ordering::SeqCst), 1);

        connection.retry();
        assert_eq!(until_incompatible(&mut events).0, PROTOCOL_VERSION + 1);
        assert_eq!(greeted.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn an_older_daemon_that_refuses_in_words_is_read_as_its_protocol_version() {
        let scene = Scene::new();
        let _greeted = answering_daemon(
            scene.endpoint.clone(),
            ServerMessage::Error {
                message: "this daemon speaks protocol version 3, the client speaks 4".to_owned(),
            },
        );
        let (_connection, mut events) = scene.client_with(None);
        assert_eq!(until_incompatible(&mut events), (3, None));

        assert_eq!(
            legacy_daemon_protocol("this daemon speaks protocol version 12"),
            Some(12)
        );
        assert_eq!(
            legacy_daemon_protocol("the first message must be a hello"),
            None
        );
        assert_eq!(
            legacy_daemon_protocol("this daemon speaks protocol version x"),
            None
        );
    }

    #[test]
    fn the_real_daemon_tells_a_client_of_a_later_version_which_version_it_speaks() {
        daemon_binary();
        let mut scene = Scene::new();
        let (_first, mut first_events) = scene.client("first");
        until_connected(&mut first_events);

        let endpoint = scene.endpoint.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let answer = runtime.block_on(async move {
            let (mut reader, mut writer) = transport::connect(&endpoint).await.unwrap();
            let hello = ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION + 1,
            };
            writer.write(&hello).await.unwrap();
            reader.read::<ServerMessage>().await.unwrap()
        });
        match answer {
            Some(ServerMessage::Incompatible {
                protocol_version, ..
            }) => assert_eq!(protocol_version, PROTOCOL_VERSION),
            other => panic!("expected the versions, got {other:?}"),
        }
        assert_eq!(scene.listening(&["first"]), 1);
    }

    #[cfg(unix)]
    fn signal(pid: u32, signal: i32) {
        // SAFETY: `kill` takes plain integers.
        unsafe {
            libc::kill(i32::try_from(pid).unwrap(), signal);
        }
    }

    /// The daemons this scene started, once they listen.
    #[cfg(unix)]
    fn listening_pid(scene: &mut Scene, name: &str) -> u32 {
        let started = Instant::now();
        while scene.listening(&[name]) == 0 {
            assert!(started.elapsed() < PATIENCE, "the daemon never listened");
            std::thread::sleep(Duration::from_millis(10));
        }
        *scene.daemons.last().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_daemon_told_to_stop_is_shown_as_stopped_and_not_started_again() {
        daemon_binary();
        let mut scene = Scene::new();
        let (_connection, mut events) = scene.client("first");
        until_connected(&mut events);
        let pid = listening_pid(&mut scene, "first");

        signal(pid, libc::SIGTERM);
        assert_eq!(until_failed(&mut events), STOPPED);
        // The client waits for the user, and starts no daemon of its own.
        std::thread::sleep(IDLE_POLL * 2 + Duration::from_millis(500));
        while let Ok(event) = events.try_recv() {
            assert!(
                !matches!(
                    event,
                    ConnectionEvent::Starting | ConnectionEvent::Connected
                ),
                "the client started the stopped daemon again"
            );
        }
        let log = std::fs::read_to_string(scene.log("first")).unwrap();
        assert_eq!(log.matches("listening on").count(), 1, "{log}");
        assert!(log.contains("[crowbotd] stopped"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn a_socket_path_that_is_too_long_is_reported_with_its_reason_and_the_flag() {
        let mut scene = Scene::new();
        let prefix = scene.dir.join("s").to_string_lossy().len();
        let name = "s".repeat(SOCKET_PATH_LIMIT + 2 - prefix);
        scene.endpoint = Endpoint::new(scene.dir.join(name).to_string_lossy().into_owned());
        assert_eq!(scene.endpoint.address().len(), SOCKET_PATH_LIMIT + 1);
        let (_connection, mut events) = scene.client("long");
        let reason = until_failed(&mut events);
        assert!(reason.contains("--endpoint"), "{reason}");
        assert!(
            reason.contains(&format!("{} bytes long", SOCKET_PATH_LIMIT + 1)),
            "{reason}"
        );
        assert!(
            reason.contains(&format!("limit of {SOCKET_PATH_LIMIT} bytes")),
            "{reason}"
        );
        assert!(!scene.dir.join("crowbot.db").exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_log_is_private_and_the_daemon_leads_a_session_of_its_own() {
        use std::os::unix::fs::PermissionsExt;

        daemon_binary();
        let mut scene = Scene::new();
        // A log an older client left readable by everyone.
        std::fs::write(scene.log("first"), "").unwrap();
        std::fs::set_permissions(scene.log("first"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let (_connection, mut events) = scene.client("first");
        until_connected(&mut events);
        let pid = listening_pid(&mut scene, "first");
        let mode = std::fs::metadata(scene.log("first"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // The sixth field of `stat` is the session: the daemon's own pid.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let fields: Vec<&str> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_eq!(fields[3], pid.to_string(), "{stat}");
    }

    /// A unit of the daemon, installed for one test in the runtime unit
    /// folder, which the user's systemd forgets at logout.
    #[cfg(target_os = "linux")]
    struct TestUnit {
        service: Service,
        file: PathBuf,
    }

    #[cfg(target_os = "linux")]
    impl TestUnit {
        /// `None`, after saying so, when no user systemd answers.
        fn install(scene: &Scene, args: &[&Path]) -> Option<Self> {
            let systemctl = |args: &[&str]| {
                Command::new("systemctl")
                    .arg("--user")
                    .args(args)
                    .output()
                    .is_ok_and(|output| output.status.success())
            };
            if !systemctl(&["show-environment"]) {
                eprintln!("skipped: no user systemd answers");
                return None;
            }
            let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
            let folder = runtime.join("systemd").join("user");
            std::fs::create_dir_all(&folder).unwrap();
            let name = format!(
                "crowbotd-client-test-{}.service",
                scene.dir.file_name().unwrap().to_string_lossy()
            );
            let quoted = |path: &Path| format!("\"{}\"", path.display());
            let mut command = vec![
                quoted(daemon_binary()),
                "--endpoint".to_owned(),
                quoted(Path::new(scene.endpoint.address())),
            ];
            command.extend(args.iter().map(|arg| quoted(arg)));
            let unit = include_str!("../../crowbotd/resources/linux/systemd/crowbotd.service")
                .lines()
                .map(|line| match line.strip_prefix("ExecStart=") {
                    Some(_) => format!("ExecStart={}\n", command.join(" ")),
                    None => format!("{line}\n"),
                })
                .collect::<String>();
            let file = folder.join(&name);
            std::fs::write(&file, unit).unwrap();
            assert!(systemctl(&["daemon-reload"]));
            Some(Self {
                service: Service::at(&file, &runtime),
                file,
            })
        }

        fn main_pid(&self) -> u32 {
            let output = Command::new("systemctl")
                .args([
                    "--user",
                    "show",
                    self.service.unit(),
                    "--property=MainPID",
                    "--value",
                ])
                .output()
                .unwrap();
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .unwrap_or(0)
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for TestUnit {
        fn drop(&mut self) {
            let systemctl = |args: &[&str]| {
                Command::new("systemctl")
                    .arg("--user")
                    .args(args)
                    .output()
                    .ok();
            };
            systemctl(&["stop", self.service.unit()]);
            std::fs::remove_file(&self.file).ok();
            systemctl(&["daemon-reload"]);
            systemctl(&["reset-failed", self.service.unit()]);
        }
    }

    #[cfg(target_os = "linux")]
    fn unit_launcher(scene: &Scene, unit: &TestUnit) -> DaemonLauncher {
        let mut launcher = scene.launcher("unit", daemon_binary());
        launcher.service = Some(unit.service.clone());
        launcher
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_client_starts_the_installed_unit_and_the_daemon_lives_in_its_control_group() {
        let scene = Scene::new();
        let db = scene.dir.join("crowbot.db");
        let Some(unit) = TestUnit::install(&scene, &[Path::new("--db"), &db]) else {
            return;
        };
        let (connection, mut events) = scene.client_with(Some(unit_launcher(&scene, &unit)));
        let (launched, _) = until_connected(&mut events);
        assert!(launched);
        let pid = unit.main_pid();
        let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap();
        assert!(
            cgroup
                .trim_end()
                .ends_with(&format!("/{}", unit.service.unit())),
            "{cgroup}"
        );
        let own = std::fs::read_to_string("/proc/self/cgroup").unwrap();
        assert_ne!(own, cgroup);
        // The unit, not the client, holds the log.
        assert!(!scene.log("unit").exists());

        // The client goes, and the daemon stays.
        drop((connection, events));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(unit.main_pid(), pid);
        let (_connection, mut later) = scene.client_with(None);
        assert!(!until_connected(&mut later).0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_client_attaches_again_once_systemd_restarts_a_daemon_that_was_killed() {
        let scene = Scene::new();
        let db = scene.dir.join("crowbot.db");
        let Some(unit) = TestUnit::install(&scene, &[Path::new("--db"), &db]) else {
            return;
        };
        let (_connection, mut events) = scene.client_with(Some(unit_launcher(&scene, &unit)));
        until_connected(&mut events);
        let first = unit.main_pid();

        // Right after the client started the unit, as a crash at once would.
        signal(first, libc::SIGKILL);
        until_connected(&mut events);
        let second = unit.main_pid();
        assert!(second != 0 && second != first, "{first} {second}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_that_fails_is_reported_with_the_reason_from_its_exit_status_and_retried_on_request() {
        let scene = Scene::new();
        // A folder is not a database, so the daemon gives up when it opens it.
        let Some(unit) = TestUnit::install(&scene, &[Path::new("--db"), &scene.dir]) else {
            return;
        };
        let (connection, mut events) = scene.client_with(Some(unit_launcher(&scene, &unit)));
        let reason = until_failed(&mut events);
        assert!(reason.contains("unit did not start"), "{reason}");
        assert!(reason.contains("exit status 1"), "{reason}");
        assert!(reason.contains("[crowbotd] failed:"), "{reason}");
        std::thread::sleep(IDLE_POLL + Duration::from_millis(500));
        assert!(events.try_recv().is_err());

        connection.retry();
        let reason = until_failed(&mut events);
        assert!(reason.contains("unit did not start"), "{reason}");
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
