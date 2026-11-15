//! Bots on Claude, Codex, and Grok: what makes each provider ask the daemon
//! first, its sign-ins, the models it offers, and the move of a bot from one
//! provider to another. Each provider is the scripted agent, started by the
//! daemon's own profile of that provider.

use std::path::{Path, PathBuf};

use crowbot_protocol::{
    ApprovalDecision, BotDraft, BotPatch, BotSummary, ClientMessage, Event, FailureCause,
    ModelsState, ProviderModels, ProviderState, ProviderStatus, ServerMessage, TurnEnd,
};
use serde_json::json;

use crate::support::{self, Client, Daemon, user_texts};

const CLAUDE: &str = "claude";
const CODEX: &str = "codex";
const GROK: &str = "grok";

/// A daemon that installs no runtime: each provider runs the executable a
/// test sets for it.
fn daemon() -> Daemon {
    Daemon::start_with_providers(&json!({}), &[])
}

fn home(daemon: &Daemon, provider: &str) -> PathBuf {
    daemon
        .data_dir()
        .join("providers")
        .join(provider)
        .join("home")
}

async fn until_provider(
    client: &mut Client,
    provider: &str,
    matches: impl Fn(&ProviderStatus) -> bool,
) -> ProviderStatus {
    client
        .until(|message| match message {
            ServerMessage::ProviderChanged(status) if status.id == provider && matches(&status) => {
                Some(status)
            }
            ServerMessage::Error { message } => panic!("the daemon refused: {message}"),
            ServerMessage::InvalidField { field, message } => panic!("{field}: {message}"),
            _ => None,
        })
        .await
}

fn settled(status: &ProviderStatus) -> bool {
    !status.checking && !status.signing_in
}

/// Makes the scripted agent the executable of the provider, and returns the
/// state the provider then reports.
async fn use_scripted(client: &mut Client, provider: &str) -> ProviderStatus {
    client
        .send(ClientMessage::SetProviderExecutable {
            provider: provider.to_owned(),
            path: Some(support::fake_agent().to_string_lossy().into_owned()),
        })
        .await;
    until_provider(client, provider, |status| {
        status.executable.is_some() && settled(status)
    })
    .await
}

async fn sign_in(client: &mut Client, provider: &str, method: Option<&str>) -> ProviderStatus {
    client
        .send(ClientMessage::SignIn {
            provider: provider.to_owned(),
            method: method.map(str::to_owned),
        })
        .await;
    until_provider(client, provider, |status| {
        settled(status) && status.state == ProviderState::Ready
    })
    .await
}

/// The provider runs the scripted agent, signed in the way it signs in.
async fn ready(daemon: &Daemon, client: &mut Client, provider: &str) {
    match provider {
        CODEX => {
            // The home already holds a sign-in, as after an earlier one.
            let home = home(daemon, CODEX);
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("signed-in"), "earlier").unwrap();
            let status = use_scripted(client, CODEX).await;
            assert_eq!(status.state, ProviderState::Ready, "{status:?}");
        }
        GROK => {
            use_scripted(client, GROK).await;
            sign_in(client, GROK, None).await;
        }
        _ => {
            use_scripted(client, provider).await;
        }
    }
}

async fn bot_on(client: &mut Client, provider: &str) -> BotSummary {
    client
        .create_bot(BotDraft {
            name: format!("Bot on {provider}"),
            provider: Some(provider.to_owned()),
            ..BotDraft::default()
        })
        .await
}

/// Creates a bot on the provider and opens its conversation.
async fn conversation_on(client: &mut Client, provider: &str) -> (BotSummary, i64) {
    let bot = bot_on(client, provider).await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;
    (bot, conversation)
}

async fn move_to(client: &mut Client, bot_id: i64, provider: &str) {
    client
        .update_bot(
            bot_id,
            BotPatch {
                provider: Some(provider.to_owned()),
                ..BotPatch::default()
            },
        )
        .await;
    client
        .until_bot(|summary| summary.bot.id == bot_id && summary.bot.provider == provider)
        .await;
}

async fn answer_approval(client: &mut Client, conversation_id: i64, allow: bool) {
    let Event::ApprovalRequested { id, .. } = client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
    else {
        unreachable!()
    };
    client
        .send(ClientMessage::ResolveApproval {
            conversation_id,
            approval_id: id,
            decision: if allow {
                ApprovalDecision::Allowed
            } else {
                ApprovalDecision::Declined
            },
        })
        .await;
}

async fn failed_turn(client: &mut Client) -> (String, Option<FailureCause>, Option<String>) {
    match client.until_turn_ends().await {
        Event::TurnFailed {
            message,
            cause,
            provider,
        } => (message, cause, provider),
        other => panic!("the turn did not fail: {other:?}"),
    }
}

async fn models(client: &mut Client, provider: &str) -> ProviderModels {
    client
        .send(ClientMessage::ListModels {
            provider: provider.to_owned(),
        })
        .await;
    client
        .until(|message| match message {
            ServerMessage::ProviderModels(models)
                if models.provider == provider && models.state != ModelsState::Loading =>
            {
                Some(models)
            }
            ServerMessage::Error { message } => panic!("the daemon refused: {message}"),
            _ => None,
        })
        .await
}

fn ids(choices: &[crowbot_protocol::ModelChoice]) -> Vec<&str> {
    choices.iter().map(|choice| choice.id.as_str()).collect()
}

/// The value of `name=` in a report of the scripted agent.
fn field<'a>(report: &'a str, name: &str) -> &'a str {
    let start = report
        .find(&format!("{name}="))
        .unwrap_or_else(|| panic!("no {name} in {report}"))
        + name.len()
        + 1;
    report[start..].split(' ').next().unwrap_or_default()
}

// US-030: provider profiles for Codex and Grok.

#[tokio::test]
async fn a_bot_on_codex_or_grok_streams_its_answer_from_that_provider() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    for (provider, model) in [(CODEX, "gpt-fast"), (GROK, "grok-fast")] {
        ready(&daemon, &mut client, provider).await;
        let (_, conversation) = conversation_on(&mut client, provider).await;
        client.say(conversation, "stream 4 5").await;
        client.until_accepted().await;
        let mut chunks = Vec::new();
        loop {
            let ServerMessage::ConversationEvent { event, .. } = client.next().await else {
                continue;
            };
            match event {
                Event::MessageText(text) => chunks.push(text),
                Event::TurnEnded(end) => {
                    assert_eq!(end, TurnEnd::Completed);
                    break;
                }
                Event::TurnFailed { message, .. } => panic!("{provider}: {message}"),
                _ => {}
            }
        }
        assert_eq!(chunks.concat(), "w0 w1 w2 w3 ", "{provider}");
        assert!(chunks.len() > 1, "{provider} did not stream: {chunks:?}");
        // The answer came from the agent of that provider.
        let settings = client.ask(conversation, "settings").await;
        assert_eq!(field(&settings, "model"), model, "{settings}");
    }
}

#[tokio::test]
async fn a_declined_write_leaves_no_file_on_each_provider() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    let out = daemon.folder("out");
    for provider in [CLAUDE, CODEX, GROK] {
        ready(&daemon, &mut client, provider).await;
        let (_, conversation) = conversation_on(&mut client, provider).await;
        let target = out.join(format!("{provider}.txt"));
        let write = format!("write {}", target.display());

        client.say(conversation, &write).await;
        answer_approval(&mut client, conversation, false).await;
        client.until_reply("write=declined").await;
        client.until_turn_ends().await;
        assert!(!target.exists(), "{provider} wrote a declined file");

        client.say(conversation, &write).await;
        answer_approval(&mut client, conversation, true).await;
        client.until_reply("write=written").await;
        client.until_turn_ends().await;
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "written");
    }
}

#[tokio::test]
async fn a_provider_that_cannot_ask_first_fails_the_turn_by_name_and_runs_no_tool() {
    let daemon = Daemon::start_with_providers(&json!({}), &[("FAKE_AGENT_NO_ASK_MODE", "1")]);
    let mut client = daemon.client().await;
    let out = daemon.folder("out");
    for (provider, name, mode) in [
        (CODEX, "ChatGPT", "`read-only`"),
        (CLAUDE, "Claude", "`default`"),
    ] {
        ready(&daemon, &mut client, provider).await;
        let (bot, conversation) = conversation_on(&mut client, provider).await;
        let target = out.join(format!("{provider}.txt"));
        client
            .say(conversation, &format!("write {}", target.display()))
            .await;
        let (message, _, failed) = failed_turn(&mut client).await;
        assert!(message.contains(name), "{message}");
        assert!(message.contains(mode), "{message}");
        assert!(message.contains("No tool was run"), "{message}");
        assert_eq!(failed.as_deref(), Some(provider));
        assert!(!target.exists(), "{provider} ran a tool");
        // The bot is still there, on the same provider.
        assert_eq!(client.bot(bot.bot.id).await.bot.provider, provider);
    }
}

#[tokio::test]
async fn an_agent_that_leaves_the_mode_that_asks_first_is_stopped_before_its_tool_runs() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    let out = daemon.folder("out");
    for (provider, name, mode) in [
        (CLAUDE, "Claude", "`default`"),
        (CODEX, "ChatGPT", "`read-only`"),
        (GROK, "Grok", "`default`"),
    ] {
        ready(&daemon, &mut client, provider).await;
        let (bot, conversation) = conversation_on(&mut client, provider).await;
        let target = out.join(format!("left-{provider}.txt"));
        client
            .say(
                conversation,
                &format!("leave bypassPermissions {}", target.display()),
            )
            .await;
        let (message, _, failed) = failed_turn(&mut client).await;
        assert!(message.contains(name), "{message}");
        assert!(message.contains(mode), "{message}");
        assert!(message.contains("`bypassPermissions`"), "{message}");
        assert_eq!(failed.as_deref(), Some(provider));
        // The agent would have written after two seconds.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert!(!target.exists(), "{provider} ran a tool out of its mode");
        // The next turn starts a session in the mode that asks first. Grok
        // reports no session mode: its arguments set it.
        if provider != GROK {
            let settings = client.ask(conversation, "settings").await;
            assert!(
                settings.contains(&format!("mode={}", mode.trim_matches('`'))),
                "{settings}"
            );
        }
        assert_eq!(client.bot(bot.bot.id).await.bot.provider, provider);
    }
}

#[tokio::test]
async fn the_users_own_settings_for_a_provider_do_not_reach_a_bot() {
    let daemon = daemon();
    let mut client = daemon.client().await;

    ready(&daemon, &mut client, CODEX).await;
    let (_, conversation) = conversation_on(&mut client, CODEX).await;
    let started = client.ask(conversation, "started").await;
    let codex_home = home(&daemon, CODEX);
    assert_eq!(Path::new(field(&started, "CODEX_HOME")), codex_home);
    assert!(
        started.contains("INITIAL_AGENT_MODE=read-only"),
        "{started}"
    );
    let settings = client.ask(conversation, "settings").await;
    assert_eq!(field(&settings, "mode"), "read-only", "{settings}");
    // The Codex home is shared by every bot on Codex. What a bot or a person
    // wrote there does not last: a config that trusts a folder, which would
    // let that folder's own `.codex/config.toml` apply, hooks, rules that
    // allow commands, and instructions.
    let planted = [
        ".env",
        "instructions.md",
        "hooks.json",
        "AGENTS.md",
        "AGENTS.override.md",
        "managed_config.toml",
        "rules/allow.rules",
        "skills/any/SKILL.md",
        "plugins/any/hooks/hooks.json",
    ];
    std::fs::write(
        codex_home.join("config.toml"),
        "notify = [\"calc\"]\n[projects.\"C:/bots/any\"]\ntrust_level = \"trusted\"\n",
    )
    .unwrap();
    for path in planted {
        let path = codex_home.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "planted").unwrap();
    }
    std::fs::create_dir_all(codex_home.join("sessions")).unwrap();
    std::fs::write(codex_home.join("auth.json"), "kept").unwrap();
    let (_, conversation) = conversation_on(&mut client, CODEX).await;
    client.ask(conversation, "say again").await;
    let config = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
    assert!(!config.contains("trusted"), "{config}");
    assert!(!config.contains("notify"), "{config}");
    for path in planted {
        assert!(!codex_home.join(path).exists(), "{path} is still there");
    }
    // Codex's own state stays, so a session can be resumed.
    assert!(codex_home.join("sessions").is_dir());
    // The sign-in stays: the daemon never touches it.
    assert_eq!(
        std::fs::read_to_string(codex_home.join("auth.json")).unwrap(),
        "kept"
    );

    ready(&daemon, &mut client, GROK).await;
    let (_, conversation) = conversation_on(&mut client, GROK).await;
    let started = client.ask(conversation, "started").await;
    let grok_home = home(&daemon, GROK);
    assert!(
        started.starts_with("args=[--permission-mode default agent stdio]"),
        "{started}"
    );
    assert_eq!(Path::new(field(&started, "GROK_HOME")), grok_home);
    for part in ["AGENTS", "HOOKS", "MCPS", "RULES", "SKILLS", "SESSIONS"] {
        for tool in ["CLAUDE", "CURSOR", "CODEX"] {
            let off = format!("GROK_{tool}_{part}_ENABLED=0");
            assert!(started.contains(&off), "{off} is missing in {started}");
        }
    }
    let rules = std::fs::read_to_string(grok_home.join("config.toml")).unwrap();
    assert!(rules.contains(r#"ask = ["Edit", "Write", "Bash", "WebFetch", "MCPTool"]"#));
    // Rules a bot or a person wrote there do not last: the daemon writes its
    // own before each start.
    std::fs::write(
        grok_home.join("config.toml"),
        "[permission]\nallow = [\"Write\"]\n",
    )
    .unwrap();
    // Nor do the other layers Grok reads from its home.
    let planted = [
        "trusted_folders.toml",
        "hooks/session-start.json",
        "workflows/any.rhai",
        "lsp.json",
        "skills/any/SKILL.md",
        "memory-v2/global/MEMORY.md",
    ];
    for path in planted {
        let path = grok_home.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "planted").unwrap();
    }
    std::fs::create_dir_all(grok_home.join("sessions")).unwrap();
    let (_, conversation) = conversation_on(&mut client, GROK).await;
    client.ask(conversation, "say again").await;
    let rules = std::fs::read_to_string(grok_home.join("config.toml")).unwrap();
    assert!(rules.contains("ask = "), "{rules}");
    assert!(!rules.contains("allow"), "{rules}");
    for path in planted {
        assert!(!grok_home.join(path).exists(), "{path} is still there");
    }
    assert!(grok_home.join("sessions").is_dir());

    ready(&daemon, &mut client, CLAUDE).await;
    let (_, conversation) = conversation_on(&mut client, CLAUDE).await;
    let started = client.ask(conversation, "started").await;
    let meta: serde_json::Value =
        serde_json::from_str(started.split_once("meta=").unwrap().1).unwrap();
    assert_eq!(meta["claudeCode"]["options"]["settingSources"], json!([]));
    assert_eq!(
        meta["claudeCode"]["options"]["allowDangerouslySkipPermissions"],
        json!(false)
    );
    let settings = client.ask(conversation, "settings").await;
    assert_eq!(field(&settings, "mode"), "default", "{settings}");
}

#[tokio::test]
async fn a_signed_out_provider_fails_the_turn_by_name_and_the_bot_works_after_the_sign_in() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    for provider in [CODEX, GROK] {
        let status = use_scripted(&mut client, provider).await;
        assert_eq!(status.state, ProviderState::SignedOut, "{status:?}");
        assert!(status.can_sign_in);
        let (bot, conversation) = conversation_on(&mut client, provider).await;

        client.say(conversation, "say hello").await;
        let (_, cause, failed) = failed_turn(&mut client).await;
        assert_eq!(cause, Some(FailureCause::SignInRequired), "{provider}");
        assert_eq!(failed.as_deref(), Some(provider));
        // The failed turn started no sign-in of its own.
        assert!(!home(&daemon, provider).join("signed-in").exists());
        assert_eq!(client.bot(bot.bot.id).await.bot.provider, provider);

        sign_in(&mut client, provider, None).await;
        assert_eq!(client.ask(conversation, "say hello").await, "hello");
    }
}

#[tokio::test]
async fn the_sign_ins_of_a_provider_are_listed_by_name_and_the_one_picked_runs() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    let status = use_scripted(&mut client, CODEX).await;
    assert_eq!(status.state, ProviderState::SignedOut);
    // The method that takes a key is not offered: the daemon never holds one.
    let listed: Vec<(&str, &str)> = status
        .sign_in_methods
        .iter()
        .map(|method| (method.id.as_str(), method.name.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("chat-gpt", "Sign in with ChatGPT"),
            ("device-code", "Sign in with a device code")
        ]
    );
    assert_eq!(
        status.sign_in_methods[1].description.as_deref(),
        Some("Sign in with a device code in the browser")
    );

    client
        .send(ClientMessage::SignIn {
            provider: CODEX.to_owned(),
            method: Some("api-key".to_owned()),
        })
        .await;
    let refused = client.until_error().await;
    assert!(refused.contains("api-key"), "{refused}");

    let signed_in = sign_in(&mut client, CODEX, Some("device-code")).await;
    assert_eq!(signed_in.account.as_deref(), Some("Fake Plus"));
    let ran = std::fs::read_to_string(home(&daemon, CODEX).join("signed-in")).unwrap();
    assert_eq!(ran, "device-code");
}

#[tokio::test]
async fn a_grok_sign_in_names_its_page_to_the_clients_until_it_ends() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    use_scripted(&mut client, GROK).await;
    client
        .send(ClientMessage::SignIn {
            provider: GROK.to_owned(),
            method: None,
        })
        .await;
    // Grok opens no browser over ACP: the daemon asks it for the page, and
    // the clients get it while the sign-in runs.
    let named = until_provider(&mut client, GROK, |status| status.sign_in_url.is_some()).await;
    assert!(named.signing_in, "{named:?}");
    assert_eq!(
        named.sign_in_url.as_deref(),
        Some("https://accounts.example/oauth2/device?user_code=FAKE-1234")
    );
    let ready = until_provider(&mut client, GROK, |status| {
        settled(status) && status.state == ProviderState::Ready
    })
    .await;
    assert_eq!(ready.sign_in_url, None);
}

#[tokio::test]
async fn a_sign_in_the_agent_runs_can_be_cancelled_and_can_fail() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    use_scripted(&mut client, CODEX).await;
    let home = home(&daemon, CODEX);

    std::fs::write(home.join("sign-in-mode"), "wait").unwrap();
    client
        .send(ClientMessage::SignIn {
            provider: CODEX.to_owned(),
            method: None,
        })
        .await;
    until_provider(&mut client, CODEX, |status| status.signing_in).await;
    client
        .send(ClientMessage::CancelSignIn {
            provider: CODEX.to_owned(),
        })
        .await;
    let cancelled = until_provider(&mut client, CODEX, settled).await;
    assert_eq!(cancelled.state, ProviderState::SignedOut);
    assert_eq!(cancelled.detail, None);

    std::fs::write(home.join("sign-in-mode"), "fail").unwrap();
    client
        .send(ClientMessage::SignIn {
            provider: CODEX.to_owned(),
            method: None,
        })
        .await;
    until_provider(&mut client, CODEX, |status| status.signing_in).await;
    let failed = until_provider(&mut client, CODEX, settled).await;
    assert_eq!(failed.state, ProviderState::SignedOut);
    assert_eq!(
        failed.detail.as_deref(),
        Some("The sign-in did not finish.")
    );
    assert!(!home.join("signed-in").exists());
}

#[test]
fn no_acp_type_leaves_the_provider_adapter() {
    fn sources(folder: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let adapter = root.join("provider");
    let mut files = Vec::new();
    sources(&root, &mut files);
    assert!(files.iter().any(|file| file.starts_with(&adapter)));
    for file in files.iter().filter(|file| !file.starts_with(&adapter)) {
        let source = std::fs::read_to_string(file).unwrap();
        assert!(
            !source.contains("agent_client_protocol"),
            "{} uses an ACP type",
            file.display()
        );
    }
}

// US-031: models and reasoning effort.

#[tokio::test]
async fn a_provider_lists_its_models_and_its_reasoning_levels() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CODEX).await;
    ready(&daemon, &mut client, CLAUDE).await;

    // No session ran yet: the daemon asks the provider for one.
    let codex = models(&mut client, CODEX).await;
    assert_eq!(codex.state, ModelsState::Listed);
    assert_eq!(ids(&codex.models), ["gpt-fast", "gpt-smart"]);
    assert_eq!(codex.default_model.as_deref(), Some("gpt-fast"));
    assert_eq!(ids(&codex.efforts), ["medium", "low", "high"]);
    assert_eq!(codex.default_effort.as_deref(), Some("medium"));
    // Each model carries its own levels, read by moving the session to it.
    assert_eq!(ids(&codex.models[0].efforts), ["medium", "low", "high"]);
    assert_eq!(
        ids(&codex.models[1].efforts),
        ["medium", "low", "high", "xhigh"]
    );
    assert_eq!(codex.models[1].default_effort.as_deref(), Some("medium"));

    let claude = models(&mut client, CLAUDE).await;
    assert_eq!(ids(&claude.models), ["fast", "smart"]);
    assert_eq!(claude.default_model.as_deref(), Some("fast"));
    assert!(claude.efforts.is_empty());

    // What a session reported is answered at once.
    client
        .send(ClientMessage::ListModels {
            provider: CODEX.to_owned(),
        })
        .await;
    let again = client
        .until(|message| match message {
            ServerMessage::ProviderModels(models) if models.provider == CODEX => Some(models),
            _ => None,
        })
        .await;
    assert_eq!(again, codex);
}

#[tokio::test]
async fn the_models_come_from_an_open_session_and_keep_the_defaults_of_the_provider() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CODEX).await;
    let bot = client
        .create_bot(BotDraft {
            name: "Atlas".to_owned(),
            provider: Some(CODEX.to_owned()),
            model: Some("gpt-smart".to_owned()),
            effort: Some("high".to_owned()),
            ..BotDraft::default()
        })
        .await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;

    client.say(conversation, "settings").await;
    // The session tells what the provider offers before the turn answers.
    let offered = client
        .until(|message| match message {
            ServerMessage::ProviderModels(models) if models.provider == CODEX => Some(models),
            _ => None,
        })
        .await;
    assert_eq!(offered.state, ModelsState::Listed);
    assert_eq!(offered.default_model.as_deref(), Some("gpt-fast"));
    assert_eq!(offered.default_effort.as_deref(), Some("medium"));
    client.until_reply("model=gpt-smart effort=high").await;
    client.until_turn_ends().await;

    // A reasoning level changed between two turns applies at the next one.
    client
        .update_bot(
            bot.bot.id,
            BotPatch {
                effort: Some(Some("low".to_owned())),
                ..BotPatch::default()
            },
        )
        .await;
    client
        .until_bot(|summary| summary.bot.effort.as_deref() == Some("low"))
        .await;
    let settings = client.ask(conversation, "settings").await;
    assert_eq!(field(&settings, "effort"), "low", "{settings}");
    assert_eq!(field(&settings, "model"), "gpt-smart", "{settings}");
}

#[tokio::test]
async fn a_provider_with_no_model_list_says_so() {
    let daemon = Daemon::start_with_providers(&json!({}), &[("FAKE_AGENT_NO_MODELS", "1")]);
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CODEX).await;
    let listed = models(&mut client, CODEX).await;
    assert_eq!(listed.state, ModelsState::Unavailable);
    assert!(listed.models.is_empty());
    assert_eq!(listed.default_model, None);
    assert_eq!(
        listed.detail.as_deref(),
        Some("ChatGPT does not list its models.")
    );

    // A provider that cannot be asked says why: it has no program here.
    let missing = models(&mut client, GROK).await;
    assert_eq!(missing.state, ModelsState::Unavailable);
    assert!(
        missing
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("no Grok runtime")),
        "{missing:?}"
    );

    // Or nobody is signed in to it.
    use_scripted(&mut client, GROK).await;
    let signed_out = models(&mut client, GROK).await;
    assert_eq!(signed_out.state, ModelsState::Unavailable);
    assert_eq!(
        signed_out.detail.as_deref(),
        Some("Sign in to Grok to see its models.")
    );
}

#[tokio::test]
async fn a_model_the_provider_no_longer_offers_falls_back_to_its_default_and_says_so() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CODEX).await;
    let bot = client
        .create_bot(BotDraft {
            name: "Atlas".to_owned(),
            provider: Some(CODEX.to_owned()),
            model: Some("gpt-retired".to_owned()),
            ..BotDraft::default()
        })
        .await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;

    client.say(conversation, "settings").await;
    let mut fallback = None;
    let mut cleared = false;
    let mut reply = String::new();
    loop {
        match client.next().await {
            ServerMessage::BotChanged(summary) if summary.bot.id == bot.bot.id => {
                cleared |= summary.bot.model.is_none();
            }
            ServerMessage::ConversationEvent { event, .. } => match event {
                Event::ModelUnavailable { model, provider } => fallback = Some((model, provider)),
                Event::MessageText(text) => reply.push_str(&text),
                Event::TurnEnded(end) => {
                    assert_eq!(end, TurnEnd::Completed);
                    break;
                }
                Event::TurnFailed { message, .. } => panic!("the turn failed: {message}"),
                _ => {}
            },
            _ => {}
        }
    }
    assert_eq!(
        fallback,
        Some(("gpt-retired".to_owned(), Some(CODEX.to_owned())))
    );
    assert!(cleared, "no client was told that the bot changed model");
    assert_eq!(field(&reply, "model"), "gpt-fast", "{reply}");
    assert_eq!(client.bot(bot.bot.id).await.bot.model, None);

    // The change is in the conversation, and the session goes on.
    let stored = support::events(&client.open(conversation).await);
    assert!(stored.contains(&Event::ModelUnavailable {
        model: "gpt-retired".to_owned(),
        provider: Some(CODEX.to_owned()),
    }));
    let session = client.ask(conversation, "session").await;
    assert!(session.ends_with("resumed=false"), "{session}");
    let stored = support::events(&client.open(conversation).await);
    let fallbacks = stored
        .iter()
        .filter(|event| matches!(event, Event::ModelUnavailable { .. }))
        .count();
    assert_eq!(fallbacks, 1);
}

// US-033: a provider switch keeps the bot.

#[tokio::test]
async fn a_bot_that_moves_to_another_provider_keeps_its_conversation_and_hands_it_over() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CLAUDE).await;
    ready(&daemon, &mut client, CODEX).await;
    let (bot, conversation) = conversation_on(&mut client, CLAUDE).await;
    assert_eq!(
        client
            .ask(conversation, "say The launch code is 7421.")
            .await,
        "The launch code is 7421."
    );

    move_to(&mut client, bot.bot.id, CODEX).await;
    client.say(conversation, "config").await;
    let moved = client
        .until_event(|event| matches!(event, Event::ProviderChanged { .. }))
        .await;
    assert_eq!(
        moved,
        Event::ProviderChanged {
            from: CLAUDE.to_owned(),
            to: CODEX.to_owned(),
        }
    );
    let mut config = String::new();
    loop {
        match client.until_event(|_| true).await {
            Event::MessageText(text) => config.push_str(&text),
            event if event.ends_turn() => break,
            _ => {}
        }
    }
    // The new session read what was said with the other provider.
    assert!(
        config.contains("This conversation ran on Claude until now."),
        "{config}"
    );
    assert!(
        config.contains("User:\nsay The launch code is 7421."),
        "{config}"
    );
    assert!(
        config.contains("Assistant:\nThe launch code is 7421."),
        "{config}"
    );
    assert!(config.contains("model=gpt-fast"), "{config}");

    // The second turn on the new provider gets no handoff again.
    let config = client.ask(conversation, "config").await;
    assert!(config.contains("preface=[]"), "{config}");

    let snapshot = client.open(conversation).await;
    assert_eq!(
        user_texts(&snapshot.events),
        ["say The launch code is 7421.", "config", "config"]
    );
    let changes = support::events(&snapshot)
        .iter()
        .filter(|event| matches!(event, Event::ProviderChanged { .. }))
        .count();
    assert_eq!(changes, 1);
}

#[tokio::test]
async fn a_handoff_holds_fifty_thousand_characters_at_most_and_keeps_the_latest() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CLAUDE).await;
    ready(&daemon, &mut client, CODEX).await;
    let (bot, conversation) = conversation_on(&mut client, CLAUDE).await;
    client
        .ask(conversation, &format!("say ancient {}", "a".repeat(40_000)))
        .await;
    client
        .ask(conversation, &format!("say recent {}", "b".repeat(20_000)))
        .await;

    move_to(&mut client, bot.bot.id, CODEX).await;
    let config = client.ask(conversation, "config").await;
    let handoff = config
        .split_once("preface=[")
        .and_then(|(_, rest)| rest.rsplit_once("] model="))
        .map(|(handoff, _)| handoff)
        .unwrap();
    assert_eq!(handoff.chars().count(), 50_000);
    assert!(handoff.contains("[Earlier messages are left out.]"));
    assert!(handoff.contains("recent bbbb"));
    assert!(!handoff.contains("ancient"));
}

#[tokio::test]
async fn a_switch_during_a_turn_applies_at_the_next_turn() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CLAUDE).await;
    ready(&daemon, &mut client, CODEX).await;
    let (bot, conversation) = conversation_on(&mut client, CLAUDE).await;

    client.say(conversation, "stream 30 30").await;
    client.until_reply("w2 ").await;
    move_to(&mut client, bot.bot.id, CODEX).await;
    // The running turn ends on the provider it started on.
    client.until_reply("w29 ").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
    let stored = support::events(&client.open(conversation).await);
    assert!(
        !stored
            .iter()
            .any(|event| matches!(event, Event::ProviderChanged { .. }))
    );

    let started = client.ask(conversation, "started").await;
    assert!(started.contains("CODEX_HOME="), "{started}");
    let stored = support::events(&client.open(conversation).await);
    assert!(stored.contains(&Event::ProviderChanged {
        from: CLAUDE.to_owned(),
        to: CODEX.to_owned(),
    }));
}

#[tokio::test]
async fn a_signed_out_new_provider_fails_the_turn_and_switching_back_restores_the_session() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CLAUDE).await;
    let signed_out = use_scripted(&mut client, CODEX).await;
    assert_eq!(signed_out.state, ProviderState::SignedOut);
    let (bot, conversation) = conversation_on(&mut client, CLAUDE).await;
    let first = client.ask(conversation, "session").await;
    assert!(first.ends_with("resumed=false"), "{first}");

    move_to(&mut client, bot.bot.id, CODEX).await;
    client.say(conversation, "say hello").await;
    let (_, cause, failed) = failed_turn(&mut client).await;
    assert_eq!(cause, Some(FailureCause::SignInRequired));
    assert_eq!(failed.as_deref(), Some(CODEX));
    // The bot keeps the provider it was moved to.
    assert_eq!(client.bot(bot.bot.id).await.bot.provider, CODEX);

    move_to(&mut client, bot.bot.id, CLAUDE).await;
    let back = client.ask(conversation, "session").await;
    assert_eq!(field(&back, "session"), field(&first, "session"));
    assert!(back.ends_with("resumed=true"), "{back}");
}

#[tokio::test]
async fn switching_back_resumes_the_session_each_provider_held() {
    let daemon = daemon();
    let mut client = daemon.client().await;
    ready(&daemon, &mut client, CLAUDE).await;
    ready(&daemon, &mut client, CODEX).await;
    let (bot, conversation) = conversation_on(&mut client, CLAUDE).await;
    let claude = client.ask(conversation, "session").await;

    move_to(&mut client, bot.bot.id, CODEX).await;
    let codex = client.ask(conversation, "session").await;
    assert!(codex.ends_with("resumed=false"), "{codex}");
    assert_ne!(field(&codex, "session"), field(&claude, "session"));

    // Back on the first provider: its session is resumed, and reads what
    // the other provider said meanwhile, and only that.
    move_to(&mut client, bot.bot.id, CLAUDE).await;
    let config = client.ask(conversation, "config").await;
    assert!(
        config.contains("This conversation ran on ChatGPT until now."),
        "{config}"
    );
    assert!(config.contains(field(&codex, "session")), "{config}");
    assert!(!config.contains(field(&claude, "session")), "{config}");
    let back = client.ask(conversation, "session").await;
    assert_eq!(field(&back, "session"), field(&claude, "session"));
    assert!(back.ends_with("resumed=true"), "{back}");

    move_to(&mut client, bot.bot.id, CODEX).await;
    let again = client.ask(conversation, "session").await;
    assert_eq!(field(&again, "session"), field(&codex, "session"));
    assert!(again.ends_with("resumed=true"), "{again}");

    let moves: Vec<(String, String)> = support::events(&client.open(conversation).await)
        .into_iter()
        .filter_map(|event| match event {
            Event::ProviderChanged { from, to } => Some((from, to)),
            _ => None,
        })
        .collect();
    let pair = |from: &str, to: &str| (from.to_owned(), to.to_owned());
    assert_eq!(
        moves,
        [
            pair(CLAUDE, CODEX),
            pair(CODEX, CLAUDE),
            pair(CLAUDE, CODEX)
        ]
    );
}

#[tokio::test]
#[ignore = "starts the real Grok CLI named by CROWBOT_GROK_EXE and opens its sign-in, without a browser"]
async fn the_real_grok_names_the_page_of_its_sign_in() {
    let grok = std::env::var("CROWBOT_GROK_EXE").expect("CROWBOT_GROK_EXE names the Grok CLI");
    let daemon = daemon();
    let mut client = daemon.client().await;
    client
        .send(ClientMessage::SetProviderExecutable {
            provider: GROK.to_owned(),
            path: Some(grok),
        })
        .await;
    let status = until_provider(&mut client, GROK, |status| {
        status.executable.is_some() && settled(status)
    })
    .await;
    assert_eq!(status.state, ProviderState::SignedOut, "{status:?}");
    client
        .send(ClientMessage::SignIn {
            provider: GROK.to_owned(),
            method: None,
        })
        .await;
    let named = until_provider(&mut client, GROK, |status| status.sign_in_url.is_some()).await;
    let page = named.sign_in_url.unwrap();
    println!("Grok names a page on {}", page.split('?').next().unwrap());
    assert!(page.starts_with("https://"), "{page}");
    client
        .send(ClientMessage::CancelSignIn {
            provider: GROK.to_owned(),
        })
        .await;
}
