use std::collections::BTreeMap;
use std::path::Path;

use crate::provider::acp::AgentProfile;

pub const PROVIDER: &str = "codex";

/// The mode in which Codex asks before each change: the sandbox lets it read,
/// and every write goes through an approval request.
const SESSION_MODE: &str = "read-only";

/// The provider of a ChatGPT plan: its agent is Codex. Codex runs with a
/// home of its own, which the daemon owns: the allowed
/// commands, the hooks, the instructions, and the servers of the user's own
/// Codex do not reach a bot. The user signs in to that home once, through
/// the sign-in the agent offers.
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
        assert!(profile.hidden_sign_ins.contains(&"api-key"));
    }
}
