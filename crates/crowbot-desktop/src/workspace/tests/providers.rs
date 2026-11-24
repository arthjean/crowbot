//! More providers and managed runtimes: the pickers of the provider, the
//! model, and the reasoning level, the sign-ins of a provider, the runtime
//! the daemon installs, and what a conversation tells about them.

use crowbot_protocol::{
    Bot, BotDraft, FailureCause, ModelChoice, ModelsState, ProviderModels, ProviderState,
    ProviderStatus, RuntimeInstall, SignInMethod, TurnEnd,
};

use gpui::{ScrollDelta, ScrollEvent};

use super::*;
use crate::transcript::RowKind;

fn status(id: &str, name: &str, state: ProviderState) -> ProviderStatus {
    ProviderStatus {
        id: id.to_owned(),
        name: name.to_owned(),
        state,
        ..ProviderStatus::default()
    }
}

fn both_ready() -> Vec<ProviderStatus> {
    vec![
        status("claude", "Claude", ProviderState::Ready),
        status("codex", "ChatGPT", ProviderState::Ready),
    ]
}

fn offered(ids: &[&str]) -> Vec<ModelChoice> {
    ids.iter()
        .map(|id| ModelChoice {
            id: (*id).to_owned(),
            name: id.to_uppercase(),
            ..ModelChoice::default()
        })
        .collect()
}

fn listed(provider: &str, models: &[&str], efforts: &[&str]) -> ProviderModels {
    ProviderModels {
        provider: provider.to_owned(),
        state: ModelsState::Listed,
        models: offered(models),
        default_model: models.first().map(|model| (*model).to_owned()),
        efforts: offered(efforts),
        default_effort: efforts.first().map(|effort| (*effort).to_owned()),
        detail: None,
    }
}

fn list_models(provider: &str) -> ClientMessage {
    ClientMessage::ListModels {
        provider: provider.to_owned(),
    }
}

fn patches(sent: &[ClientMessage]) -> Vec<BotPatch> {
    sent.iter()
        .filter_map(|message| match message {
            ClientMessage::UpdateBot { patch, .. } => Some(patch.clone()),
            _ => None,
        })
        .collect()
}

/// A window with the settings panel of the first bot, which runs on Claude.
fn panel(cx: &mut TestAppContext) -> (Entity<Workspace>, Daemon, &mut VisualTestContext) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1600.)));
    cx.update(|window, _| window.activate_window());
    daemon.connect(two_bots(), cx);
    click(cx, "bot-1");
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(10, 1, Vec::new())),
        cx,
    );
    click(cx, "open-bot-settings");
    // The panel asks the provider of the bot for its models.
    assert!(daemon.sent().contains(&list_models("claude")));
    (workspace, daemon, cx)
}

fn notice_keys(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<&'static str> {
    workspace.read_with(cx, |this, _| {
        this.open
            .as_ref()
            .unwrap()
            .transcript
            .rows()
            .iter()
            .filter(|row| matches!(row.kind, RowKind::Notice(_)))
            .map(|row| &*Box::leak(format!("notice-{}", row.key).into_boxed_str()))
            .collect()
    })
}

// US-031: models and reasoning effort.

#[gpui::test]
fn the_panel_lists_what_the_provider_offers_and_marks_the_current_choices(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = panel(cx);
    daemon.tell(ServerMessage::Providers(both_ready()), cx);
    daemon.tell(
        ServerMessage::ProviderModels(listed("claude", &["fast", "smart"], &["low", "high"])),
        cx,
    );
    for choice in [
        "panel-provider-choice-0-chosen",
        "panel-provider-choice-1",
        "panel-model-choice-0-chosen",
        "panel-model-choice-1",
        "panel-model-choice-2",
        "panel-effort-0-chosen",
        "panel-effort-1",
        "panel-effort-2",
    ] {
        assert!(shown(cx, choice), "{choice}");
    }
    daemon.sent();

    click(cx, "panel-model-choice-2");
    assert_eq!(
        patches(&daemon.sent()),
        [BotPatch {
            model: Some(Some("smart".to_owned())),
            ..BotPatch::default()
        }]
    );
    let smart = summary(1, "Atlas", BotState::Idle);
    daemon.tell(
        ServerMessage::BotChanged(BotSummary {
            bot: Bot {
                model: Some("smart".to_owned()),
                ..smart.bot.clone()
            },
            ..smart
        }),
        cx,
    );
    assert!(shown(cx, "panel-model-choice-2-chosen"));
    assert!(shown(cx, "panel-model-choice-0"));

    click(cx, "panel-effort-2");
    assert_eq!(
        patches(&daemon.sent()),
        [BotPatch {
            effort: Some(Some("high".to_owned())),
            ..BotPatch::default()
        }]
    );
    assert!(shown(cx, "panel-effort-2-chosen"));
    click(cx, "panel-effort-0");
    assert_eq!(
        patches(&daemon.sent()),
        [BotPatch {
            effort: Some(None),
            ..BotPatch::default()
        }]
    );

    // Another provider: the bot moves, and that provider is asked for its models.
    click(cx, "panel-provider-choice-1");
    let sent = daemon.sent();
    assert_eq!(
        patches(&sent),
        [BotPatch {
            provider: Some("codex".to_owned()),
            ..BotPatch::default()
        }]
    );
    assert!(sent.contains(&list_models("codex")), "{sent:?}");
    assert!(shown(cx, "panel-provider-choice-1-chosen"));
}

#[gpui::test]
fn a_provider_with_no_model_list_shows_its_default_and_says_so(cx: &mut TestAppContext) {
    let (_workspace, daemon, cx) = panel(cx);
    daemon.tell(
        ServerMessage::ProviderModels(ProviderModels {
            provider: "claude".to_owned(),
            state: ModelsState::Loading,
            ..ProviderModels::default()
        }),
        cx,
    );
    assert!(shown(cx, "panel-models-note"));

    daemon.tell(
        ServerMessage::ProviderModels(ProviderModels {
            provider: "claude".to_owned(),
            state: ModelsState::Unavailable,
            default_model: Some("sonnet".to_owned()),
            detail: Some("Claude does not list its models.".to_owned()),
            ..ProviderModels::default()
        }),
        cx,
    );
    assert!(shown(cx, "panel-models-note"));
    assert!(shown(cx, "panel-model-choice-0-chosen"));
    assert!(!shown(cx, "panel-model-choice-1"));
    assert!(!shown(cx, "panel-effort"));
    // The model can still be typed.
    assert!(shown(cx, "panel-model"));

    daemon.tell(
        ServerMessage::ProviderModels(listed("claude", &["fast"], &[])),
        cx,
    );
    assert!(!shown(cx, "panel-models-note"));
    assert!(shown(cx, "panel-model-choice-1"));
}

// US-030: the sign-ins of a provider, and US-033: the provider of a new bot.

#[gpui::test]
fn the_creation_form_picks_the_provider_its_model_and_its_reasoning_level(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1600.)));
    daemon.connect(Vec::new(), cx);
    let signed_out = ProviderStatus {
        can_sign_in: true,
        sign_in_methods: vec![
            SignInMethod {
                id: "chat-gpt".to_owned(),
                name: "Sign in with ChatGPT".to_owned(),
                description: None,
            },
            SignInMethod {
                id: "device-code".to_owned(),
                name: "Sign in with a device code".to_owned(),
                description: None,
            },
        ],
        ..status("codex", "ChatGPT", ProviderState::SignedOut)
    };
    daemon.tell(
        ServerMessage::Providers(vec![
            status("claude", "Claude", ProviderState::Ready),
            signed_out,
        ]),
        cx,
    );
    assert!(daemon.sent().contains(&list_models("claude")));
    click(cx, "creation-model-row");
    assert!(shown(cx, "creation-provider-0-chosen"));
    assert!(shown(cx, "creation-provider-1"));
    assert!(!shown(cx, "creation-provider-notice"));
    daemon.tell(
        ServerMessage::ProviderModels(listed("claude", &["fast", "smart"], &[])),
        cx,
    );
    // The default model is chosen, and the provider has no reasoning levels.
    assert!(shown(cx, "creation-model-choice-0-chosen"));
    assert!(shown(cx, "creation-model-choice-1"));
    assert!(!shown(cx, "creation-model-choice-2"));
    assert!(!shown(cx, "creation-effort"));

    // A tab shows the models of another provider, and the bot moves there
    // only when one of them is picked.
    click(cx, "creation-provider-1");
    assert_eq!(daemon.sent(), [list_models("codex")]);
    assert!(shown(cx, "creation-provider-1-chosen"));
    assert!(shown(cx, "creation-models-note"));
    assert!(!shown(cx, "creation-provider-notice"));
    click(cx, "creation-model-choice-0");
    assert!(!shown(cx, "creation-model-picker"));

    // The other provider is signed out: its sign-ins are listed by name.
    assert!(shown(cx, "creation-provider-notice"));
    assert!(shown(cx, "creation-sign-in-chat-gpt"));
    click(cx, "creation-sign-in-device-code");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SignIn {
            provider: "codex".to_owned(),
            method: Some("device-code".to_owned()),
        }]
    );

    daemon.tell(
        ServerMessage::ProviderChanged(status("codex", "ChatGPT", ProviderState::Ready)),
        cx,
    );
    assert!(!shown(cx, "creation-provider-notice"));
    // A provider that changed state is asked for its models again.
    assert_eq!(daemon.sent(), [list_models("codex")]);
    daemon.tell(
        ServerMessage::ProviderModels(listed(
            "codex",
            &["gpt-fast", "gpt-smart"],
            &["medium", "low", "high"],
        )),
        cx,
    );
    click(cx, "creation-model-row");
    click(cx, "creation-model-choice-1");
    assert_eq!(
        workspace.read_with(cx, |this, _| this
            .creation
            .as_ref()
            .and_then(|creation| creation.model.clone())),
        Some("gpt-smart".to_owned())
    );
    // A provider with reasoning levels keeps the picker open for them.
    assert!(shown(cx, "creation-model-choice-1-chosen"));
    click(cx, "creation-effort");
    assert!(shown(cx, "creation-effort-0-chosen"), "the default level");
    click(cx, "creation-effort-2");
    assert!(!shown(cx, "creation-effort-menu"));
    click(cx, "creation-effort");
    assert!(shown(cx, "creation-effort-2-chosen"));
    cx.simulate_keystrokes("escape");
    assert!(!shown(cx, "creation-effort-menu"));
    assert!(
        shown(cx, "creation-model-picker"),
        "Escape closes the menu first"
    );
    // A click outside the picker closes it, and lands nowhere else.
    click(cx, "creation-name");
    assert!(!shown(cx, "creation-model-picker"));
    type_name(cx, "Atlas");
    click(cx, "create-bot");
    let created = daemon.sent();
    let Some(ClientMessage::CreateBot { draft }) = created.last() else {
        panic!("no bot was created: {created:?}");
    };
    assert_eq!(
        (
            draft.provider.as_deref(),
            draft.model.as_deref(),
            draft.effort.as_deref()
        ),
        (Some("codex"), Some("gpt-smart"), Some("high"))
    );
    assert_eq!(
        draft.name,
        BotDraft {
            name: "Atlas".to_owned(),
            ..BotDraft::default()
        }
        .name
    );
}

// US-032: managed agent runtimes.

#[gpui::test]
fn a_provider_shows_its_runtime_its_download_and_takes_an_executable_of_the_user(
    cx: &mut TestAppContext,
) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    click(cx, "settings-providers");
    let claude = |state| ProviderStatus {
        runtime: Some("claude-agent-acp 0.107.0".to_owned()),
        ..status("claude", "Claude", state)
    };
    let set = |path: Option<&str>| ClientMessage::SetProviderExecutable {
        provider: "claude".to_owned(),
        path: path.map(str::to_owned),
    };
    daemon.tell(
        ServerMessage::Providers(vec![claude(ProviderState::NotInstalled)]),
        cx,
    );
    daemon.sent();
    click(cx, "provider-0");
    assert!(shown(cx, "provider-0-runtime"));
    assert!(!shown(cx, "provider-0-download"));
    click(cx, "provider-0-install");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::InstallRuntime {
            provider: "claude".to_owned()
        }]
    );

    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            install: Some(RuntimeInstall {
                received: 50 << 20,
                total: 228 << 20,
            }),
            ..claude(ProviderState::NotInstalled)
        }),
        cx,
    );
    assert!(shown(cx, "provider-0-download"));
    assert!(!shown(cx, "provider-0-install"));
    daemon.tell(
        ServerMessage::ProviderChanged(claude(ProviderState::Ready)),
        cx,
    );
    assert!(!shown(cx, "provider-0-download"));
    assert!(!shown(cx, "provider-0-install"));

    // The user's own executable, in place of the runtime.
    assert!(!shown(cx, "provider-0-installed-runtime"));
    click(cx, "provider-0-own-executable");
    click(cx, "provider-executable");
    cx.simulate_input("C:/tools/agent.exe");
    click(cx, "save-executable");
    assert_eq!(daemon.sent(), [set(Some("C:/tools/agent.exe"))]);
    daemon.tell(
        ServerMessage::InvalidField {
            field: "executable".to_owned(),
            message: "The file C:/tools/agent.exe does not exist.".to_owned(),
        },
        cx,
    );
    assert!(shown(cx, "provider-executable-error"));
    assert!(shown(cx, "provider-executable"));

    click(cx, "save-executable");
    assert_eq!(daemon.sent(), [set(Some("C:/tools/agent.exe"))]);
    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            executable: Some("C:/tools/agent.exe".to_owned()),
            ..claude(ProviderState::Ready)
        }),
        cx,
    );
    assert!(!shown(cx, "provider-executable"));
    click(cx, "provider-0-installed-runtime");
    assert_eq!(daemon.sent(), [set(None)]);
}

/// A conversation that holds the events of the daemon's tools opens, and
/// each shows (EP-001 of the harness PRD).
#[gpui::test]
fn a_conversation_with_daemon_tools_and_a_note_opens_and_shows_each_row(cx: &mut TestAppContext) {
    let (workspace, daemon, cx) = open(cx);
    daemon_connected(&daemon, cx);
    click(cx, "bot-1");
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(
            10,
            1,
            vec![
                Event::UserText(
                    crowbot_protocol::UserMessage::new("go")
                        .with_note(Some("Memory changed.".to_owned())),
                ),
                Event::ToolCallStarted {
                    id: "t1".to_owned(),
                    title: "mcp__crowbot__memory".to_owned(),
                    status: None,
                    kind: None,
                    input: None,
                    effect: Some(crowbot_protocol::ToolEffect::BotState),
                },
                Event::ApprovalResolved {
                    id: Some("a1".to_owned()),
                    tool_call_id: "t1".to_owned(),
                    title: Some("mcp__crowbot__memory".to_owned()),
                    decision: crowbot_protocol::ApprovalDecision::Allowed,
                    reason: Some(crowbot_protocol::ApprovalReason::DaemonTool),
                },
                Event::ToolRefused {
                    tool: "probe_review".to_owned(),
                    message: "This session has no tool named probe_review.".to_owned(),
                },
                Event::MessageText("ok".to_owned()),
                Event::TurnEnded(TurnEnd::Completed),
            ],
        )),
        cx,
    );
    let notices = notice_keys(&workspace, cx);
    assert_eq!(notices.len(), 1);
    assert!(shown(cx, notices[0]), "{}", notices[0]);
    let rows = workspace.read_with(cx, |this, _| {
        this.open.as_ref().unwrap().transcript.rows().to_vec()
    });
    assert!(rows.iter().any(|row| matches!(
        &row.kind,
        RowKind::User { note: Some(note), .. } if note == "Memory changed."
    )));
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row.kind, RowKind::Approval(_))),
        "a daemon tool's approval has no card of its own"
    );
}

#[gpui::test]
fn the_conversation_tells_a_download_a_provider_change_and_a_model_that_is_gone(
    cx: &mut TestAppContext,
) {
    let (workspace, daemon, cx) = open(cx);
    daemon_connected(&daemon, cx);
    click(cx, "bot-1");
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(
            10,
            1,
            vec![
                Event::user_text("hi".to_owned()),
                Event::ProviderChanged {
                    from: "claude".to_owned(),
                    to: "codex".to_owned(),
                },
                Event::ModelUnavailable {
                    model: "opus".to_owned(),
                    provider: Some("codex".to_owned()),
                },
                Event::MessageText("ok".to_owned()),
                Event::TurnEnded(TurnEnd::Completed),
            ],
        )),
        cx,
    );
    let notices = notice_keys(&workspace, cx);
    assert_eq!(notices.len(), 2);
    for notice in notices {
        assert!(shown(cx, notice), "{notice}");
    }

    // A turn that waits for its runtime tells how far the download is.
    daemon.tell(
        ServerMessage::ConversationEvent {
            conversation_id: 10,
            event: Event::user_text("again".to_owned()),
            at_ms: NOON + 10_000,
        },
        cx,
    );
    assert!(shown(cx, "activity"));
    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            install: Some(RuntimeInstall {
                received: 1 << 20,
                total: 8 << 20,
            }),
            ..status("claude", "Claude", ProviderState::NotInstalled)
        }),
        cx,
    );
    assert!(shown(cx, "activity-download"));
    daemon.tell(
        ServerMessage::ProviderChanged(status("claude", "Claude", ProviderState::Ready)),
        cx,
    );
    assert!(!shown(cx, "activity-download"));
    assert!(shown(cx, "activity"));
}

fn daemon_connected(daemon: &Daemon, cx: &mut VisualTestContext) {
    daemon.link(ConnectionEvent::Connected, cx);
    daemon.tell(ServerMessage::Preferences(Preferences::default()), cx);
    daemon.tell(ServerMessage::Bots(two_bots()), cx);
}

#[gpui::test]
fn a_turn_that_failed_for_a_sign_in_offers_the_sign_in_of_its_provider(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "bot-1");
    daemon.sent();
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(
            10,
            1,
            vec![
                Event::user_text("hi".to_owned()),
                Event::TurnFailed {
                    message: "Authentication required".to_owned(),
                    cause: Some(FailureCause::SignInRequired),
                    provider: Some("claude".to_owned()),
                },
            ],
        )),
        cx,
    );
    // The state of the provider was asked for at the connection: the
    // sign-in waits for it.
    assert!(!daemon.sent().contains(&ClientMessage::ListProviders));
    assert!(!shown(cx, "turn-sign-in"));
    daemon.tell(
        ServerMessage::Providers(vec![ProviderStatus {
            can_sign_in: true,
            ..status("claude", "Claude", ProviderState::SignedOut)
        }]),
        cx,
    );
    click(cx, "turn-sign-in");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SignIn {
            provider: "claude".to_owned(),
            method: None,
        }]
    );
    daemon.tell(
        ServerMessage::ProviderChanged(status("claude", "Claude", ProviderState::Ready)),
        cx,
    );
    assert!(!shown(cx, "turn-sign-in"));
}

#[gpui::test]
fn the_page_of_a_provider_scrolls_until_its_last_row_is_reachable(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    cx.simulate_resize(size(px(1000.), px(320.)));
    click(cx, "open-settings");
    click(cx, "settings-providers");
    daemon.tell(
        ServerMessage::Providers(vec![ProviderStatus {
            runtime: Some("grok 1.0.77 (69 MB)".to_owned()),
            ..status("grok", "Grok", ProviderState::NotInstalled)
        }]),
        cx,
    );
    daemon.sent();
    click(cx, "provider-0");
    let body = bounds(cx, "settings-body");
    let own = bounds(cx, "provider-0-own-executable");
    assert!(
        own.bottom() > body.bottom(),
        "the last row fits without a scroll: {own:?} in {body:?}"
    );
    cx.simulate_event(ScrollEvent {
        position: body.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-2000.))),
        ..ScrollEvent::default()
    });
    cx.run_until_parked();
    let own = bounds(cx, "provider-0-own-executable");
    assert!(
        own.bottom() <= body.bottom() && own.top() >= body.top(),
        "the last row stays out of reach: {own:?} in {body:?}"
    );
    click(cx, "provider-0-own-executable");
    assert!(shown(cx, "provider-executable"));
}

#[gpui::test]
fn the_window_that_starts_a_sign_in_opens_the_page_the_provider_names(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    click(cx, "settings-providers");
    let signed_out = ProviderStatus {
        can_sign_in: true,
        ..status("grok", "Grok", ProviderState::SignedOut)
    };
    let page = "https://accounts.example/oauth2/device?user_code=FAKE-1234";
    let named = ProviderStatus {
        signing_in: true,
        sign_in_url: Some(page.to_owned()),
        ..signed_out.clone()
    };
    daemon.tell(ServerMessage::Providers(vec![signed_out.clone()]), cx);
    click(cx, "provider-0");

    // A sign-in that another window started shows the page, and opens nothing.
    daemon.tell(ServerMessage::ProviderChanged(named.clone()), cx);
    assert!(shown(cx, "provider-0-sign-in-page"));
    // The row shows the code that the page asks to check.
    assert!(shown(cx, "provider-0-sign-in-code"));
    assert_eq!(cx.opened_url(), None);
    daemon.tell(ServerMessage::ProviderChanged(signed_out.clone()), cx);
    assert!(!shown(cx, "provider-0-sign-in-page"));

    // The window that starts it opens the page once, when the provider names it.
    daemon.sent();
    click(cx, "provider-0-sign-in");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SignIn {
            provider: "grok".to_owned(),
            method: None,
        }]
    );
    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            signing_in: true,
            ..signed_out.clone()
        }),
        cx,
    );
    assert_eq!(cx.opened_url(), None);
    daemon.tell(ServerMessage::ProviderChanged(named.clone()), cx);
    assert_eq!(cx.opened_url().as_deref(), Some(page));
    click(cx, "provider-0-sign-in-page");
    assert_eq!(cx.opened_url().as_deref(), Some(page));
}

#[test]
fn a_sign_in_code_is_read_from_the_page_and_nothing_else() {
    use super::super::forms::sign_in_code;
    assert_eq!(
        sign_in_code("https://accounts.x.ai/oauth2/device?user_code=FJK4-52Q5").as_deref(),
        Some("FJK4-52Q5")
    );
    assert_eq!(
        sign_in_code("https://a.example/device?x=1&user_code=ab-12").as_deref(),
        Some("AB-12")
    );
    assert_eq!(sign_in_code("https://a.example/device"), None);
    assert_eq!(sign_in_code("https://a.example/device?user_code="), None);
    assert_eq!(
        sign_in_code("https://a.example/?user_code=%3Cscript%3E"),
        None
    );
}

#[gpui::test]
fn the_reasoning_levels_follow_the_model_and_the_gear_opens_the_provider_settings(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1600.)));
    daemon.connect(Vec::new(), cx);
    daemon.tell(
        ServerMessage::Providers(vec![status("claude", "Claude", ProviderState::Ready)]),
        cx,
    );
    // The first model has levels of its own; the second has none.
    let mut models = listed("claude", &["opus", "haiku"], &["low", "high"]);
    models.models[0].efforts = offered(&["low", "high", "max"]);
    models.models[0].default_effort = Some("high".to_owned());
    daemon.tell(ServerMessage::ProviderModels(models), cx);
    daemon.sent();

    click(cx, "creation-model-row");
    click(cx, "creation-effort");
    // The levels of the default model, its own default first.
    assert!(shown(cx, "creation-effort-2"), "low, high, max");
    assert!(shown(cx, "creation-effort-1-chosen"), "high is the default");
    click(cx, "creation-effort-2");
    let effort = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, _| {
            this.creation
                .as_ref()
                .and_then(|creation| creation.effort.clone())
        })
    };
    assert_eq!(effort(cx), Some("max".to_owned()));

    // A model without that level goes back to its own default, and a model
    // with no level of its own takes the levels of the provider.
    click(cx, "creation-model-choice-1");
    assert_eq!(effort(cx), None);
    click(cx, "creation-effort");
    assert!(shown(cx, "creation-effort-0-chosen"), "low is the default");
    assert!(!shown(cx, "creation-effort-2"));
    cx.simulate_keystrokes("escape");

    click(cx, "creation-provider-settings");
    assert!(!shown(cx, "creation-model-picker"));
    assert!(shown(cx, "settings-page"));
}
