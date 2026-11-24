use std::collections::BTreeMap;
use std::path::Path;

use crate::provider::acp::{AgentProfile, CommandFolder};

pub const PROVIDER: &str = "codex";

/// The mode in which Codex asks before each change: the sandbox lets it read,
/// and every write goes through an approval request.
const SESSION_MODE: &str = "read-only";

/// What Codex reads from its home. It trusts no folder: the `.codex` config
/// of a bot's folder applies only to a folder this file marks as trusted.
/// The daemon writes the file again before each start, so a bot that edits
/// it changes nothing for the next session or for another bot.
const CONFIG: &str = "# Written by Crow Bot before each start. Changes here do not last.\n";

/// What the home must not hold at a start: every layer Codex reads beside its
/// config, which a bot could write to reach the next session or another bot.
/// That is hooks, rules that let commands run without asking, a managed
/// config, plugins (which bring hooks and servers), skills, prompts, agents,
/// instructions, memories, and a `.env` file, whose variables Codex loads
/// into its own environment. The rest is Codex's own state, which a resumed
/// session needs: the sessions, the databases, the sandbox, and the sign-in
/// (`auth.json`), which the daemon never touches. A list of what to keep would
/// remove that state too.
const REMOVED: [&str; 15] = [
    ".env",
    "hooks.json",
    "hooks",
    "rules",
    "managed_config.toml",
    "plugins",
    "skills",
    "prompts",
    "agents",
    "AGENTS.md",
    "AGENTS.override.md",
    "instructions.md",
    "memories",
    "memories_v2",
    "vendor_imports",
];

/// The provider of a ChatGPT plan: its agent is Codex. Codex runs with a
/// home of its own, which the daemon owns: the allowed
/// commands, the hooks, the instructions, and the servers of the user's own
/// Codex do not reach a bot, and what a bot writes there does not last. The
/// user signs in to that home once, through the sign-in the agent offers.
pub fn profile(home: &Path) -> AgentProfile {
    let home_text = home.to_string_lossy().into_owned();
    AgentProfile {
        name: "ChatGPT",
        env: BTreeMap::from([
            ("CODEX_HOME".to_owned(), home_text),
            ("INITIAL_AGENT_MODE".to_owned(), SESSION_MODE.to_owned()),
        ]),
        session_mode: Some(SESSION_MODE),
        hidden_sign_ins: &["api-key"],
        folders: vec![home.to_owned()],
        files: vec![(home.join("config.toml"), CONFIG.to_owned())],
        removed: REMOVED.iter().map(|path| home.join(path)).collect(),
        // No permission request of this provider names the folder of a
        // command (`docs/harness-providers.md`, A9).
        command_folder: CommandFolder::Unnamed,
        ..AgentProfile::generic()
    }
}

/// On Linux Codex runs each shell command under bubblewrap (`bwrap`), which
/// needs user namespaces unless it is installed setuid. This starts the
/// `bwrap` Codex picks, the first one on `PATH` and else the one its runtime
/// brings, once, with the namespaces Codex asks for. A computer that disables
/// user namespaces then fails the turn with its cause, instead of failing each
/// command the bot runs. Any other failure is left to Codex to report.
#[cfg(target_os = "linux")]
pub async fn check_sandbox(launch: &crate::provider::acp::Launch) -> Result<(), String> {
    let Some(bwrap) = sandbox_program(launch) else {
        return Ok(());
    };
    let checked = tokio::process::Command::new(&bwrap)
        .args(["--unshare-all", "--die-with-parent", "--ro-bind", "/", "/"])
        .args(["--", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output();
    let Ok(Ok(output)) = tokio::time::timeout(std::time::Duration::from_secs(10), checked).await
    else {
        return Ok(());
    };
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.lines().next().unwrap_or_default().trim();
    if output.status.success() || !said.contains("namespace") {
        return Ok(());
    }
    Err(format!(
        "ChatGPT's sandbox needs user namespaces, which this computer disables ({}: {said}).",
        bwrap.display()
    ))
}

/// The `bwrap` Codex runs: the first one on the agent's `PATH`, else the one
/// beside the Codex program of its runtime.
#[cfg(target_os = "linux")]
fn sandbox_program(launch: &crate::provider::acp::Launch) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let runs = |path: &Path| {
        std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    };
    let path = launch
        .env
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let on_path = std::env::split_paths(&path)
        .map(|folder| folder.join("bwrap"))
        .find(|candidate| runs(candidate));
    let bundled = || {
        let codex = Path::new(launch.env.get("CODEX_PATH")?);
        let bwrap = codex
            .parent()?
            .parent()?
            .join("codex-resources")
            .join("bwrap");
        runs(&bwrap).then_some(bwrap)
    };
    on_path.or_else(bundled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_runs_in_a_home_of_the_daemon_and_in_the_mode_that_asks() {
        let home = Path::new("/data/providers/codex/home");
        let profile = profile(home);
        assert_eq!(
            profile.env.get("CODEX_HOME").map(String::as_str),
            Some(home.to_string_lossy().as_ref())
        );
        assert_eq!(profile.session_mode, Some("read-only"));
        assert_eq!(profile.folders, [home]);
        assert_eq!(
            profile.files,
            [(home.join("config.toml"), CONFIG.to_owned())]
        );
        assert!(!CONFIG.contains("trust"));
        assert!(profile.removed.contains(&home.join("rules")));
        assert!(
            !profile
                .removed
                .iter()
                .any(|path| path.ends_with("auth.json"))
        );
        assert!(profile.hidden_sign_ins.contains(&"api-key"));
    }
}
