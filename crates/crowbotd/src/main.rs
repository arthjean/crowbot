mod conversation;
mod daemon;
mod digest;
mod handoff;
mod instance;
mod lifetime;
mod private;
mod provider;
mod runtime;
mod server;
mod stop;
mod store;
mod systemd;

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use crowbot_protocol::transport::Listener;
use crowbot_protocol::{
    EXIT_DATABASE_HELD, EXIT_ENDPOINT_HELD, EXIT_ENDPOINT_TOO_LONG, Endpoint, Folders,
    SOCKET_PATH_LIMIT,
};
use tokio::sync::{oneshot, watch};

use crate::daemon::Daemon;
use crate::instance::LockFailure;
use crate::runtime::Runtimes;
use crate::store::{SharedStore, Store};

const USAGE: &str =
    "usage: crowbotd [--db <path>] [--bots-dir <dir>] [--data-dir <dir>] [--agent <command>]
                [--runtime-manifest <file>] [--endpoint <address>]
       crowbotd --help | --version

The daemon listens on a local endpoint and serves Crow Bot clients until it is stopped.
SIGTERM, SIGINT, or SIGHUP stops it: the running turns end, then it exits with
status 0. A second signal stops it at once.
A bot created with no folder gets one of its own under --bots-dir (~/CrowBot/Bots).
The agent runtimes the daemon installs, and the files providers keep for bots, go
under --data-dir (the folder of the database). --runtime-manifest replaces the pinned
runtimes by the ones a JSON file lists, for a mirror.

On Linux the database, the runtimes, and the provider homes go under
$XDG_DATA_HOME/crowbot (~/.local/share/crowbot), and the socket under
$XDG_RUNTIME_DIR/crowbot. Elsewhere they go under ~/.crowbot.

Exit status: 1 when the daemon cannot start, 2 for a wrong argument, 3 when another
crowbotd uses the database, 4 when another program holds the endpoint, 5 when the
path of the socket is too long.";

/// What the arguments ask for: a daemon to run, or a text to print.
enum Command {
    Run(Args),
    Print(String),
}

struct Args {
    db: Option<PathBuf>,
    bots_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    command: Option<String>,
    runtime_manifest: Option<PathBuf>,
    endpoint: Option<Endpoint>,
}

fn parse_args(mut raw: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = Args {
        db: None,
        bots_dir: None,
        data_dir: None,
        command: None,
        runtime_manifest: None,
        endpoint: None,
    };
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--db" => args.db = Some(PathBuf::from(raw.next().ok_or("--db needs a path")?)),
            "--bots-dir" => {
                args.bots_dir = Some(PathBuf::from(
                    raw.next().ok_or("--bots-dir needs a directory")?,
                ));
            }
            "--data-dir" => {
                args.data_dir = Some(PathBuf::from(
                    raw.next().ok_or("--data-dir needs a directory")?,
                ));
            }
            "--agent" => args.command = Some(raw.next().ok_or("--agent needs a command")?),
            "--runtime-manifest" => {
                args.runtime_manifest = Some(PathBuf::from(
                    raw.next().ok_or("--runtime-manifest needs a file")?,
                ));
            }
            "--endpoint" => {
                args.endpoint = Some(Endpoint::new(
                    raw.next().ok_or("--endpoint needs an address")?,
                ));
            }
            "--help" | "-h" => return Ok(Command::Print(USAGE.to_owned())),
            "--version" | "-V" => return Ok(Command::Print(version())),
            _ => return Err(format!("unexpected argument: {arg}\n\n{USAGE}")),
        }
    }
    Ok(Command::Run(args))
}

fn version() -> String {
    format!("crowbotd {}", env!("CARGO_PKG_VERSION"))
}

/// Where the bots that were given no folder get one of their own.
fn default_bots_dir() -> Result<PathBuf, String> {
    std::env::home_dir()
        .map(|home| home.join("CrowBot").join("Bots"))
        .ok_or_else(|| "cannot find the home directory; pass --bots-dir <dir>".to_owned())
}

/// Why a daemon did not start, each with its exit status.
enum StartError {
    /// Status 1.
    Failed(String),
    /// `EXIT_DATABASE_HELD`: another daemon uses the database.
    DatabaseHeld(String),
    /// `EXIT_ENDPOINT_HELD`: another program holds the endpoint.
    EndpointHeld(String),
    /// `EXIT_ENDPOINT_TOO_LONG`: the socket path does not fit.
    EndpointTooLong(String),
}

impl From<String> for StartError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<&str> for StartError {
    fn from(message: &str) -> Self {
        Self::Failed(message.to_owned())
    }
}

/// How long the turns of a daemon that stops get to end before their agents
/// are closed, and how long the daemon waits for them in all: it exits within
/// 5 seconds of the signal.
const STOP_PATIENCE: Duration = Duration::from_secs(3);
const STOP_DEADLINE: Duration = Duration::from_millis(3_500);

/// Starts the daemon in this order: check the endpoint, lock the database,
/// open it, end the turns a previous daemon left running, then claim the
/// endpoint. An endpoint that answers therefore belongs to a daemon that is
/// ready, and a daemon that fails to start is never reached. A socket path
/// that cannot be used is refused before any file or folder exists.
async fn run(args: Args) -> Result<(), StartError> {
    let folders = Folders::local();
    let local = || folders.clone().ok_or("cannot find the home directory");
    let endpoint = match args.endpoint {
        Some(endpoint) => endpoint,
        None => {
            let folders = local()?;
            if folders.runtime_missing && cfg!(unix) {
                eprintln!(
                    "[crowbotd] warning: XDG_RUNTIME_DIR is not set, so the socket goes in {}",
                    folders.runtime.display()
                );
            }
            Endpoint::local().ok_or("cannot choose a local endpoint; pass --endpoint <address>")?
        }
    };
    if let Some(length) = endpoint.too_long() {
        return Err(StartError::EndpointTooLong(format!(
            "the socket path {endpoint} is {length} bytes long, over the limit of {SOCKET_PATH_LIMIT} bytes; pass a shorter --endpoint"
        )));
    }
    let default_db = args.db.is_none();
    let db = match args.db {
        Some(path) => path,
        None => local()
            .map_err(|_| "cannot find the home directory; pass --db <path>")?
            .database(),
    };
    let bots_dir = match args.bots_dir {
        Some(dir) => std::path::absolute(&dir)
            .map_err(|error| format!("cannot use {} for bots: {error}", dir.display()))?,
        None => default_bots_dir()?,
    };
    let absolute = |dir: &PathBuf| {
        std::path::absolute(dir)
            .map_err(|error| format!("cannot use {} for data: {error}", dir.display()))
    };
    let data_dir = match (&args.data_dir, db.parent()) {
        (Some(dir), _) => absolute(dir)?,
        (None, Some(folder)) if !folder.as_os_str().is_empty() => absolute(&folder.to_owned())?,
        (None, _) => absolute(&PathBuf::from("."))?,
    };
    let manifest = args
        .runtime_manifest
        .as_deref()
        .map(Runtimes::read_manifest)
        .transpose()?;
    let runtimes = Runtimes::new(data_dir.join("runtimes"), manifest);
    // The folder of the database is the user's alone. A folder of Crow Bot's
    // own is made so; one the user chose keeps its mode.
    let db_folder = absolute(&db.parent().map_or_else(PathBuf::new, Path::to_owned))?;
    let own_folder = default_db || folders.as_ref().is_some_and(|f| f.holds(&db_folder));
    private::database_folder(&db_folder, own_folder)?;
    #[cfg(unix)]
    if Some(endpoint.address()) == Endpoint::local().as_ref().map(Endpoint::address)
        && let Some(folders) = &folders
    {
        // The default socket's folder, which `Listener::bind` creates 0700.
        private::database_folder(&folders.runtime, true)?;
    }
    eprintln!("[crowbotd] process {}", std::process::id());
    let _locked =
        instance::lock_database(&db, &endpoint)
            .await
            .map_err(|failure| match failure {
                LockFailure::Held(message) => StartError::DatabaseHeld(message),
                LockFailure::Failed(message) => StartError::Failed(message),
            })?;
    let store = open_store(&db)?;
    eprintln!("[crowbotd] database {}", db.display());
    let interrupted = store.recover().map_err(|error| error.to_string())?;
    if interrupted > 0 {
        eprintln!("[crowbotd] ended {interrupted} turn(s) that a previous daemon left running");
    }
    let mut signals =
        stop::Signals::new().map_err(|error| format!("cannot handle signals: {error}"))?;
    let listener = Listener::bind(&endpoint).map_err(|error| match error.kind() {
        ErrorKind::PermissionDenied | ErrorKind::AddrInUse => StartError::EndpointHeld(format!(
            "another program already holds the endpoint {endpoint}: another crowbotd is running, or another program took the name"
        )),
        _ => StartError::Failed(format!("cannot listen on {endpoint}: {error}")),
    })?;
    eprintln!("[crowbotd] listening on {endpoint}");
    systemd::ready();
    let store = SharedStore::new(store);
    let daemon = Arc::new(Daemon::new(
        store.clone(),
        db,
        bots_dir,
        data_dir,
        args.command,
        runtimes,
    ));
    let (listen, stop_listening) = oneshot::channel();
    let (close, closing) = watch::channel(false);
    let mut serving = tokio::spawn(server::serve(
        daemon.clone(),
        listener,
        server::Shutdown {
            listen: stop_listening,
            close: closing,
        },
    ));
    let signal = tokio::select! {
        signal = signals.next() => signal,
        _ = &mut serving => return Err("the daemon stopped serving".into()),
    };
    eprintln!("[crowbotd] stopping on {signal}; a second signal stops it at once");
    systemd::stopping();
    tokio::spawn(async move {
        let signal = signals.next().await;
        eprintln!("[crowbotd] stopped at once on a second {signal}");
        std::process::exit(128 + signal.number());
    });
    listen.send(()).ok();
    daemon
        .stop(STOP_PATIENCE, tokio::time::Instant::now() + STOP_DEADLINE)
        .await;
    close.send(true).ok();
    serving.await.ok();
    if let Err(error) = store.close() {
        eprintln!("[crowbotd] cannot close the database: {error}");
    }
    eprintln!("[crowbotd] stopped");
    Ok(())
}

/// Opens the database. A new database is created 0600 first, on Unix, so its
/// `-wal` and `-shm` files, which SQLite creates with the mode of the
/// database, are private too; files that exist are made private.
fn open_store(db: &Path) -> Result<Store, String> {
    let failed = |error: std::io::Error| format!("cannot open {}: {error}", db.display());
    private::options()
        .create(true)
        .append(true)
        .open(db)
        .map_err(failed)?;
    for extension in ["", "-wal", "-shm"] {
        let mut path = db.as_os_str().to_os_string();
        path.push(extension);
        private::restrict(Path::new(&path)).map_err(failed)?;
    }
    Store::open(db).map_err(|error| error.to_string())
}

fn main() -> ExitCode {
    systemd::take_notify_socket();
    #[cfg(unix)]
    {
        let mut raw = std::env::args_os().skip(1).peekable();
        if raw.peek().is_some_and(|first| first == lifetime::GUARD) {
            raw.next();
            return lifetime::guard(raw);
        }
    }
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Command::Run(args)) => args,
        Ok(Command::Print(text)) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    // Before any process starts: each one is then bound to the daemon.
    if let Err(error) = lifetime::bind_children() {
        eprintln!("[crowbotd] the agents will not end with the daemon: {error}");
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("[crowbotd] failed: cannot start the runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let status = runtime.block_on(serve(args));
    // A task that still blocks a thread does not hold the exit back.
    runtime.shutdown_timeout(Duration::from_millis(500));
    status
}

async fn serve(args: Args) -> ExitCode {
    let (message, status) = match run(args).await {
        Ok(()) => return ExitCode::SUCCESS,
        Err(StartError::Failed(message)) => (message, ExitCode::FAILURE),
        Err(StartError::DatabaseHeld(message)) => (message, ExitCode::from(EXIT_DATABASE_HELD)),
        Err(StartError::EndpointHeld(message)) => (message, ExitCode::from(EXIT_ENDPOINT_HELD)),
        Err(StartError::EndpointTooLong(message)) => {
            (message, ExitCode::from(EXIT_ENDPOINT_TOO_LONG))
        }
    };
    eprintln!("[crowbotd] failed: {message}");
    status
}
