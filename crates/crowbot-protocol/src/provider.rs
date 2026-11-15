use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    Ready,
    SignedOut,
    RuntimeMissing,
    /// The daemon installs the runtime itself, and has not done so yet.
    NotInstalled,
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
    /// The page where the person finishes the running sign-in, when the
    /// provider names one: the client that started the sign-in opens it.
    #[serde(default)]
    pub sign_in_url: Option<String>,
    /// The sign-ins the provider offers, in the order it lists them.
    #[serde(default)]
    pub sign_in_methods: Vec<SignInMethod>,
    /// The executable the user set in place of the installed runtime.
    #[serde(default)]
    pub executable: Option<String>,
    /// The runtime the daemon installs for this provider, as a name and a
    /// version. `None` when it installs none on this computer.
    #[serde(default)]
    pub runtime: Option<String>,
    /// Set while the runtime downloads.
    #[serde(default)]
    pub install: Option<RuntimeInstall>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInMethod {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// How far the download of a runtime is, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInstall {
    pub received: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelsState {
    /// The daemon is asking the provider.
    Loading,
    Listed,
    /// The provider reports no list, or could not be asked.
    #[default]
    #[serde(other)]
    Unavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The reasoning levels of this model, when the provider has levels that
    /// depend on the model. Empty for a level, and for a model whose levels
    /// are not known: the levels of the provider then apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<ModelChoice>,
    /// The level a session of this model starts on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

/// The models and the reasoning levels a provider offers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModels {
    pub provider: String,
    #[serde(default)]
    pub state: ModelsState,
    #[serde(default)]
    pub models: Vec<ModelChoice>,
    /// The model a session runs on when the bot names none.
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub efforts: Vec<ModelChoice>,
    #[serde(default)]
    pub default_effort: Option<String>,
    /// Why the list is not available.
    #[serde(default)]
    pub detail: Option<String>,
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
        assert!(status.sign_in_methods.is_empty());
        assert_eq!(status.install, None);
    }

    #[test]
    fn a_model_list_in_a_state_this_version_does_not_know_is_unavailable() {
        let models: ProviderModels =
            serde_json::from_str(r#"{"provider":"codex","state":"stale"}"#).unwrap();
        assert_eq!(models.state, ModelsState::Unavailable);
        assert!(models.models.is_empty());
        let listed = ProviderModels {
            provider: "codex".to_owned(),
            state: ModelsState::Listed,
            models: vec![ModelChoice {
                id: "gpt".to_owned(),
                name: "GPT".to_owned(),
                ..ModelChoice::default()
            }],
            default_model: Some("gpt".to_owned()),
            ..ProviderModels::default()
        };
        let wire = serde_json::to_string(&listed).unwrap();
        assert_eq!(
            serde_json::from_str::<ProviderModels>(&wire).unwrap(),
            listed
        );
    }

    #[test]
    fn a_state_keeps_its_name_on_the_wire() {
        for (state, name) in [
            (ProviderState::Ready, "ready"),
            (ProviderState::SignedOut, "signed_out"),
            (ProviderState::RuntimeMissing, "runtime_missing"),
            (ProviderState::NotInstalled, "not_installed"),
            (ProviderState::Unknown, "unknown"),
        ] {
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{name}\"")
            );
        }
    }
}
