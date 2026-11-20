use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub use crowbot_protocol::scratch::Scratch;
use crowbot_protocol::transport::{self, MessageReader, MessageWriter};
use crowbot_protocol::{
    BotDraft, BotPatch, BotSummary, ClientMessage, ConversationSnapshot, ConversationSummary,
    Endpoint, Event, PROTOCOL_VERSION, QueueState, ServerMessage, TimedEvent,
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

/// A folder of its own for a test, removed when it is dropped. It sits where
/// a socket inside it fits, whatever `TMPDIR` is (`crowbot_protocol::scratch`).
pub fn test_dir() -> Scratch {
    Scratch::new("crowbotd-test")
}

/// Whether the test runs as root, as a container job does. A test that root
/// cannot run says so, with `why`, and does nothing.
pub fn skipped_as_root(why: &str) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipped: the test runs as root, and {why}");
            return true;
        }
    }
    #[cfg(not(unix))]
    let _ = why;
    false
}

pub fn fake_agent() -> &'static Path {
    static AGENT: OnceLock<PathBuf> = OnceLock::new();
    AGENT.get_or_init(|| {
        let name = format!("fake_acp_agent{}", std::env::consts::EXE_SUFFIX);
        let agent = Path::new(env!("CARGO_BIN_EXE_crowbotd"))
            .with_file_name("examples")
            .join(name);
        // Built each time: a test filter skips the examples, and an agent left
        // by an earlier build would answer with an older script.
        let built = Command::new(env!("CARGO"))
            .args([
                "build",
                "--locked",
                "-p",
                "crowbotd",
                "--example",
                "fake_acp_agent",
            ])
            .status()
            .unwrap();
        assert!(built.success(), "cannot build the scripted agent");
        agent
    })
}

/// The agent that the bots of a test daemon run.
#[derive(Clone, Copy)]
enum Agent {
    /// The scripted agent, which may advertise that it takes a system prompt.
    /// It reports a sign-in, read from the file `auth` of the test folder.
    Scripted { system_prompt: bool },
    /// The scripted agent, which says nothing about a sign-in.
    Silent,
    /// An agent whose program does not exist.
    Missing,
    /// The daemon's own Claude profile: the real adapter and the real model.
    Claude,
    /// The daemon's own Claude profile and the runtime it installs itself,
    /// on a `PATH` that holds neither Bun nor Node.js.
    ClaudeAlone,
    /// The daemon's own providers. The runtimes it installs are the ones of
    /// the file `manifest.json` of the test folder, and the process gets the
    /// variables given, which the scripted agent it starts reads.
    Providers(&'static [(&'static str, &'static str)]),
}

fn daemon_command(dir: &Path, endpoint: &Endpoint, agent: Agent) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crowbotd"));
    command
        .arg("--db")
        .arg(dir.join("crowbot.db"))
        .arg("--bots-dir")
        .arg(dir.join("bots"))
        .args(["--endpoint", endpoint.address()])
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    let scripted = |env: serde_json::Value| {
        serde_json::json!({ "command": fake_agent(), "env": env }).to_string()
    };
    let heartbeat = dir.join("heartbeat");
    match agent {
        Agent::Scripted { system_prompt } => {
            let mut env = serde_json::json!({
                "FAKE_AGENT_HEARTBEAT": heartbeat,
                "FAKE_AGENT_AUTH": dir.join("auth"),
            });
            if system_prompt {
                env["FAKE_AGENT_SYSTEM_PROMPT"] = "1".into();
            }
            command.args(["--agent", &scripted(env)]);
        }
        Agent::Silent => {
            let env = serde_json::json!({ "FAKE_AGENT_HEARTBEAT": heartbeat });
            command.args(["--agent", &scripted(env)]);
        }
        Agent::Missing => {
            let agent = serde_json::json!({ "command": dir.join("no-such-agent") });
            command.args(["--agent", &agent.to_string()]);
        }
        Agent::Claude => {}
        Agent::ClaudeAlone => {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let kept = std::env::split_paths(&path).filter(|folder| {
                !["bun", "node"].iter().any(|runtime| {
                    folder
                        .join(runtime)
                        .with_extension(std::env::consts::EXE_EXTENSION)
                        .is_file()
                        || folder.join(runtime).is_file()
                })
            });
            command.env("PATH", std::env::join_paths(kept).unwrap());
        }
        Agent::Providers(variables) => {
            command
                .arg("--runtime-manifest")
                .arg(dir.join("manifest.json"))
                .env("FAKE_AGENT_HEARTBEAT", heartbeat)
                .envs(variables.iter().copied());
        }
    }
    command
}

/// The same command, run in a user namespace of its own under `confinement`:
/// `runtimes` is the folder mounted `noexec`.
fn confined(command: Command, runtimes: &Path, confinement: Confinement) -> Command {
    let script = match confinement {
        Confinement::NoexecRuntimes => {
            r#"mkdir -p "$0" && mount -t tmpfs -o noexec,mode=0700 tmpfs "$0" && exec "$@""#
        }
        Confinement::NoUserNamespaces => concat!(
            "echo 0 > /proc/sys/user/max_user_namespaces && ",
            r#"exec setpriv --bounding-set -all --inh-caps -all --ambient-caps -all -- "$@""#
        ),
    };
    let mut wrapped = Command::new("unshare");
    wrapped
        .args(["--user", "--map-root-user", "--mount", "sh", "-c", script])
        .arg(runtimes)
        .arg(command.get_program())
        .args(command.get_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    for (name, value) in command.get_envs() {
        match value {
            Some(value) => wrapped.env(name, value),
            None => wrapped.env_remove(name),
        };
    }
    wrapped
}

/// Runs a daemon that is expected to exit on its own.
pub fn run_daemon(dir: &Path, endpoint: &Endpoint) -> Output {
    let agent = Agent::Scripted {
        system_prompt: false,
    };
    daemon_command(dir, endpoint, agent)
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

pub struct Daemon {
    dir: Scratch,
    endpoint: Endpoint,
    child: Option<Child>,
    agent: Agent,
    /// The daemon runs in a user namespace of its own, which takes something
    /// from it.
    confinement: Option<Confinement>,
}

/// What a daemon in a user namespace of its own goes without. Its account
/// maps to the test's. Only Linux tests confine a daemon.
#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum Confinement {
    /// The folder of its runtimes is a file system mounted `noexec`.
    NoexecRuntimes,
    /// It holds no capability and can create no user namespace, as on a
    /// computer that disables them.
    NoUserNamespaces,
}

impl Daemon {
    /// A daemon whose agent takes no system prompt at session creation.
    pub fn start() -> Self {
        Self::start_with(Agent::Scripted {
            system_prompt: false,
        })
    }

    /// A daemon whose agent advertises that it takes a system prompt.
    pub fn start_with_system_prompt() -> Self {
        Self::start_with(Agent::Scripted {
            system_prompt: true,
        })
    }

    /// A daemon whose agent says nothing about a sign-in.
    pub fn start_with_silent_agent() -> Self {
        Self::start_with(Agent::Silent)
    }

    /// A daemon whose agent program does not exist.
    pub fn start_with_missing_agent() -> Self {
        Self::start_with(Agent::Missing)
    }

    /// A daemon whose bots run on Claude, with the user's own sign-in.
    pub fn start_with_claude() -> Self {
        Self::start_with(Agent::Claude)
    }

    /// A daemon whose bots run on Claude, started from the runtime the
    /// daemon installs, on a computer with neither Bun nor Node.js.
    pub fn start_with_claude_alone() -> Self {
        Self::start_with(Agent::ClaudeAlone)
    }

    /// A daemon with its own providers, which installs the runtimes that
    /// `manifest` lists and no other. Each provider runs the scripted agent
    /// once a test sets it as the executable of the provider.
    pub fn start_with_providers(
        manifest: &serde_json::Value,
        variables: &'static [(&'static str, &'static str)],
    ) -> Self {
        let mut daemon = Self::stopped(Agent::Providers(variables));
        std::fs::write(daemon.dir.join("manifest.json"), manifest.to_string()).unwrap();
        daemon.spawn();
        daemon
    }

    /// The daemon of `start_with_providers`, run in a user namespace of its
    /// own under `confinement`; `None` when this computer cannot make one.
    #[cfg(target_os = "linux")]
    pub fn start_with_providers_confined(
        manifest: &serde_json::Value,
        confinement: Confinement,
    ) -> Option<Self> {
        let scratch = test_dir();
        let probe = confined(Command::new("true"), &scratch.join("runtimes"), confinement)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !probe.is_ok_and(|status| status.success()) {
            return None;
        }
        let mut daemon = Self::stopped(Agent::Providers(&[]));
        daemon.confinement = Some(confinement);
        std::fs::write(daemon.dir.join("manifest.json"), manifest.to_string()).unwrap();
        daemon.spawn();
        Some(daemon)
    }

    /// The folder the daemon keeps its runtimes and the files of providers in.
    pub fn data_dir(&self) -> &Path {
        &self.dir
    }

    /// A daemon that starts on a database file the test wrote first, as an
    /// earlier daemon would have left it.
    pub fn start_on(write_database: impl FnOnce(&Path)) -> Self {
        let mut daemon = Self::stopped(Agent::Scripted {
            system_prompt: false,
        });
        write_database(&daemon.dir.join("crowbot.db"));
        daemon.spawn();
        daemon
    }

    fn start_with(agent: Agent) -> Self {
        let mut daemon = Self::stopped(agent);
        daemon.spawn();
        daemon
    }

    fn stopped(agent: Agent) -> Self {
        let dir = test_dir();
        let endpoint = test_endpoint(&dir);
        Self {
            dir,
            endpoint,
            child: None,
            agent,
            confinement: None,
        }
    }

    pub fn database(&self) -> PathBuf {
        self.dir.join("crowbot.db")
    }

    /// Where the daemon makes the folders of the bots that were given none.
    pub fn bots_dir(&self) -> PathBuf {
        self.dir.join("bots")
    }

    /// A folder of the test that the daemon knows nothing about.
    pub fn folder(&self, name: &str) -> PathBuf {
        let folder = self.dir.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// Starts the daemon again on the same database and endpoint.
    pub fn spawn(&mut self) {
        assert!(self.child.is_none(), "the daemon is still running");
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("daemon.log"))
            .unwrap();
        let mut command = daemon_command(&self.dir, &self.endpoint, self.agent);
        if let Some(confinement) = self.confinement {
            command = confined(command, &self.dir.join("runtimes"), confinement);
        }
        self.child = Some(command.stderr(log).spawn().unwrap());
    }

    /// The endpoint the daemon listens on.
    // Read by the tests of the stop signals, which Unix alone sends.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The process of the daemon, while it runs.
    // Read by the tests of the stop signals, which Unix alone sends.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn pid(&self) -> u32 {
        self.child.as_ref().expect("the daemon is not running").id()
    }

    /// Sends a signal to the daemon, as a service stop or Ctrl+C does.
    #[cfg(unix)]
    pub fn signal(&self, signal: i32) {
        let pid = i32::try_from(self.pid()).unwrap();
        // SAFETY: `kill` takes plain integers.
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
    }

    /// Waits for the daemon to exit, and returns its status and how long it
    /// took, or `None` when it still runs after `patience`.
    // Read by the tests of the stop signals, which Unix alone sends.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn wait_exit(
        &mut self,
        patience: Duration,
    ) -> Option<(std::process::ExitStatus, Duration)> {
        let started = Instant::now();
        let child = self.child.as_mut()?;
        while started.elapsed() < patience {
            if let Some(status) = child.try_wait().unwrap() {
                self.child = None;
                return Some((status, started.elapsed()));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// What the daemon wrote to its standard error.
    // Read by the tests of the stop signals, which Unix alone sends.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default()
    }

    /// The processes of the scripted agents of this daemon: the ones whose
    /// environment names the folder of the test.
    #[cfg(target_os = "linux")]
    pub fn agent_processes(&self) -> Vec<u32> {
        processes_with(&self.dir.join("heartbeat").to_string_lossy())
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

    pub async fn raw_connection(&self) -> (MessageReader, MessageWriter) {
        connect(&self.endpoint).await
    }

    /// The file that tells whether the scripted agent is signed in, and the
    /// files of the same name with another extension.
    pub fn auth_file(&self, extension: &str) -> PathBuf {
        self.dir.join("auth").with_extension(extension)
    }

    /// The counter that the scripted agent rewrites while its process is alive.
    pub fn heartbeat(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("heartbeat")).ok()
    }
}

/// The daemon is killed, and its agents are let end, before its folder, a
/// field, is removed: an agent still alive would write in it meanwhile.
impl Drop for Daemon {
    fn drop(&mut self) {
        self.kill();
        #[cfg(target_os = "linux")]
        eventually(Duration::from_secs(10), || {
            self.agent_processes().is_empty()
        });
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
        tokio::time::timeout(PATIENCE, self.reader.read::<ServerMessage>())
            .await
            .expect("the daemon sent nothing in time")
            .unwrap()
            .expect("the daemon closed the connection")
    }

    /// Reads until the daemon closes the connection, and returns what it sent.
    // Read by the tests of the stop signals, which Unix alone sends.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub async fn until_closed(&mut self) -> Vec<ServerMessage> {
        let mut messages = Vec::new();
        loop {
            let read = tokio::time::timeout(PATIENCE, self.reader.read::<ServerMessage>())
                .await
                .expect("the daemon kept the connection open");
            match read {
                Ok(Some(message)) => messages.push(message),
                Ok(None) | Err(_) => return messages,
            }
        }
    }

    /// The next message, for a turn of a real model that takes its time.
    pub async fn next_within_minutes(&mut self) -> ServerMessage {
        tokio::time::timeout(
            Duration::from_secs(180),
            self.reader.read::<ServerMessage>(),
        )
        .await
        .expect("the daemon sent nothing in time")
        .unwrap()
        .expect("the daemon closed the connection")
    }

    /// The next message about a conversation the client opened, if one comes
    /// in time. What the daemon tells every client about bots is skipped.
    pub async fn next_within(&mut self, patience: Duration) -> Option<ServerMessage> {
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            let read = self.reader.read::<ServerMessage>();
            match tokio::time::timeout_at(deadline, read).await {
                Ok(message) => {
                    let message = message.unwrap().expect("the daemon closed the connection");
                    if !matches!(
                        message,
                        ServerMessage::BotChanged(_) | ServerMessage::ConversationChanged(_)
                    ) {
                        return Some(message);
                    }
                }
                Err(_) => return None,
            }
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

    pub async fn until_accepted(&mut self) -> i64 {
        self.until(|message| match message {
            ServerMessage::MessageAccepted { conversation_id } => Some(conversation_id),
            ServerMessage::MessageRefused { message, .. } => {
                panic!("the daemon refused the message: {message}")
            }
            _ => None,
        })
        .await
    }

    pub async fn until_refused(&mut self) -> (i64, String) {
        self.until(|message| match message {
            ServerMessage::MessageRefused {
                conversation_id,
                message,
            } => Some((conversation_id, message)),
            ServerMessage::MessageAccepted { .. } => panic!("the daemon took the message"),
            _ => None,
        })
        .await
    }

    pub async fn until_not_resolved(&mut self) -> (i64, String, String) {
        self.until(|message| match message {
            ServerMessage::ApprovalNotResolved {
                conversation_id,
                approval_id,
                message,
            } => Some((conversation_id, approval_id, message)),
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

    /// Creates a bot and opens its conversation.
    pub async fn new_conversation(&mut self) -> i64 {
        let id = self.new_bot("Crow").await.current_conversation_id;
        self.open(id).await;
        id
    }

    pub async fn new_bot(&mut self, name: &str) -> BotSummary {
        self.create_bot(BotDraft {
            name: name.to_owned(),
            ..BotDraft::default()
        })
        .await
    }

    pub async fn create_bot(&mut self, draft: BotDraft) -> BotSummary {
        self.send(ClientMessage::CreateBot { draft }).await;
        self.until_created().await
    }

    /// Reads messages until the daemon tells which bot this client made, and
    /// returns the summary that was sent for it.
    pub async fn until_created(&mut self) -> BotSummary {
        let mut changed = Vec::new();
        loop {
            match self.next().await {
                ServerMessage::BotChanged(summary) => changed.push(summary),
                ServerMessage::BotCreated { bot_id } => {
                    return changed
                        .into_iter()
                        .rev()
                        .find(|summary| summary.bot.id == bot_id)
                        .expect("the new bot was not sent before the answer");
                }
                ServerMessage::Error { message } => panic!("the daemon refused: {message}"),
                ServerMessage::InvalidField { field, message } => {
                    panic!("the daemon refused {field}: {message}")
                }
                _ => {}
            }
        }
    }

    pub async fn update_bot(&mut self, bot_id: i64, patch: BotPatch) {
        self.send(ClientMessage::UpdateBot { bot_id, patch }).await;
    }

    /// Reads messages until a bot summary matches.
    pub async fn until_bot(&mut self, matches: impl Fn(&BotSummary) -> bool) -> BotSummary {
        self.until(|message| match message {
            ServerMessage::BotChanged(summary) if matches(&summary) => Some(summary),
            _ => None,
        })
        .await
    }

    /// Reads messages until the daemon refuses a field, and returns its name.
    pub async fn until_invalid(&mut self) -> String {
        self.until(|message| match message {
            ServerMessage::InvalidField { field, .. } => Some(field),
            _ => None,
        })
        .await
    }

    pub async fn bots(&mut self) -> Vec<BotSummary> {
        self.send(ClientMessage::ListBots).await;
        self.until(|message| match message {
            ServerMessage::Bots(bots) => Some(bots),
            _ => None,
        })
        .await
    }

    pub async fn bot(&mut self, bot_id: i64) -> BotSummary {
        let bots = self.bots().await;
        bots.into_iter()
            .find(|summary| summary.bot.id == bot_id)
            .expect("the bot is not in the list")
    }

    pub async fn conversations(&mut self, bot_id: i64) -> Vec<ConversationSummary> {
        self.send(ClientMessage::ListConversations { bot_id }).await;
        self.until(|message| match message {
            ServerMessage::Conversations {
                conversations,
                bot_id: listed,
            } if listed == bot_id => Some(conversations),
            _ => None,
        })
        .await
    }

    /// Sends a prompt and returns the whole reply of the turn.
    pub async fn ask(&mut self, conversation_id: i64, text: &str) -> String {
        self.say(conversation_id, text).await;
        let mut reply = String::new();
        loop {
            match self.next().await {
                ServerMessage::ConversationEvent {
                    conversation_id: about,
                    event,
                    ..
                } if about == conversation_id => match event {
                    Event::MessageText(text) => reply.push_str(&text),
                    Event::TurnFailed { message, .. } => panic!("the turn failed: {message}"),
                    event if event.ends_turn() => return reply,
                    _ => {}
                },
                _ => {}
            }
        }
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

pub fn events(snapshot: &ConversationSnapshot) -> Vec<Event> {
    snapshot
        .events
        .iter()
        .map(|timed| timed.event.clone())
        .collect()
}

pub fn user_texts(events: &[TimedEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|timed| match &timed.event {
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

/// The processes of this user whose environment holds `text`.
#[cfg(target_os = "linux")]
pub fn processes_with(text: &str) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            // A process that exited and was not reaped yet is no longer there.
            let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let zombie = state
                .rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'));
            !zombie
                && std::fs::read(format!("/proc/{pid}/environ"))
                    .is_ok_and(|environ| String::from_utf8_lossy(&environ).contains(text))
        })
        .collect()
}

/// Waits until `done` holds, or `patience` passes, and returns whether it held.
pub fn eventually(patience: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    loop {
        if done() {
            return true;
        }
        if started.elapsed() > patience {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
