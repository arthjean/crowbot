//! The agent runtimes the daemon installs: the download at the first turn,
//! the check of its hash, the download that fails, and the executable a user
//! sets in its place. The publisher is a local HTTP server of the test, and
//! the runtime it serves is the scripted agent.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crowbot_protocol::{
    BotDraft, ClientMessage, Event, ProviderState, ProviderStatus, ServerMessage, TurnEnd,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::support::{self, Client, Daemon};

const CLAUDE: &str = "claude";
const VERSION: &str = "test-1";
const CHUNK: usize = 16 << 10;

fn program() -> String {
    format!("agent{}", std::env::consts::EXE_SUFFIX)
}

fn runtime() -> Vec<u8> {
    std::fs::read(support::fake_agent()).unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The publisher of the test runtime.
struct Publisher {
    port: u16,
    requests: Arc<AtomicUsize>,
}

impl Publisher {
    /// Serves the runtime, with a pause after each part of it sent.
    fn start(pause: Duration) -> Self {
        Self::on(TcpListener::bind("127.0.0.1:0").unwrap(), pause)
    }

    fn on(listener: TcpListener, pause: Duration) -> Self {
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = requests.clone();
        let body = Arc::new(runtime());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let body = body.clone();
                std::thread::spawn(move || serve(stream, &body, pause));
            }
        });
        Self { port, requests }
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

fn serve(mut stream: TcpStream, body: &[u8], pause: Duration) {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => request.push(byte[0]),
            _ => return,
        }
    }
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    for part in body.chunks(CHUNK) {
        if stream.write_all(part).is_err() || stream.flush().is_err() {
            return;
        }
        std::thread::sleep(pause);
    }
}

/// A port of this computer that nothing listens on.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn manifest(port: u16, sha256: &str, bytes: usize) -> Value {
    json!({
        CLAUDE: {
            "name": "claude-agent-acp",
            "version": VERSION,
            "artifacts": [{
                "url": format!("http://127.0.0.1:{port}/agent"),
                "sha256": sha256,
                "bytes": bytes,
                "unpack": "file",
                "file": program()
            }],
            "program": program()
        }
    })
}

/// A daemon whose Claude runtime is the one the publisher serves.
fn daemon(publisher: &Publisher) -> Daemon {
    let runtime = runtime();
    Daemon::start_with_providers(
        &manifest(publisher.port, &sha256(&runtime), runtime.len()),
        &[],
    )
}

fn installed(daemon: &Daemon) -> PathBuf {
    daemon
        .data_dir()
        .join("runtimes")
        .join(CLAUDE)
        .join(VERSION)
}

async fn providers(client: &mut Client) -> Vec<ProviderStatus> {
    client.send(ClientMessage::ListProviders).await;
    client
        .until(|message| match message {
            ServerMessage::Providers(providers) => Some(providers),
            _ => None,
        })
        .await
}

async fn claude_bot(client: &mut Client) -> i64 {
    let bot = client
        .create_bot(BotDraft {
            name: "Atlas".to_owned(),
            ..BotDraft::default()
        })
        .await;
    assert_eq!(bot.bot.provider, CLAUDE);
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;
    conversation
}

/// What a client sees of a turn: the reply, how the turn ended, and each
/// step of the download that was reported before the first text.
struct Turn {
    reply: String,
    end: Event,
    downloads: Vec<(u64, u64)>,
}

async fn turn(client: &mut Client, conversation: i64, text: &str) -> Turn {
    client.say(conversation, text).await;
    let mut turn = Turn {
        reply: String::new(),
        end: Event::TurnEnded(TurnEnd::Other),
        downloads: Vec::new(),
    };
    loop {
        match client.next().await {
            ServerMessage::ProviderChanged(status) if status.id == CLAUDE => {
                if let Some(install) = status.install {
                    assert!(
                        turn.reply.is_empty(),
                        "the turn started before the download ended"
                    );
                    turn.downloads.push((install.received, install.total));
                }
            }
            ServerMessage::ConversationEvent { event, .. } => match event {
                Event::MessageText(text) => turn.reply.push_str(&text),
                event if event.ends_turn() => {
                    turn.end = event;
                    return turn;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn failure(turn: &Turn) -> (&str, Option<&str>) {
    match &turn.end {
        Event::TurnFailed {
            message, provider, ..
        } => (message.as_str(), provider.as_deref()),
        other => panic!("the turn did not fail: {other:?}"),
    }
}

// US-032: managed agent runtimes.

#[tokio::test]
async fn the_first_turn_downloads_the_pinned_runtime_checks_it_and_runs_it() {
    let publisher = Publisher::start(Duration::from_millis(15));
    let daemon = daemon(&publisher);
    let mut client = daemon.client().await;

    let listed = providers(&mut client).await;
    let names: Vec<&str> = listed.iter().map(|status| status.id.as_str()).collect();
    assert_eq!(names, ["claude", "codex", "grok"]);
    let claude = &listed[0];
    assert_eq!(claude.state, ProviderState::NotInstalled, "{claude:?}");
    assert_eq!(claude.runtime.as_deref(), Some("claude-agent-acp test-1"));
    assert_eq!(publisher.requests(), 0, "a check downloaded the runtime");
    assert!(!installed(&daemon).exists());

    let conversation = claude_bot(&mut client).await;
    let first = turn(&mut client, conversation, "say caw").await;
    assert_eq!(first.end, Event::TurnEnded(TurnEnd::Completed));
    assert_eq!(first.reply, "caw");
    // The client followed the download, and the turn started once it ended.
    let total = runtime().len() as u64;
    assert!(first.downloads.len() > 2, "{:?}", first.downloads);
    assert!(first.downloads.iter().all(|step| step.1 == total));
    assert!(
        first
            .downloads
            .iter()
            .any(|step| step.0 > 0 && step.0 < total)
    );
    assert!(
        first
            .downloads
            .windows(2)
            .all(|pair| pair[0].0 <= pair[1].0)
    );

    // The runtime is stored under the folder of the daemon, as it was served.
    let stored = std::fs::read(installed(&daemon).join(program())).unwrap();
    assert_eq!(sha256(&stored), sha256(&runtime()));
    assert!(installed(&daemon).join("installed.json").is_file());
    assert_eq!(publisher.requests(), 1);

    // It is downloaded once: a later turn, a new bot, and a new daemon run it.
    let second = turn(&mut client, conversation, "say again").await;
    assert_eq!(second.reply, "again");
    assert!(second.downloads.is_empty());
    let other = claude_bot(&mut client).await;
    assert_eq!(turn(&mut client, other, "say caw").await.reply, "caw");
    assert_eq!(publisher.requests(), 1);
}

#[tokio::test]
async fn a_download_that_does_not_match_its_hash_is_removed_and_nothing_of_it_runs() {
    let publisher = Publisher::start(Duration::ZERO);
    let bytes = runtime().len();
    let pinned = "0".repeat(64);
    let daemon = Daemon::start_with_providers(&manifest(publisher.port, &pinned, bytes), &[]);
    let mut client = daemon.client().await;
    let conversation = claude_bot(&mut client).await;

    let failed = turn(&mut client, conversation, "say caw").await;
    let (message, provider) = failure(&failed);
    assert_eq!(provider, Some(CLAUDE));
    assert!(
        message.starts_with("The Claude runtime was not installed"),
        "{message}"
    );
    assert!(message.contains("SHA-256"), "{message}");
    assert!(message.contains(&pinned), "{message}");
    assert!(message.contains(&sha256(&runtime())), "{message}");
    assert!(message.contains("nothing of it was run"), "{message}");
    assert_eq!(publisher.requests(), 1);

    // The file is gone, and the agent in it never started.
    let runtimes = daemon.data_dir().join("runtimes").join(CLAUDE);
    let left: Vec<PathBuf> = std::fs::read_dir(&runtimes)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(daemon.heartbeat(), None);
    assert_eq!(failed.reply, "");
}

#[tokio::test]
async fn a_runtime_changed_after_its_install_is_downloaded_again_before_it_runs() {
    let publisher = Publisher::start(Duration::ZERO);
    let mut daemon = daemon(&publisher);
    let mut client = daemon.client().await;
    let conversation = claude_bot(&mut client).await;
    assert_eq!(
        turn(&mut client, conversation, "say caw").await.reply,
        "caw"
    );
    assert_eq!(publisher.requests(), 1);
    drop(client);
    // The daemon ends with its agent, which holds its program open.
    daemon.kill();

    // What a bot that may write files could do to the runtime: change the
    // program, and add a file that a program loads from its own folder.
    let program = installed(&daemon).join(program());
    std::fs::write(&program, "not the pinned agent").unwrap();
    std::fs::write(installed(&daemon).join("version.dll"), "planted").unwrap();
    daemon.spawn();
    let mut client = daemon.client().await;
    client.open(conversation).await;

    let next = turn(&mut client, conversation, "say again").await;
    assert_eq!(
        next.end,
        Event::TurnEnded(TurnEnd::Completed),
        "{:?}",
        next.end
    );
    assert_eq!(next.reply, "again");
    assert!(!next.downloads.is_empty(), "the changed runtime ran");
    assert_eq!(publisher.requests(), 2);
    assert_eq!(
        sha256(&std::fs::read(&program).unwrap()),
        sha256(&runtime())
    );
    assert!(!installed(&daemon).join("version.dll").exists());
}

#[tokio::test]
async fn a_changed_runtime_that_cannot_be_removed_does_not_run() {
    let publisher = Publisher::start(Duration::ZERO);
    let mut daemon = daemon(&publisher);
    let mut client = daemon.client().await;
    let conversation = claude_bot(&mut client).await;
    assert_eq!(
        turn(&mut client, conversation, "say caw").await.reply,
        "caw"
    );
    drop(client);
    daemon.kill();

    // A program that still runs, and differs from the pinned one by a byte.
    let folder = installed(&daemon);
    let mut changed = runtime();
    changed.push(0);
    std::fs::write(folder.join(program()), changed).unwrap();
    let heartbeat = daemon.data_dir().join("heartbeat");
    std::fs::remove_file(&heartbeat).ok();
    // The marker cannot be removed: on Windows another process holds it with
    // no share for removal, and on Unix its folder takes no change.
    #[cfg(windows)]
    let _held = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(folder.join("installed.json"))
            .unwrap()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    daemon.spawn();
    let mut client = daemon.client().await;
    client.open(conversation).await;

    let next = turn(&mut client, conversation, "say again").await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(
        matches!(next.end, Event::TurnFailed { .. }),
        "{:?}",
        next.end
    );
    assert_eq!(daemon.heartbeat(), None, "the changed runtime ran");
}

#[tokio::test]
async fn a_download_larger_than_its_pinned_size_stops_and_nothing_of_it_runs() {
    let publisher = Publisher::start(Duration::ZERO);
    let runtime = runtime();
    let pinned = runtime.len() - 1;
    let daemon =
        Daemon::start_with_providers(&manifest(publisher.port, &sha256(&runtime), pinned), &[]);
    let mut client = daemon.client().await;
    let conversation = claude_bot(&mut client).await;

    let failed = turn(&mut client, conversation, "say caw").await;
    let (message, provider) = failure(&failed);
    assert_eq!(provider, Some(CLAUDE));
    assert!(
        message.contains(&format!("larger than the pinned {pinned} bytes")),
        "{message}"
    );
    let runtimes = daemon.data_dir().join("runtimes").join(CLAUDE);
    let left: Vec<PathBuf> = std::fs::read_dir(&runtimes)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(daemon.heartbeat(), None);
}

#[tokio::test]
async fn a_stop_during_the_download_ends_the_turn() {
    let publisher = Publisher::start(Duration::from_millis(120));
    let daemon = daemon(&publisher);
    let mut client = daemon.client().await;
    let conversation = claude_bot(&mut client).await;

    client.say(conversation, "say caw").await;
    client
        .until(|message| match message {
            ServerMessage::ProviderChanged(status) if status.id == CLAUDE => {
                status.install.filter(|install| install.received > 0)
            }
            _ => None,
        })
        .await;
    client
        .send(ClientMessage::StopTurn {
            conversation_id: conversation,
        })
        .await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Cancelled)
    );
    assert!(!installed(&daemon).exists());
    assert_eq!(daemon.heartbeat(), None);
}

#[tokio::test]
async fn offline_the_turn_fails_by_the_name_of_the_runtime_and_works_once_the_network_is_back() {
    let port = closed_port();
    let runtime = runtime();
    let daemon =
        Daemon::start_with_providers(&manifest(port, &sha256(&runtime), runtime.len()), &[]);
    let mut client = daemon.client().await;
    let conversation = claude_bot(&mut client).await;

    let offline = turn(&mut client, conversation, "say caw").await;
    assert_eq!(
        failure(&offline),
        (
            "The Claude runtime is not installed and cannot be downloaded offline.",
            Some(CLAUDE)
        )
    );
    assert!(!installed(&daemon).exists());

    // The network is back: the same bot answers.
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    let publisher = Publisher::on(listener, Duration::ZERO);
    let online = turn(&mut client, conversation, "say caw").await;
    assert_eq!(online.end, Event::TurnEnded(TurnEnd::Completed));
    assert_eq!(online.reply, "caw");
    assert_eq!(publisher.requests(), 1);
}

#[tokio::test]
async fn an_executable_the_user_set_runs_in_place_of_the_runtime() {
    let publisher = Publisher::start(Duration::ZERO);
    let daemon = daemon(&publisher);
    let mut client = daemon.client().await;
    let own = daemon.folder("own").join(program());
    std::fs::copy(support::fake_agent(), &own).unwrap();
    let before = sha256(&std::fs::read(&own).unwrap());
    let set = |path: Option<String>| ClientMessage::SetProviderExecutable {
        provider: CLAUDE.to_owned(),
        path,
    };

    client
        .send(set(Some(
            daemon
                .folder("own")
                .join("missing")
                .to_string_lossy()
                .into_owned(),
        )))
        .await;
    assert_eq!(client.until_invalid().await, "executable");
    client.send(set(Some("relative/agent".to_owned()))).await;
    assert_eq!(client.until_invalid().await, "executable");

    client
        .send(set(Some(own.to_string_lossy().into_owned())))
        .await;
    let status = client
        .until(|message| match message {
            ServerMessage::ProviderChanged(status)
                if status.id == CLAUDE && status.executable.is_some() && !status.checking =>
            {
                Some(status)
            }
            _ => None,
        })
        .await;
    assert_eq!(status.executable.as_deref(), own.to_str());
    assert_ne!(status.state, ProviderState::NotInstalled);

    let conversation = claude_bot(&mut client).await;
    let answered = turn(&mut client, conversation, "say caw").await;
    assert_eq!(answered.reply, "caw");
    assert!(answered.downloads.is_empty());
    // Nothing was downloaded, and the executable is as the user left it.
    assert_eq!(publisher.requests(), 0);
    assert!(!installed(&daemon).exists());
    assert_eq!(sha256(&std::fs::read(&own).unwrap()), before);

    // Without it, the daemon goes back to the runtime it installs.
    client.send(set(None)).await;
    let cleared = client
        .until(|message| match message {
            ServerMessage::ProviderChanged(status)
                if status.id == CLAUDE && status.executable.is_none() =>
            {
                Some(status)
            }
            _ => None,
        })
        .await;
    assert_eq!(cleared.state, ProviderState::NotInstalled);
}

#[tokio::test]
async fn a_runtime_can_be_installed_before_the_first_turn() {
    let publisher = Publisher::start(Duration::ZERO);
    let daemon = daemon(&publisher);
    let mut client = daemon.client().await;
    providers(&mut client).await;
    client
        .send(ClientMessage::InstallRuntime {
            provider: CLAUDE.to_owned(),
        })
        .await;
    client
        .until(|message| match message {
            ServerMessage::ProviderChanged(status)
                if status.id == CLAUDE
                    && status.install.is_none()
                    && !status.checking
                    && status.state != ProviderState::NotInstalled =>
            {
                Some(status)
            }
            ServerMessage::Error { message } => panic!("the daemon refused: {message}"),
            _ => None,
        })
        .await;
    assert!(installed(&daemon).join(program()).is_file());
    let conversation = claude_bot(&mut client).await;
    let answered = turn(&mut client, conversation, "say caw").await;
    assert_eq!(answered.reply, "caw");
    assert!(answered.downloads.is_empty());
    assert_eq!(publisher.requests(), 1);
}
