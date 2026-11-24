//! The guard script of a routine (US-035): a program of the bot's folder,
//! approved with the hash of its content, that runs before each run and
//! decides whether the bot is needed, so that a run with nothing new costs
//! no turn.
//!
//! The script runs in the bot's folder, under the guard that ends it with
//! the daemon (`lifetime.rs`), in a process group of its own that the daemon
//! ends when it runs past `SCRIPT_TIMEOUT`. No output, or a last line
//! `{"wakeAgent": false}`, means the bot is not needed.

use std::path::Path;
use std::process::Stdio;

use crowbot_protocol::RoutineScript;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{SCRIPT_OUTPUT_BYTES, SCRIPT_TIMEOUT};
use crate::digest::sha256_hex;

/// How a guard script ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptEnd {
    /// Its content is not the one a person approved: it did not run.
    Changed,
    /// It found nothing new.
    Silent,
    /// It printed this, which comes before the routine's prompt.
    Wake(String),
    /// It failed, or ran too long, for this reason.
    Failed(String),
}

pub async fn run(script: &RoutineScript, folder: &Path) -> ScriptEnd {
    let path = Path::new(&script.path);
    let content = match tokio::fs::read(path).await {
        Ok(content) => content,
        Err(error) => return ScriptEnd::Failed(format!("The script cannot be read: {error}.")),
    };
    if sha256_hex(&content) != script.hash {
        return ScriptEnd::Changed;
    }
    let (program, args) = crate::lifetime::guarded(path, &[]);
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ScriptEnd::Failed(format!("The script did not start: {error}."));
        }
    };
    let group = child.id();
    let mut stdout = child.stdout.take();
    let finished = async {
        let mut output = Vec::new();
        if let Some(stdout) = stdout.as_mut() {
            let mut limited = stdout.take(u64::try_from(SCRIPT_OUTPUT_BYTES).unwrap_or(u64::MAX));
            limited.read_to_end(&mut output).await.ok();
        }
        // What the script prints past the limit is not read: the pipe is
        // closed, and the script ends as it ends.
        drop(stdout.take());
        (child.wait().await, output)
    };
    let limit = crate::guard::scaled(SCRIPT_TIMEOUT);
    let (status, output) = match tokio::time::timeout(limit, finished).await {
        Ok(ended) => ended,
        Err(_) => {
            end_group(group);
            return ScriptEnd::Failed(format!(
                "The script ran longer than {} seconds, so Crow Bot ended it.",
                limit.as_secs()
            ));
        }
    };
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return ScriptEnd::Failed(format!("The script ended with {status}."));
        }
        Err(error) => return ScriptEnd::Failed(format!("The script failed: {error}.")),
    }
    read_output(&String::from_utf8_lossy(&output))
}

/// What a script's output asks for.
pub fn read_output(output: &str) -> ScriptEnd {
    let output = output.trim();
    if output.is_empty() {
        return ScriptEnd::Silent;
    }
    let last = output.lines().rev().find(|line| !line.trim().is_empty());
    let asleep = last
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .is_some_and(|value| value.get("wakeAgent") == Some(&serde_json::Value::Bool(false)));
    if asleep {
        ScriptEnd::Silent
    } else {
        ScriptEnd::Wake(output.to_owned())
    }
}

/// Ends the script's process group: the guard, the script, and what it
/// started.
#[allow(clippy::needless_pass_by_value)]
fn end_group(group: Option<u32>) {
    #[cfg(unix)]
    if let Some(group) = group.and_then(|id| i32::try_from(id).ok()) {
        // SAFETY: `kill` takes plain integers; a group that is gone is an
        // error it reports, with no other effect.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    // On Windows the child is killed when it is dropped (`kill_on_drop`).
    #[cfg(windows)]
    let _ = group;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_output_or_a_last_line_that_says_so_needs_no_bot() {
        assert_eq!(read_output("  \n"), ScriptEnd::Silent);
        assert_eq!(
            read_output("checked 4 feeds\n{\"wakeAgent\": false}\n"),
            ScriptEnd::Silent
        );
        assert_eq!(
            read_output("2 new mails\n"),
            ScriptEnd::Wake("2 new mails".to_owned())
        );
        assert_eq!(
            read_output("{\"wakeAgent\": true}"),
            ScriptEnd::Wake("{\"wakeAgent\": true}".to_owned())
        );
    }
}
