use serde_json::{Map, Value, json};

use crate::provider::acp::AgentProfile;

const COMMAND: &str = "bunx @agentclientprotocol/claude-agent-acp@0.107.0";
const SESSION_MODE: &str = "default";

pub fn profile() -> AgentProfile {
    AgentProfile {
        command: COMMAND.to_owned(),
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
