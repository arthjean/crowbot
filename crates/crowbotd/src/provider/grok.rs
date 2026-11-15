use std::collections::BTreeMap;
use std::path::Path;

use crate::provider::acp::AgentProfile;

pub const PROVIDER: &str = "grok";

/// The sign-in Grok wants named before each session.
const SESSION_AUTH: &str = "grok.com";

/// The request that names the page where a Grok sign-in finishes.
const SIGN_IN_PAGE: &str = "_x.ai/auth/get_url";

/// What Grok reads from its home. The `ask` rules make each tool that changes
/// something ask first, and no approval is remembered from one call to the
/// next. The daemon writes the file again before each start.
const CONFIG: &str = r#"[permission]
ask = ["Edit", "Write", "Bash", "WebFetch", "MCPTool"]

[ui]
remember_tool_approvals = false
"#;

const OTHER_TOOLS: [&str; 3] = ["CLAUDE", "CURSOR", "CODEX"];
const OTHER_TOOL_PARTS: [&str; 6] = ["AGENTS", "HOOKS", "MCPS", "RULES", "SKILLS", "SESSIONS"];

/// Grok runs with a home of its own, which the daemon owns, and reads nothing
/// from the other coding tools of the user: their allowed commands, hooks,
/// instructions, and servers do not reach a bot. The user signs in to that
/// home once, through the sign-in the agent offers.
pub fn profile(home: &Path) -> AgentProfile {
    let mut env = BTreeMap::from([
        ("GROK_HOME".to_owned(), home.to_string_lossy().into_owned()),
        ("GROK_REMEMBER_TOOL_APPROVALS".to_owned(), "0".to_owned()),
    ]);
    for tool in OTHER_TOOLS {
        for part in OTHER_TOOL_PARTS {
            env.insert(format!("GROK_{tool}_{part}_ENABLED"), "0".to_owned());
        }
    }
    AgentProfile {
        name: "Grok",
        args: ["--permission-mode", "default", "agent", "stdio"]
            .map(str::to_owned)
            .to_vec(),
        env,
        // Grok 1.0.77 reports no session mode over ACP: the argument sets it,
        // and a session that reports leaving it is stopped.
        started_mode: Some("default"),
        session_auth: Some(SESSION_AUTH),
        // Grok opens no browser for a sign-in over ACP: it names the page,
        // which a client opens.
        sign_in_page: Some(SIGN_IN_PAGE),
        folders: vec![home.to_owned()],
        files: vec![(home.join("config.toml"), CONFIG.to_owned())],
        ..AgentProfile::generic()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_reads_the_rules_of_the_daemon_and_none_of_another_tool() {
        let home = Path::new("/data/providers/grok/home");
        let profile = profile(home);
        assert_eq!(
            profile.args,
            ["--permission-mode", "default", "agent", "stdio"]
        );
        assert_eq!(
            profile.env.get("GROK_HOME").map(String::as_str),
            Some(home.to_string_lossy().as_ref())
        );
        assert_eq!(
            profile
                .env
                .get("GROK_CLAUDE_RULES_ENABLED")
                .map(String::as_str),
            Some("0")
        );
        assert_eq!(profile.env.len(), 20);
        let (path, config) = &profile.files[0];
        assert_eq!(path, &home.join("config.toml"));
        assert!(config.contains(r#"ask = ["Edit", "Write", "Bash", "WebFetch", "MCPTool"]"#));
        assert_eq!(profile.session_auth, Some("grok.com"));
    }
}
