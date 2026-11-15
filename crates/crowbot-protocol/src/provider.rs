use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    Ready,
    SignedOut,
    RuntimeMissing,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub state: ProviderState,
    #[serde(default)]
    pub checking: bool,
    #[serde(default)]
    pub signing_in: bool,
    #[serde(default)]
    pub can_sign_in: bool,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub help_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub daemon_version: String,
    pub protocol_version: u32,
    pub database: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_this_version_does_not_know_is_unknown() {
        let status: ProviderStatus =
            serde_json::from_str(r#"{"id":"claude","name":"Claude","state":"rate_limited"}"#)
                .unwrap();
        assert_eq!(status.state, ProviderState::Unknown);
        assert!(!status.checking);
        assert!(!status.can_sign_in);
        assert_eq!(status.account, None);
    }

    #[test]
    fn a_state_keeps_its_name_on_the_wire() {
        for (state, name) in [
            (ProviderState::Ready, "ready"),
            (ProviderState::SignedOut, "signed_out"),
            (ProviderState::RuntimeMissing, "runtime_missing"),
            (ProviderState::Unknown, "unknown"),
        ] {
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{name}\"")
            );
        }
    }
}
