use std::collections::BTreeMap;
use std::path::Path;

use crate::provider::acp::AgentProfile;

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
/// instructions, and memories. The rest is Codex's own state, which a resumed
/// session needs: the sessions, the databases, the sandbox, and the sign-in
/// (`auth.json`), which the daemon never touches. A list of what to keep would
/// remove that state too.
const REMOVED: [&str; 13] = [
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
        ..AgentProfile::generic()
    }
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
