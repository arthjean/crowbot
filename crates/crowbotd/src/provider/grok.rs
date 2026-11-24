use std::collections::BTreeMap;
use std::path::Path;

use crate::provider::acp::{AgentProfile, CommandFolder};

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

/// What the home must not hold at a start: every layer Grok 1.0.77 reads
/// beside its config, which a bot could write to reach the next session or
/// another bot. That is the folders it trusts (whose own `.grok` hooks would
/// then run), hooks, workflows (scripts), language servers (commands),
/// plugins, skills, agents, personas, rules, memories, a managed config, the
/// settings, the sandbox config, and the status line script. The rest is
/// Grok's own state: the sessions, the logs, and the sign-in (`auth.json`)
/// and server credentials, which the daemon never touches.
const REMOVED: [&str; 18] = [
    "trusted_folders.toml",
    "hooks",
    "workflows",
    "lsp.json",
    "plugins",
    "skills",
    "agents",
    "personas",
    "rules",
    "memory",
    "memory-v2",
    "managed_config.toml",
    "requirements.toml",
    "settings.json",
    "sandbox.toml",
    "statusline.sh",
    "AGENTS.md",
    "GROK.md",
];

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
        removed: REMOVED.iter().map(|path| home.join(path)).collect(),
        // No permission request of this provider names the folder of a
        // command (`docs/harness-providers.md`, A9).
        command_folder: CommandFolder::Unnamed,
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
        assert!(profile.removed.contains(&home.join("trusted_folders.toml")));
        assert!(
            !profile
                .removed
                .iter()
                .any(|path| path.ends_with("auth.json") || path.ends_with("sessions"))
        );
    }
}
