mod conversation;
mod provider;
mod server;
mod store;

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crowbot_protocol::Endpoint;
use crowbot_protocol::transport::Listener;

use crate::server::Daemon;
use crate::store::{SharedStore, Store};

const USAGE: &str =
    "usage: crowbotd [--db <path>] [--cwd <dir>] [--agent <command>] [--endpoint <address>]

The daemon listens on a local endpoint and serves Crow Bot clients until it is stopped.";

struct Args {
    db: Option<PathBuf>,
    cwd: Option<PathBuf>,
    command: Option<String>,
    endpoint: Option<Endpoint>,
}

fn parse_args(mut raw: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        db: None,
        cwd: None,
        command: None,
        endpoint: None,
    };
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--db" => args.db = Some(PathBuf::from(raw.next().ok_or("--db needs a path")?)),
            "--cwd" => args.cwd = Some(PathBuf::from(raw.next().ok_or("--cwd needs a directory")?)),
            "--agent" => args.command = Some(raw.next().ok_or("--agent needs a command")?),
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

async fn run(args: Args) -> Result<(), String> {
    let db = match args.db {
        Some(path) => path,
        None => default_db_path()?,
    };
    let cwd = match args.cwd {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    let endpoint = match args.endpoint {
        Some(endpoint) => endpoint,
        None => {
            Endpoint::local().ok_or("cannot choose a local endpoint; pass --endpoint <address>")?
        }
    };
    let listener = Listener::bind(&endpoint).map_err(|error| match error.kind() {
        ErrorKind::PermissionDenied | ErrorKind::AddrInUse => {
            format!("another crowbotd is already listening on {endpoint}")
        }
        _ => format!("cannot listen on {endpoint}: {error}"),
    })?;
    let store = Store::open(&db).map_err(|error| error.to_string())?;
    eprintln!("[crowbotd] database {}", db.display());
    eprintln!("[crowbotd] listening on {endpoint}");
    let daemon = Arc::new(Daemon::new(SharedStore::new(store), cwd, args.command));
    server::serve(daemon, listener).await;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("[crowbotd] failed: {message}");
            ExitCode::FAILURE
        }
    }
}
