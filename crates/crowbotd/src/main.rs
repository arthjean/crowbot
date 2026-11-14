mod event;
mod provider;
mod store;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::event::{ApprovalPolicy, Event};
use crate::provider::acp::{self, AgentProfile, SessionConfig};
use crate::provider::claude;
use crate::store::{Conversation, Recorder, Store, StoreError};

const USAGE: &str = "usage: crowbotd [--db <path>] [--cwd <dir>] [--agent <command>] [--approve] \
[--conversation <id>] [--history] [<prompt>]

With a prompt, the daemon runs one turn. Without one, it reads one prompt per line from standard input.";

const CUSTOM_PROVIDER: &str = "custom";

struct Args {
    db: Option<PathBuf>,
    cwd: Option<PathBuf>,
    command: Option<String>,
    approvals: ApprovalPolicy,
    conversation: Option<i64>,
    history: bool,
    prompt: Option<String>,
}

fn parse_args(mut raw: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        db: None,
        cwd: None,
        command: None,
        approvals: ApprovalPolicy::Decline,
        conversation: None,
        history: false,
        prompt: None,
    };
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--db" => args.db = Some(PathBuf::from(raw.next().ok_or("--db needs a path")?)),
            "--cwd" => args.cwd = Some(PathBuf::from(raw.next().ok_or("--cwd needs a directory")?)),
            "--agent" => args.command = Some(raw.next().ok_or("--agent needs a command")?),
            "--approve" => args.approvals = ApprovalPolicy::AllowOnce,
            "--conversation" => {
                let id = raw.next().ok_or("--conversation needs an id")?;
                args.conversation = Some(
                    id.parse()
                        .map_err(|_| format!("not a conversation id: {id}"))?,
                );
            }
            "--history" => args.history = true,
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ if args.prompt.is_none() => args.prompt = Some(arg),
            _ => return Err(format!("unexpected argument: {arg}")),
        }
    }
    Ok(args)
}

fn default_db_path() -> Result<PathBuf, String> {
    std::env::home_dir()
        .map(|home| home.join(".crowbot").join("crowbot.db"))
        .ok_or_else(|| "cannot find the home directory; pass --db <path>".to_owned())
}

fn open_conversation(store: &Store, args: &Args) -> Result<Conversation, String> {
    if let Some(id) = args.conversation {
        return store
            .conversation(id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("conversation {id} does not exist"));
    }
    let cwd = match &args.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    let provider = if args.command.is_some() {
        CUSTOM_PROVIDER
    } else {
        claude::PROVIDER
    };
    store
        .create_conversation(provider, args.command.as_deref(), &cwd)
        .map_err(|error| error.to_string())
}

fn agent_profile(conversation: &Conversation) -> Result<AgentProfile, String> {
    match (conversation.provider.as_str(), &conversation.command) {
        (claude::PROVIDER, _) => Ok(claude::profile()),
        (CUSTOM_PROVIDER, Some(command)) => Ok(AgentProfile::generic(command.clone())),
        (provider, _) => Err(format!("unknown provider: {provider}")),
    }
}

fn prompt_source(prompt: Option<String>) -> UnboundedReceiver<String> {
    let (sender, receiver) = mpsc::unbounded_channel();
    match prompt {
        Some(prompt) => {
            sender.send(prompt).ok();
        }
        None => {
            std::thread::spawn(move || {
                for line in std::io::stdin().lines() {
                    let Ok(line) = line else { break };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if sender.send(line.to_owned()).is_err() {
                        break;
                    }
                }
            });
        }
    }
    receiver
}

fn print_event(event: &Event) {
    match event {
        Event::SessionStarted { .. } | Event::ThoughtText(_) => {}
        Event::UserText(text) => eprintln!("\n> {text}"),
        Event::MessageText(text) => {
            print!("{text}");
            std::io::stdout().flush().ok();
        }
        Event::ToolCallStarted { id, title, status } => {
            eprintln!("\n[tool {id}] started: {title} ({status:?})");
        }
        Event::ToolCallUpdated { id, title, status } => {
            eprintln!(
                "[tool {id}] updated: {} ({status:?})",
                title.as_deref().unwrap_or("-")
            );
        }
        Event::ApprovalResolved {
            tool_call_id,
            title,
            decision,
        } => {
            eprintln!(
                "[tool {tool_call_id}] approval {decision:?}: {}",
                title.as_deref().unwrap_or("-")
            );
        }
        Event::TurnEnded(end) => eprintln!("\n[turn] {end:?}"),
    }
}

async fn run(args: Args) -> Result<(), String> {
    let db = match &args.db {
        Some(path) => path.clone(),
        None => default_db_path()?,
    };
    let store = Store::open(&db).map_err(|error| error.to_string())?;
    let conversation = open_conversation(&store, &args)?;
    eprintln!("[conversation {}]", conversation.id);

    if args.history {
        for event in store
            .events(conversation.id)
            .map_err(|error| error.to_string())?
        {
            print_event(&event);
        }
        println!();
        return Ok(());
    }

    let config = SessionConfig {
        agent: agent_profile(&conversation)?,
        cwd: conversation.cwd.clone(),
        approvals: args.approvals,
        resume: conversation.provider_session_id.clone(),
    };
    let prompts = prompt_source(args.prompt);

    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut recorder = Recorder::new(store, conversation.id);
    let recording = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            print_event(&event);
            recorder.record(event)?;
        }
        recorder.flush()?;
        Ok::<(), StoreError>(())
    });

    let session = acp::run_session(config, prompts, sender)
        .await
        .map_err(|error| error.to_string());
    let recorded = match recording.await {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    session.and(recorded)
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
            eprintln!("\n[crowbotd] failed: {message}");
            ExitCode::FAILURE
        }
    }
}
