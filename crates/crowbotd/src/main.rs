mod event;
mod provider;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use tokio::sync::mpsc;

use crate::event::{ApprovalPolicy, Event};
use crate::provider::acp::{self, AgentProfile, Turn};
use crate::provider::claude;

const USAGE: &str = "usage: crowbotd [--agent <command>] [--cwd <dir>] [--approve] <prompt>";

fn parse_args(mut raw: impl Iterator<Item = String>) -> Result<Turn, String> {
    let mut agent = None;
    let mut cwd = None;
    let mut prompt = None;
    let mut approvals = ApprovalPolicy::Decline;
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--agent" => {
                let command = raw.next().ok_or("--agent needs a command")?;
                agent = Some(AgentProfile::generic(command));
            }
            "--cwd" => cwd = Some(PathBuf::from(raw.next().ok_or("--cwd needs a directory")?)),
            "--approve" => approvals = ApprovalPolicy::AllowOnce,
            _ if prompt.is_none() => prompt = Some(arg),
            _ => return Err(format!("unexpected argument: {arg}")),
        }
    }
    let prompt = prompt.ok_or(USAGE)?;
    let cwd = match cwd {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    Ok(Turn {
        agent: agent.unwrap_or_else(claude::profile),
        cwd,
        prompt,
        approvals,
    })
}

fn print_event(event: &Event) {
    match event {
        Event::MessageText(text) => {
            print!("{text}");
            std::io::stdout().flush().ok();
        }
        Event::ThoughtText(_) => {}
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
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let turn = match parse_args(std::env::args().skip(1)) {
        Ok(turn) => turn,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    let (sender, mut receiver) = mpsc::unbounded_channel();
    let printer = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            print_event(&event);
        }
    });

    let outcome = acp::run_turn(turn, sender).await;
    printer.await.ok();

    match outcome {
        Ok(end) => {
            eprintln!("\n[turn] {end:?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("\n[turn] failed: {error}");
            ExitCode::FAILURE
        }
    }
}
