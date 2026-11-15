mod conversation;
mod daemon;
mod digest;
mod handoff;
mod instance;
mod lifetime;
mod provider;
mod runtime;
mod server;
mod store;

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crowbot_protocol::transport::Listener;
use crowbot_protocol::{EXIT_DATABASE_HELD, EXIT_ENDPOINT_HELD, Endpoint};

use crate::daemon::Daemon;
use crate::instance::LockFailure;
use crate::runtime::Runtimes;
use crate::store::{SharedStore, Store};

const USAGE: &str =
    "usage: crowbotd [--db <path>] [--bots-dir <dir>] [--data-dir <dir>] [--agent <command>]
                [--runtime-manifest <file>] [--endpoint <address>]

The daemon listens on a local endpoint and serves Crow Bot clients until it is stopped.
A bot created with no folder gets one of its own under --bots-dir (~/CrowBot/Bots).
The agent runtimes the daemon installs, and the files providers keep for bots, go
under --data-dir (the folder of the database). --runtime-manifest replaces the pinned
runtimes by the ones a JSON file lists, for a mirror.";

struct Args {
    db: Option<PathBuf>,
    bots_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    command: Option<String>,
    runtime_manifest: Option<PathBuf>,
    endpoint: Option<Endpoint>,
}

fn parse_args(mut raw: impl Iterator<Item = String>) -> Result<Args, String> {
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
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ => return Err(format!("unexpected argument: {arg}\n\n{USAGE}")),
        }
    }
    Ok(args)
}

fn default_db_path() -> Result<PathBuf, String> {
    std::env::home_dir()
        .map(|home| home.join(".crowbot").join("crowbot.db"))
        .ok_or_else(|| "cannot find the home directory; pass --db <path>".to_owned())
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

/// The folder `~/.crowbot` holds the database, the log, and the socket of
/// the default daemon: only its user may enter it. It is made so, or kept so
/// when the client or a permissive umask made it first.
#[cfg(unix)]
fn private_folder(folder: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
    let metadata = std::fs::metadata(folder)
        .map_err(|error| format!("cannot read {}: {error}", folder.display()))?;
    // SAFETY: `geteuid` has no preconditions.
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "{} belongs to another account, so it could read the database",
            folder.display()
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot make {} private: {error}", folder.display()))?;
        eprintln!("[crowbotd] made {} private to its user", folder.display());
    }
    Ok(())
}

/// Starts the daemon in this order: lock the database, open it, end the turns
/// a previous daemon left running, then claim the endpoint. An endpoint that
/// answers therefore belongs to a daemon that is ready, and a daemon that
/// fails to start is never reached.
async fn run(args: Args) -> Result<(), StartError> {
    #[cfg(unix)]
    if args.db.is_none() || args.endpoint.is_none() {
        let home = std::env::home_dir().ok_or("cannot find the home directory")?;
        private_folder(&home.join(".crowbot"))?;
    }
    let db = match args.db {
        Some(path) => path,
        None => default_db_path()?,
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
    let endpoint = match args.endpoint {
        Some(endpoint) => endpoint,
        None => {
            Endpoint::local().ok_or("cannot choose a local endpoint; pass --endpoint <address>")?
        }
    };
    eprintln!("[crowbotd] process {}", std::process::id());
    let _locked =
        instance::lock_database(&db, &endpoint)
            .await
            .map_err(|failure| match failure {
                LockFailure::Held(message) => StartError::DatabaseHeld(message),
                LockFailure::Failed(message) => StartError::Failed(message),
            })?;
    let store = Store::open(&db).map_err(|error| error.to_string())?;
    eprintln!("[crowbotd] database {}", db.display());
    let interrupted = store.recover().map_err(|error| error.to_string())?;
    if interrupted > 0 {
        eprintln!("[crowbotd] ended {interrupted} turn(s) that a previous daemon left running");
    }
    let listener = Listener::bind(&endpoint).map_err(|error| match error.kind() {
        ErrorKind::PermissionDenied | ErrorKind::AddrInUse => StartError::EndpointHeld(format!(
            "another program already holds the endpoint {endpoint}: another crowbotd is running, or another program took the name"
        )),
        _ => StartError::Failed(format!("cannot listen on {endpoint}: {error}")),
    })?;
    eprintln!("[crowbotd] listening on {endpoint}");
    let daemon = Arc::new(Daemon::new(
        SharedStore::new(store),
        db,
        bots_dir,
        data_dir,
        args.command,
        runtimes,
    ));
    server::serve(daemon, listener).await;
    Ok(())
}

fn main() -> ExitCode {
    #[cfg(unix)]
    {
        let mut raw = std::env::args_os().skip(1).peekable();
        if raw.peek().is_some_and(|first| first == lifetime::GUARD) {
            raw.next();
            return lifetime::guard(raw);
        }
    }
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
    runtime.block_on(serve())
}

async fn serve() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let (message, status) = match run(args).await {
        Ok(()) => return ExitCode::SUCCESS,
        Err(StartError::Failed(message)) => (message, ExitCode::FAILURE),
        Err(StartError::DatabaseHeld(message)) => (message, ExitCode::from(EXIT_DATABASE_HELD)),
        Err(StartError::EndpointHeld(message)) => (message, ExitCode::from(EXIT_ENDPOINT_HELD)),
    };
    eprintln!("[crowbotd] failed: {message}");
    status
}
