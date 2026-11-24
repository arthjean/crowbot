use serde_json::{Map, Value, json};

use crate::provider::acp::{AgentProfile, CommandFolder};

pub const PROVIDER: &str = "claude";

/// The mode in which the agent asks before each gated tool call.
const SESSION_MODE: &str = "default";

pub fn profile() -> AgentProfile {
    AgentProfile {
        name: "Claude",
        session_meta: Some(session_meta()),
        session_mode: Some(SESSION_MODE),
        // No permission request of this provider names the folder of a
        // command (`docs/harness-providers.md`, A9).
        command_folder: CommandFolder::Unnamed,
        ..AgentProfile::generic()
    }
}

/// The session reads none of the user's own settings: no allowed command and
/// no bypass mode of the user reaches a bot.
fn session_meta() -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert(
        "claudeCode".to_owned(),
        json!({
            "options": {
                "settingSources": [],
                "allowDangerouslySkipPermissions": false
            }
        }),
    );
    meta
}
