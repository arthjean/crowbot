use serde_json::{Map, Value, json};

use crate::provider::acp::AgentProfile;

pub const PROVIDER: &str = "claude";

const COMMAND: &str = "bunx @agentclientprotocol/claude-agent-acp@0.107.0";
const SESSION_MODE: &str = "default";
const RUNTIME_URL: &str = "https://bun.sh";

pub fn profile() -> AgentProfile {
    AgentProfile {
        name: "Claude",
        command: COMMAND.to_owned(),
        runtime_url: Some(RUNTIME_URL),
        session_meta: Some(session_meta()),
        session_mode: Some(SESSION_MODE.to_owned()),
    }
}

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
