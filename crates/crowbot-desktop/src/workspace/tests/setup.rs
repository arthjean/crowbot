//! Setup and settings: the creation form, the settings panel of a bot, the
//! settings page, and the search.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crowbot_protocol::{
    AVATAR_COLORS, BOT_INSTRUCTIONS_LENGTH, ConversationHit, DaemonInfo, MascotShape,
    MessageAuthor, MessageHit, PermissionLevel, ProviderState, ProviderStatus, SearchResults,
    SendShortcut,
};

use super::*;
use crate::composer::Composer;
use crate::theme::{Theme, duration};
use crate::transcript::RowKind;
use crate::workspace::bot_panel::SaveState;
use crate::workspace::creation::TEMPLATES;
use crate::workspace::forms::automatic_color;
use gpui::{Modifiers, ScrollDelta, ScrollEvent};

fn text_of(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    input: impl Fn(&Workspace) -> &Entity<Composer>,
) -> String {
    workspace.read_with(cx, |this, cx| input(this).read(cx).text().to_owned())
}

fn wait(cx: &mut VisualTestContext, time: Duration) {
    cx.executor().advance_clock(time);
    cx.run_until_parked();
}

fn select_all_and_delete(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a backspace"
    } else {
        "ctrl-a backspace"
    });
    cx.run_until_parked();
}

fn search_keys(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-k"
    } else {
        "ctrl-k"
    });
    cx.run_until_parked();
}

fn provider(state: ProviderState) -> ProviderStatus {
    ProviderStatus {
        id: "claude".to_owned(),
        name: "Claude".to_owned(),
        state,
        can_sign_in: true,
        ..ProviderStatus::default()
    }
}

fn updates(sent: &[ClientMessage]) -> Vec<BotPatch> {
    sent.iter()
        .filter_map(|message| match message {
            ClientMessage::UpdateBot { patch, .. } => Some(patch.clone()),
            _ => None,
        })
        .collect()
}

fn tops(cx: &mut VisualTestContext, selectors: &[&'static str]) -> Vec<f32> {
    selectors
        .iter()
        .map(|selector| f32::from(bounds(cx, selector).top()))
        .collect()
}

fn with_panel(cx: &mut TestAppContext) -> (Entity<Workspace>, Daemon, &mut VisualTestContext) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1400.)));
    // A window that is not active tells nothing about the focus.
    cx.update(|window, _| window.activate_window());
    daemon.connect(two_bots(), cx);
    click(cx, "bot-1");
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(10, 1, Vec::new())),
        cx,
    );
    click(cx, "open-bot-settings");
    daemon.sent();
    (workspace, daemon, cx)
}

// US-026: bot creation.

#[gpui::test]
fn with_no_bot_the_first_bot_form_fills_the_conversation_pane(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1400.)));
    daemon.connect(Vec::new(), cx);
    assert_eq!(daemon.sent().last(), Some(&ClientMessage::ListProviders));

    assert!(shown(cx, "creation"));
    assert!(!shown(cx, "cancel-creation"));
    assert!(!shown(cx, "send-message"));
    let pane = bounds(cx, "creation");
    assert!(pane.left() >= bounds(cx, "sidebar").right() - px(1.));
    for part in [
        "creation-avatar",
        "creation-color-auto",
        "creation-color-0",
        "creation-color-16",
        "creation-shape-0",
        "creation-shape-5",
        "creation-name",
        "creation-purpose",
        "creation-model-row",
        "create-bot",
        "templates",
        "template-0",
        "template-5",
    ] {
        assert!(shown(cx, part), "{part}");
    }
    let order = tops(
        cx,
        &[
            "creation-avatar",
            "creation-color-0",
            "creation-shape-0",
            "creation-name",
            "creation-purpose",
            "creation-model-row",
            "create-bot",
            "templates",
        ],
    );
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");

    // As in OpenBot, no field takes the focus when the form opens; a key
    // typed there lands nowhere.
    let name_focused = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            workspace
                .read(cx)
                .creation_name
                .focus_handle(cx)
                .is_focused(window)
        })
    };
    assert!(!name_focused(cx));
    cx.simulate_input("x");
    assert_eq!(
        text_of(&workspace, cx, |this| &this.creation_name),
        "New bot"
    );

    // A suggestion fills the name and the purpose.
    type_name(cx, "Atlas");
    assert!(name_focused(cx));
    assert_eq!(text_of(&workspace, cx, |this| &this.creation_name), "Atlas");
    click(cx, "template-1");
    assert_eq!(
        text_of(&workspace, cx, |this| &this.creation_name),
        TEMPLATES[1].name
    );
    assert_eq!(
        text_of(&workspace, cx, |this| &this.creation_purpose),
        TEMPLATES[1].purpose
    );
    assert_eq!(daemon.sent(), []);
}

#[gpui::test]
fn the_automatic_color_follows_the_head_until_a_color_is_picked(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1400.)));
    daemon.connect(Vec::new(), cx);
    let color = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, cx| this.preview_bot(cx).avatar.color)
    };
    // With no bot yet, the form starts on the first head.
    let shape = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, cx| this.preview_bot(cx).avatar.shape)
    };
    assert_eq!(shape(cx), MascotShape::Smooth);
    assert_eq!(color(cx), automatic_color(MascotShape::Smooth));
    click(cx, "creation-shape-3");
    assert_eq!(shape(cx), MascotShape::Curl);
    assert_eq!(color(cx), automatic_color(MascotShape::Curl));
    click(cx, "creation-color-2");
    click(cx, "creation-shape-5");
    assert_eq!(color(cx), AVATAR_COLORS[2], "a picked color stays");
    click(cx, "creation-color-auto");
    assert_eq!(color(cx), automatic_color(MascotShape::Flat));
    // The head is the only choice of shape: the beak is the same for all.
    assert!(!shown(cx, "creation-beak-0"));
}

#[gpui::test]
fn a_suggestion_stays_chosen_until_the_form_changes(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1400.)));
    daemon.connect(Vec::new(), cx);
    let chosen =
        |cx: &mut VisualTestContext| workspace.read_with(cx, |this, cx| this.chosen_template(cx));
    assert_eq!(chosen(cx), None);
    click(cx, "template-1");
    assert_eq!(chosen(cx), Some(1));
    click(cx, "creation-name");
    cx.simulate_input("!");
    assert_eq!(
        chosen(cx),
        None,
        "an edited name is no longer the suggestion"
    );
}

#[gpui::test]
fn the_suggestion_row_scrolls_by_wheel_and_by_drag_and_a_drag_is_not_a_click(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1400.)));
    daemon.connect(Vec::new(), cx);
    let first = |cx: &mut VisualTestContext| bounds(cx, "template-0").left();
    let start = first(cx);

    // A vertical wheel moves the row sideways.
    let row = bounds(cx, "templates");
    cx.simulate_event(ScrollEvent {
        position: row.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
        ..ScrollEvent::default()
    });
    cx.run_until_parked();
    let wheeled = first(cx);
    assert!(wheeled < start, "{wheeled:?} after {start:?}");

    // A drag moves it back, and the card under the release takes no click.
    let from = bounds(cx, "template-1").center();
    let to = point(from.x + px(100.), from.y);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(from.x + px(50.), from.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(first(cx) > wheeled);
    assert_eq!(
        workspace.read_with(cx, |this, cx| this.creation_name.read(cx).text().to_owned()),
        "New bot",
        "the drag picked no suggestion"
    );

    // A plain click still picks one.
    click(cx, "template-1");
    assert_eq!(
        workspace.read_with(cx, |this, cx| this.chosen_template(cx)),
        Some(1)
    );
}

#[gpui::test]
fn create_shows_a_creating_state_then_the_bot_opens_with_the_composer_focused(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(Vec::new(), cx);
    daemon.sent();

    type_name(cx, "  Atlas ");
    click(cx, "creation-purpose");
    cx.simulate_input("Plan the week.");
    // A model the provider does not list is picked by its id.
    click(cx, "creation-model-row");
    click(cx, "creation-model");
    cx.simulate_input("smart");
    press_enter(cx);
    assert!(!shown(cx, "creation-model-picker"));
    click(cx, "creation-color-3");
    click(cx, "creation-shape-2");
    click(cx, "create-bot");
    let sent = daemon.sent();
    let [ClientMessage::CreateBot { draft }] = &sent[..] else {
        panic!("{sent:?}");
    };
    assert_eq!(draft.name, "Atlas");
    assert_eq!(draft.instructions, "Plan the week.");
    assert_eq!(draft.model.as_deref(), Some("smart"));
    let avatar = draft.avatar.as_ref().unwrap();
    assert_eq!(avatar.color, AVATAR_COLORS[3]);
    assert_eq!(avatar.shape, MascotShape::Spikes);

    assert!(shown(cx, "creating-bot"));
    assert!(!shown(cx, "create-bot"));
    click(cx, "creating-bot");
    press_enter(cx);
    assert_eq!(daemon.sent(), [], "one bot is asked for");

    daemon.tell(
        ServerMessage::BotChanged(summary(7, "Atlas", BotState::Idle)),
        cx,
    );
    daemon.tell(ServerMessage::BotCreated { bot_id: 7 }, cx);
    assert!(!shown(cx, "creation"));
    assert!(shown(cx, "bot-7"));
    assert_eq!(
        daemon.sent(),
        [ClientMessage::OpenConversation {
            conversation_id: 70
        }]
    );
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(70, 7, Vec::new())),
        cx,
    );
    daemon.sent();
    cx.simulate_input("hello");
    cx.simulate_keystrokes("enter");
    assert!(daemon.sent().contains(&ClientMessage::SendMessage {
        conversation_id: 70,
        text: "hello".to_owned(),
    }));

    // The next form starts on the default name and an empty purpose.
    click(cx, "new-bot");
    assert_eq!(
        text_of(&workspace, cx, |this| &this.creation_name),
        "New bot"
    );
    assert_eq!(text_of(&workspace, cx, |this| &this.creation_purpose), "");
}

#[gpui::test]
fn the_name_starts_on_a_selected_default_and_an_empty_name_creates_nothing(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(Vec::new(), cx);
    daemon.sent();

    assert_eq!(
        text_of(&workspace, cx, |this| &this.creation_name),
        "New bot"
    );
    click(cx, "creation-name");
    select_all_and_delete(cx);
    cx.simulate_input("   ");
    click(cx, "create-bot");
    press_enter(cx);
    assert_eq!(daemon.sent(), [], "Create waits for a name");
    assert!(workspace.read_with(cx, |this, _| this.bots.is_empty()));
    assert!(!shown(cx, "creating-bot"));
    assert!(!shown(cx, "creation-name-error"));
    click(cx, "creation-name");
    select_all_and_delete(cx);

    // A name the daemon refuses shows on the same field, and the form stays.
    cx.simulate_input("Atlas");
    press_enter(cx);
    assert!(matches!(
        &daemon.sent()[..],
        [ClientMessage::CreateBot { .. }]
    ));
    daemon.tell(
        ServerMessage::InvalidField {
            field: "name".to_owned(),
            message: "A bot's name can hold 80 characters at most.".to_owned(),
        },
        cx,
    );
    assert!(shown(cx, "creation-name-error"));
    assert!(shown(cx, "create-bot"));
    assert!(!shown(cx, "notice"));

    // Another refusal shows above Create.
    press_enter(cx);
    daemon.sent();
    daemon.tell(
        ServerMessage::Error {
            message: "Cannot create the folder.".to_owned(),
        },
        cx,
    );
    assert!(shown(cx, "creation-error"));
    assert!(bounds(cx, "creation-error").bottom() <= bounds(cx, "create-bot").top());
}

#[gpui::test]
fn a_provider_that_cannot_run_is_stated_above_create_with_the_control_that_fixes_it(
    cx: &mut TestAppContext,
) {
    let (_workspace, mut daemon, cx) = open(cx);
    // Tall enough for the whole form: the notice, the model, and Create.
    cx.simulate_resize(size(px(1000.), px(1600.)));
    daemon.connect(Vec::new(), cx);
    daemon.sent();
    assert!(!shown(cx, "creation-provider-notice"));
    let sign_in = || ClientMessage::SignIn {
        provider: "claude".to_owned(),
        method: None,
    };

    daemon.tell(
        ServerMessage::Providers(vec![provider(ProviderState::SignedOut)]),
        cx,
    );
    assert!(shown(cx, "creation-provider-notice"));
    assert!(bounds(cx, "creation-provider-notice").bottom() <= bounds(cx, "create-bot").top());
    // The form asks the provider of the new bot for its models.
    assert_eq!(
        daemon.sent(),
        [ClientMessage::ListModels {
            provider: "claude".to_owned()
        }]
    );
    click(cx, "creation-sign-in");
    assert_eq!(daemon.sent(), [sign_in()]);

    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            signing_in: true,
            ..provider(ProviderState::SignedOut)
        }),
        cx,
    );
    assert!(!shown(cx, "creation-sign-in"));
    click(cx, "creation-cancel-sign-in");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::CancelSignIn {
            provider: "claude".to_owned()
        }]
    );

    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            can_sign_in: false,
            detail: Some("The program `bunx` was not found on this computer.".to_owned()),
            help_url: Some("https://bun.sh".to_owned()),
            ..provider(ProviderState::RuntimeMissing)
        }),
        cx,
    );
    assert!(shown(cx, "creation-provider-notice"));
    assert!(shown(cx, "creation-get-runtime"));
    assert!(!shown(cx, "creation-sign-in"));
    click(cx, "creation-check");
    // A provider that changed state is asked for its models again.
    assert_eq!(
        daemon.sent(),
        [
            ClientMessage::ListModels {
                provider: "claude".to_owned()
            },
            ClientMessage::CheckProvider {
                provider: "claude".to_owned()
            }
        ]
    );

    // Create stays available.
    click(cx, "creation-name");
    cx.simulate_input("Atlas");
    click(cx, "create-bot");
    let created = daemon.sent();
    assert!(
        matches!(&created[..], [ClientMessage::CreateBot { .. }]),
        "{created:?}"
    );
    daemon.tell(
        ServerMessage::InvalidField {
            field: "name".to_owned(),
            message: "no".to_owned(),
        },
        cx,
    );

    daemon.tell(
        ServerMessage::ProviderChanged(provider(ProviderState::Ready)),
        cx,
    );
    assert!(!shown(cx, "creation-provider-notice"));
}

#[gpui::test]
fn a_provider_not_installed_yet_offers_its_install_on_the_creation_form(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1600.)));
    daemon.connect(Vec::new(), cx);
    daemon.sent();
    // A provider downloads its runtime at the first turn, and a person can
    // only sign in once it is there: the form says so, and offers Install.
    daemon.tell(
        ServerMessage::Providers(vec![ProviderStatus {
            detail: Some(
                "Crow Bot downloads claude-agent-acp 1.0 (219 MB) when a bot first needs it."
                    .to_owned(),
            ),
            ..provider(ProviderState::NotInstalled)
        }]),
        cx,
    );
    assert!(shown(cx, "creation-provider-notice"));
    assert!(bounds(cx, "creation-provider-notice").bottom() <= bounds(cx, "create-bot").top());
    daemon.sent();
    click(cx, "creation-install");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::InstallRuntime {
            provider: "claude".to_owned()
        }]
    );
    // Once installed and signed in, the notice goes.
    daemon.tell(
        ServerMessage::ProviderChanged(provider(ProviderState::Ready)),
        cx,
    );
    assert!(!shown(cx, "creation-provider-notice"));
}

#[gpui::test]
fn new_bot_with_a_bot_opens_the_same_form_with_cancel(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(10, 1, Vec::new())),
        cx,
    );
    assert!(!shown(cx, "creation"));
    daemon.sent();

    click(cx, "new-bot");
    assert_eq!(daemon.sent(), []);
    assert!(shown(cx, "creation"));
    assert!(shown(cx, "cancel-creation"));
    for part in [
        "creation-name",
        "creation-purpose",
        "creation-model-row",
        "create-bot",
        "template-0",
    ] {
        assert!(shown(cx, part), "{part}");
    }
    assert!(!shown(cx, "send-message"));
    type_name(cx, "Finch");
    assert_eq!(text_of(&workspace, cx, |this| &this.creation_name), "Finch");

    click(cx, "cancel-creation");
    assert!(!shown(cx, "creation"));
    assert!(shown(cx, "send-message"));

    click(cx, "new-bot");
    assert_eq!(
        text_of(&workspace, cx, |this| &this.creation_name),
        "New bot"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "creation"));

    click(cx, "new-bot");
    click(cx, "bot-2");
    assert!(!shown(cx, "creation"));
    assert_eq!(
        workspace.read_with(cx, |this, _| this.selected_bot),
        Some(2)
    );
    assert!(
        !daemon
            .sent()
            .iter()
            .any(|message| matches!(message, ClientMessage::CreateBot { .. }))
    );
}

#[gpui::test]
fn the_first_bot_form_gives_way_to_a_bot_made_elsewhere_unless_it_holds_text(
    cx: &mut TestAppContext,
) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(Vec::new(), cx);
    type_name(cx, "Mine");
    daemon.tell(
        ServerMessage::BotChanged(summary(1, "Atlas", BotState::Idle)),
        cx,
    );
    assert!(shown(cx, "creation"), "a form that holds text stays");
    assert!(shown(cx, "cancel-creation"));
    daemon.tell(ServerMessage::BotDeleted { bot_id: 1 }, cx);
    assert!(shown(cx, "creation"));
    assert!(!shown(cx, "cancel-creation"));
}

// US-027: the settings panel of a bot.

#[gpui::test]
fn the_panel_shows_its_parts_in_order(cx: &mut TestAppContext) {
    let (_workspace, _daemon, cx) = with_panel(cx);
    let order = tops(
        cx,
        &[
            "panel-avatar",
            "panel-name",
            "panel-title",
            "panel-instructions",
            "panel-provider",
            "panel-model",
            "level-ask-first",
            "level-full-access",
            "panel-folder",
            "panel-new-chat",
            "panel-delete-history",
            "panel-delete-bot",
        ],
    );
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    assert!(shown(cx, "level-ask-first-selected"));
    assert!(!shown(cx, "level-full-access-selected"));
    let panel = bounds(cx, "side-panel");
    assert_eq!(panel.size.width, layout::SIDE_PANEL);
}

#[gpui::test]
fn a_field_saves_after_600_ms_of_idle_or_when_it_loses_the_focus(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_panel(cx);
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_name), "Atlas");

    click(cx, "panel-title");
    cx.simulate_input("Research");
    wait(cx, duration::FIELD_SAVE - Duration::from_millis(1));
    cx.simulate_input("er");
    wait(cx, duration::FIELD_SAVE - Duration::from_millis(1));
    assert_eq!(updates(&daemon.sent()), [], "the typing has not paused");
    wait(cx, Duration::from_millis(1));
    assert_eq!(
        updates(&daemon.sent()),
        [BotPatch {
            title: Some("Researcher".to_owned()),
            ..BotPatch::default()
        }]
    );
    let saving = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, _| this.panel.fields[1].state.clone())
    };
    assert_eq!(saving(cx), SaveState::Saving);
    let mut changed = summary(1, "Atlas", BotState::Idle);
    changed.bot.title = "Researcher".to_owned();
    daemon.tell(ServerMessage::BotChanged(changed.clone()), cx);
    daemon.tell(ServerMessage::BotUpdated { bot_id: 1 }, cx);
    assert_eq!(saving(cx), SaveState::Idle);
    wait(cx, duration::FIELD_SAVE);
    assert_eq!(
        updates(&daemon.sent()),
        [],
        "a saved field is not sent again"
    );

    // A field that loses the focus is saved at once.
    click(cx, "panel-instructions");
    cx.simulate_input("Cite every source.");
    click(cx, "panel-name");
    assert!(shown(cx, "panel-name"));
    assert_eq!(
        updates(&daemon.sent()),
        [BotPatch {
            instructions: Some("Cite every source.".to_owned()),
            ..BotPatch::default()
        }]
    );
    changed.bot.instructions = "Cite every source.".to_owned();
    daemon.tell(ServerMessage::BotChanged(changed.clone()), cx);
    daemon.tell(ServerMessage::BotUpdated { bot_id: 1 }, cx);

    // The model with no text is the default one.
    click(cx, "panel-model");
    cx.simulate_input("smart");
    wait(cx, duration::FIELD_SAVE);
    assert_eq!(
        updates(&daemon.sent()),
        [BotPatch {
            model: Some(Some("smart".to_owned())),
            ..BotPatch::default()
        }]
    );
    changed.bot.model = Some("smart".to_owned());
    daemon.tell(ServerMessage::BotChanged(changed), cx);
    daemon.tell(ServerMessage::BotUpdated { bot_id: 1 }, cx);
    select_all_and_delete(cx);
    wait(cx, duration::FIELD_SAVE);
    assert_eq!(
        updates(&daemon.sent()),
        [BotPatch {
            model: Some(None),
            ..BotPatch::default()
        }]
    );

    for control in ["panel-save", "save-bot", "save"] {
        assert!(!shown(cx, control), "the panel has no save button");
    }
}

#[gpui::test]
fn an_empty_name_goes_back_to_the_saved_one_when_the_field_loses_the_focus(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = with_panel(cx);
    click(cx, "panel-name");
    select_all_and_delete(cx);
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_name), "");
    wait(cx, duration::FIELD_SAVE);
    assert_eq!(updates(&daemon.sent()), [], "an empty name is never sent");

    click(cx, "panel-title");
    assert!(shown(cx, "panel-name"));
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_name), "Atlas");
    wait(cx, duration::FIELD_SAVE);
    assert_eq!(updates(&daemon.sent()), []);
    assert!(!shown(cx, "panel-name-error"));
}

#[gpui::test]
fn a_save_that_fails_shows_its_reason_on_the_field_and_keeps_the_text(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_panel(cx);
    click(cx, "panel-title");
    cx.simulate_input("Planner");
    wait(cx, duration::FIELD_SAVE);
    assert_eq!(updates(&daemon.sent()).len(), 1);
    assert!(!shown(cx, "panel-title-error"));

    daemon.tell(
        ServerMessage::InvalidField {
            field: "title".to_owned(),
            message: "This field can hold 80 characters at most.".to_owned(),
        },
        cx,
    );
    assert!(shown(cx, "panel-title-error"));
    let (field, error) = (bounds(cx, "panel-title"), bounds(cx, "panel-title-error"));
    assert!(error.top() >= field.bottom());
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_title), "Planner");
    assert_eq!(
        workspace.read_with(cx, |this, _| this.panel.fields[1].state.clone()),
        SaveState::Failed("This field can hold 80 characters at most.".into())
    );
    assert!(!shown(cx, "notice"), "the reason shows on the field only");
    assert!(!shown(cx, "panel-name-error"));

    // The next edit is saved again, and a save that works clears the state.
    cx.simulate_input("s");
    wait(cx, duration::FIELD_SAVE);
    assert_eq!(
        updates(&daemon.sent()),
        [BotPatch {
            title: Some("Planners".to_owned()),
            ..BotPatch::default()
        }]
    );
    daemon.tell(ServerMessage::BotUpdated { bot_id: 1 }, cx);
    assert!(!shown(cx, "panel-title-error"));

    // With no daemon, a save fails at once and the text stays.
    cx.simulate_input("!");
    daemon.link(
        ConnectionEvent::Disconnected("the pipe closed".to_owned()),
        cx,
    );
    wait(cx, duration::FIELD_SAVE);
    assert_eq!(updates(&daemon.sent()), []);
    assert_eq!(
        text_of(&workspace, cx, |this| &this.panel_title),
        "Planners!"
    );
    assert_eq!(
        workspace.read_with(cx, |this, _| this.panel.fields[1].state.clone()),
        SaveState::Failed("The daemon cannot be reached.".into())
    );
}

#[gpui::test]
fn a_save_that_fails_after_the_panel_moved_to_another_bot_does_not_mark_that_bot(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = with_panel(cx);
    click(cx, "panel-title");
    cx.simulate_input("Planner");

    // The panel moves to the other bot before the typing paused: the text
    // is sent for the bot it was typed for.
    click(cx, "bot-2");
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(20, 2, Vec::new())),
        cx,
    );
    let sent = daemon.sent();
    assert!(
        sent.contains(&ClientMessage::UpdateBot {
            bot_id: 1,
            patch: BotPatch {
                title: Some("Planner".to_owned()),
                ..BotPatch::default()
            },
        }),
        "{sent:?}"
    );
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_name), "Wren");

    daemon.tell(
        ServerMessage::InvalidField {
            field: "title".to_owned(),
            message: "This field can hold 80 characters at most.".to_owned(),
        },
        cx,
    );
    assert!(
        !shown(cx, "panel-title-error"),
        "the title of the other bot did not fail"
    );
    assert_eq!(
        workspace.read_with(cx, |this, _| this.panel.fields[1].state.clone()),
        SaveState::Idle
    );
    assert!(shown(cx, "notice"), "the failure is still told");
}

#[gpui::test]
fn the_instructions_stop_at_20_000_characters_and_show_the_limit(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    cx.simulate_resize(size(px(1000.), px(1400.)));
    let mut atlas = summary(1, "Atlas", BotState::Idle);
    atlas.bot.instructions = "x".repeat(BOT_INSTRUCTIONS_LENGTH - 2_500);
    daemon.connect(vec![atlas], cx);
    click(cx, "open-bot-settings");
    assert!(!shown(cx, "panel-instructions-count"));
    assert!(!shown(cx, "panel-instructions-limit"));
    let length = |cx: &mut VisualTestContext| {
        text_of(&workspace, cx, |this| &this.panel_instructions)
            .chars()
            .count()
    };

    click(cx, "panel-instructions");
    cx.simulate_input(&"é".repeat(2_480));
    assert_eq!(length(cx), BOT_INSTRUCTIONS_LENGTH - 20);
    assert!(shown(cx, "panel-instructions-count"));
    assert!(!shown(cx, "panel-instructions-limit"));

    cx.simulate_input(&"y".repeat(50));
    assert_eq!(length(cx), BOT_INSTRUCTIONS_LENGTH);
    assert!(shown(cx, "panel-instructions-limit"));
    cx.simulate_input("more");
    assert_eq!(length(cx), BOT_INSTRUCTIONS_LENGTH);
    let text = text_of(&workspace, cx, |this| &this.panel_instructions);
    assert_eq!(text.matches('y').count(), 20, "only what fits is taken");
    assert!(!text.contains("more"), "the field takes no more text");

    wait(cx, duration::FIELD_SAVE);
    let saved = updates(&daemon.sent());
    assert_eq!(
        saved[0]
            .instructions
            .as_ref()
            .map(|text| text.chars().count()),
        Some(BOT_INSTRUCTIONS_LENGTH)
    );
}

#[gpui::test]
fn delete_history_and_delete_bot_say_what_they_remove_and_wait_for_a_confirmation(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = with_panel(cx);

    click(cx, "panel-delete-history");
    assert!(shown(cx, "confirm-dialog"));
    let said = workspace.read_with(cx, |this, _| this.confirm);
    assert_eq!(said, Some(Confirm::DeleteHistory(1)));
    assert!(shown(cx, "confirm-description"));
    click(cx, "confirm-cancel");
    assert!(!shown(cx, "confirm-dialog"));
    assert_eq!(
        daemon.sent(),
        [],
        "nothing is removed before the confirmation"
    );

    click(cx, "panel-delete-history");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "confirm-dialog"));
    assert_eq!(daemon.sent(), []);

    click(cx, "panel-delete-history");
    click(cx, "confirm-ok");
    assert_eq!(daemon.sent(), [ClientMessage::DeleteHistory { bot_id: 1 }]);
    assert!(!shown(cx, "confirm-dialog"));

    // The bot stays, and its new conversation opens with nothing to close.
    daemon.tell(
        ServerMessage::HistoryDeleted {
            bot_id: 1,
            current_conversation_id: 77,
        },
        cx,
    );
    assert_eq!(
        daemon.sent(),
        [ClientMessage::OpenConversation {
            conversation_id: 77
        }]
    );
    assert_eq!(names(&workspace, cx), ["Atlas", "Wren"]);
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(77, 1, Vec::new())),
        cx,
    );
    assert!(shown(cx, "empty-conversation"));
    daemon.sent();

    click(cx, "panel-delete-bot");
    assert!(shown(cx, "delete-dialog"));
    assert_eq!(daemon.sent(), []);
    click(cx, "cancel-delete");
    assert_eq!(daemon.sent(), []);
    assert_eq!(names(&workspace, cx), ["Atlas", "Wren"]);
    click(cx, "panel-delete-bot");
    click(cx, "confirm-delete");
    assert!(
        daemon
            .sent()
            .contains(&ClientMessage::DeleteBot { bot_id: 1 })
    );
}

#[gpui::test]
fn a_change_to_the_level_the_model_or_the_instructions_shows_in_the_header_and_the_sidebar(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = with_panel(cx);
    let bot = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, _| this.selected_bot().unwrap().bot.clone())
    };

    // Raising the level asks first; the change then shows at once.
    click(cx, "level-full-access");
    assert!(shown(cx, "confirm-dialog"));
    assert_eq!(daemon.sent(), []);
    assert_eq!(bot(cx).permission_level, PermissionLevel::AskFirst);
    click(cx, "confirm-ok");
    assert_eq!(
        updates(&daemon.sent()),
        [BotPatch {
            permission_level: Some(PermissionLevel::FullAccess),
            ..BotPatch::default()
        }]
    );
    assert_eq!(bot(cx).permission_level, PermissionLevel::FullAccess);
    assert!(shown(cx, "level-full-access-selected"));
    daemon.tell(ServerMessage::BotUpdated { bot_id: 1 }, cx);

    // Lowering it asks nothing.
    click(cx, "level-ask-first");
    assert!(!shown(cx, "confirm-dialog"));
    assert_eq!(updates(&daemon.sent()).len(), 1);
    assert!(shown(cx, "level-ask-first-selected"));
    daemon.tell(ServerMessage::BotUpdated { bot_id: 1 }, cx);

    // What the daemon tells about the bot is what the header, the sidebar,
    // and the fields show, whichever client changed it.
    let mut changed = summary(1, "Orion", BotState::Idle);
    changed.bot.model = Some("smart".to_owned());
    changed.bot.instructions = "Answer in one line.".to_owned();
    changed.bot.permission_level = PermissionLevel::FullAccess;
    daemon.tell(ServerMessage::BotChanged(changed), cx);
    assert_eq!(bot(cx).model.as_deref(), Some("smart"));
    assert_eq!(names(&workspace, cx), ["Orion", "Wren"]);
    assert!(shown(cx, "level-full-access-selected"));
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_name), "Orion");
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_model), "smart");
    assert_eq!(
        text_of(&workspace, cx, |this| &this.panel_instructions),
        "Answer in one line."
    );
    assert_eq!(
        updates(&daemon.sent()),
        [],
        "a change from the daemon is not sent back"
    );

    // The avatar: a color and a head.
    click(cx, "panel-avatar");
    click(cx, "panel-color-2");
    click(cx, "panel-shape-3");
    let avatars: Vec<_> = updates(&daemon.sent())
        .into_iter()
        .filter_map(|patch| patch.avatar)
        .collect();
    assert_eq!(avatars.len(), 2);
    assert_eq!(avatars[0].color, AVATAR_COLORS[2]);
    assert_eq!(avatars[1].shape, MascotShape::Curl);
    assert_eq!(avatars[1].color, AVATAR_COLORS[2]);
    assert_eq!(bot(cx).avatar, avatars[1]);

    // The fields follow the bot on screen.
    click(cx, "bot-2");
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_name), "Wren");
}

// US-028: the settings page.

fn info() -> DaemonInfo {
    DaemonInfo {
        daemon_version: "9.8.7".to_owned(),
        protocol_version: 6,
        database: "C:\\Users\\me\\.crowbot\\crowbot.db".to_owned(),
        linger: None,
    }
}

/// Opens the menu of a select and takes one of its rows.
fn choose(cx: &mut VisualTestContext, select: &'static str, row: &'static str) {
    click(cx, select);
    click(cx, row);
}

#[gpui::test]
fn the_settings_page_has_general_providers_and_about(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    daemon.sent();
    assert!(!shown(cx, "settings-page"));

    click(cx, "open-settings");
    assert!(shown(cx, "settings-page"));
    assert!(
        !shown(cx, "sidebar") && !shown(cx, "conversation-header"),
        "the page takes the place of the bots and of the conversation"
    );
    assert_eq!(daemon.sent(), [ClientMessage::GetDaemonInfo]);
    let sections = tops(
        cx,
        &["settings-general", "settings-providers", "settings-about"],
    );
    assert!(sections.windows(2).all(|pair| pair[0] < pair[1]));
    for control in ["settings-theme", "settings-send"] {
        assert!(shown(cx, control), "{control}");
    }
    // The section column is as wide as the sidebar, and the groups share
    // one column beside it.
    let (nav, body) = (bounds(cx, "settings-sections"), bounds(cx, "settings-body"));
    assert!(nav.right() <= body.left());
    assert_eq!(
        bounds(cx, "settings-theme").right(),
        bounds(cx, "settings-send").right()
    );

    click(cx, "settings-providers");
    assert!(shown(cx, "providers-loading"));
    assert!(!shown(cx, "settings-theme"));
    daemon.tell(
        ServerMessage::Providers(vec![provider(ProviderState::Ready)]),
        cx,
    );
    // A provider is a row of the list, and has a page of its own.
    assert!(shown(cx, "provider-0"));
    assert!(!shown(cx, "provider-0-state"));
    click(cx, "provider-0");
    assert!(shown(cx, "provider-0-state"));
    assert!(shown(cx, "settings-path-parent"));
    click(cx, "settings-path-parent");
    assert!(shown(cx, "provider-0") && !shown(cx, "provider-0-state"));
    click(cx, "provider-0");
    click(cx, "close-settings");
    assert!(
        shown(cx, "provider-0") && !shown(cx, "provider-0-state"),
        "Back goes one level up, to the list"
    );

    click(cx, "settings-about");
    daemon.tell(ServerMessage::DaemonInfo(info()), cx);
    for row in [
        "about-client",
        "about-daemon",
        "about-protocol",
        "about-database",
    ] {
        assert!(shown(cx, row), "{row}");
    }
    assert_eq!(
        workspace.read_with(cx, |this, _| this.daemon_info.clone()),
        Some(info())
    );
    assert!(!shown(cx, "about-linger"), "the notion does not apply");
    // A daemon whose computer stops the user's services at logout says so,
    // with the command that keeps them; lingering on says nothing.
    for (linger, notice) in [(Some(false), true), (Some(true), false)] {
        daemon.tell(
            ServerMessage::DaemonInfo(DaemonInfo { linger, ..info() }),
            cx,
        );
        assert_eq!(shown(cx, "about-linger"), notice, "{linger:?}");
    }

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "settings-page"));
    assert!(shown(cx, "sidebar"));
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    });
    cx.run_until_parked();
    assert!(shown(cx, "settings-theme"), "the page opens on General");
    click(cx, "close-settings");
    assert!(!shown(cx, "settings-page"));

    // The rail shows which page is open, and goes back to the bots.
    click(cx, "open-settings");
    click(cx, "open-bots");
    assert!(!shown(cx, "settings-page"));
    // The ring of a provider opens the page of that provider.
    click(cx, "rail-provider-claude");
    assert!(shown(cx, "provider-0-state"));
}

#[gpui::test]
fn a_preference_change_applies_at_once_and_follows_the_other_clients(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    daemon.sent();
    let theme = |cx: &mut VisualTestContext| cx.update(|_, cx| Theme::of(cx));

    choose(cx, "settings-theme", "settings-theme-2");
    assert_eq!(theme(cx), Theme::dark());
    assert!(!shown(cx, "settings-select-menu"));
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                appearance: Some(Appearance::Dark),
                ..PreferencesPatch::default()
            },
        }]
    );
    choose(cx, "settings-theme", "settings-theme-2");
    assert_eq!(daemon.sent(), [], "the same value is not sent again");
    choose(cx, "settings-theme", "settings-theme-1");
    assert_eq!(theme(cx), Theme::light());
    daemon.sent();

    choose(cx, "settings-send", "settings-send-1");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                send_shortcut: Some(SendShortcut::ModEnter),
                ..PreferencesPatch::default()
            },
        }]
    );
    assert_eq!(
        workspace.read_with(cx, |this, _| this.preferences.send_shortcut),
        SendShortcut::ModEnter
    );

    // A change made on another client shows here when the daemon tells it.
    daemon.tell(
        ServerMessage::Preferences(Preferences {
            appearance: Appearance::Dark,
            send_shortcut: SendShortcut::Enter,
            ..Preferences::default()
        }),
        cx,
    );
    assert_eq!(theme(cx), Theme::dark());
    assert_eq!(
        workspace.read_with(cx, |this, _| this.preferences.send_shortcut),
        SendShortcut::Enter
    );
    assert_eq!(daemon.sent(), []);
}

#[gpui::test]
fn a_select_opens_under_its_row_and_answers_to_the_keyboard(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    daemon.sent();
    let appearance =
        |cx: &mut VisualTestContext| workspace.read_with(cx, |this, _| this.preferences.appearance);
    assert_eq!(appearance(cx), Appearance::System);

    // The menu hangs under the select, on its trailing edge.
    click(cx, "settings-theme");
    let (select, menu) = (
        bounds(cx, "settings-theme"),
        bounds(cx, "settings-select-menu"),
    );
    assert!(menu.top() >= select.bottom(), "{menu:?} under {select:?}");
    assert_eq!(menu.right(), select.right());

    // A press outside closes the menu and changes nothing.
    click(cx, "settings-sections");
    assert!(!shown(cx, "settings-select-menu"));
    assert_eq!(daemon.sent(), []);
    assert!(shown(cx, "settings-page"));

    // Escape closes the menu first, and leaves the page open.
    click(cx, "settings-theme");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "settings-select-menu"));
    assert!(shown(cx, "settings-page"));

    // The arrows move through the values, and Enter takes the one they are on.
    click(cx, "settings-theme");
    cx.simulate_keystrokes("down down");
    press_enter(cx);
    assert!(!shown(cx, "settings-select-menu"));
    assert_eq!(appearance(cx), Appearance::Dark);
    press_enter(cx);
    cx.simulate_keystrokes("up up up");
    press_enter(cx);
    assert_eq!(appearance(cx), Appearance::System, "Up stops at the first");
}

#[gpui::test]
fn the_search_lists_the_settings_that_match_and_opens_the_one_picked(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    daemon.tell(
        ServerMessage::Providers(vec![provider(ProviderState::SignedOut)]),
        cx,
    );

    click(cx, "settings-search");
    cx.simulate_input("claude");
    cx.run_until_parked();
    assert!(
        !shown(cx, "settings-general"),
        "the results take the place of the sections"
    );
    assert!(shown(cx, "settings-result-0") && !shown(cx, "settings-result-1"));
    press_enter(cx);
    assert!(
        shown(cx, "provider-0-sign-in"),
        "the page of the provider opens"
    );
    assert!(
        shown(cx, "settings-providers"),
        "the search is cleared, and the sections are back"
    );

    click(cx, "settings-search");
    cx.simulate_input("zzz");
    cx.run_until_parked();
    assert!(shown(cx, "settings-no-results"));
    // Escape clears the search first, then closes the page.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(shown(cx, "settings-page") && shown(cx, "settings-general"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "settings-page"));

    // A result picked with the pointer opens its section.
    click(cx, "open-settings");
    click(cx, "settings-search");
    cx.simulate_input("database");
    cx.run_until_parked();
    click(cx, "settings-result-0");
    assert!(shown(cx, "about-database"));
}

#[gpui::test]
fn each_provider_shows_its_state_and_the_control_that_fixes_it(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    click(cx, "settings-providers");
    daemon.tell(
        ServerMessage::Providers(vec![ProviderStatus {
            checking: true,
            ..provider(ProviderState::Unknown)
        }]),
        cx,
    );
    daemon.sent();
    assert!(shown(cx, "provider-0"));
    click(cx, "provider-0");
    assert!(shown(cx, "provider-0-state"));
    assert!(!shown(cx, "provider-0-sign-in"));

    daemon.tell(
        ServerMessage::ProviderChanged(provider(ProviderState::SignedOut)),
        cx,
    );
    click(cx, "provider-0-sign-in");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SignIn {
            provider: "claude".to_owned(),
            method: None,
        }]
    );

    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            signing_in: true,
            ..provider(ProviderState::SignedOut)
        }),
        cx,
    );
    assert!(!shown(cx, "provider-0-sign-in"));
    assert!(shown(cx, "provider-0-cancel-sign-in"));

    // The end of the sign-in shows as soon as the daemon tells it.
    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            account: Some("Claude Ultra".to_owned()),
            ..provider(ProviderState::Ready)
        }),
        cx,
    );
    assert!(!shown(cx, "provider-0-sign-in"));
    assert!(!shown(cx, "provider-0-cancel-sign-in"));
    assert!(shown(cx, "provider-0-detail"));
    click(cx, "provider-0-check");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::CheckProvider {
            provider: "claude".to_owned()
        }]
    );

    daemon.tell(
        ServerMessage::ProviderChanged(ProviderStatus {
            can_sign_in: false,
            detail: Some("The program `bunx` was not found on this computer.".to_owned()),
            help_url: Some("https://bun.sh".to_owned()),
            ..provider(ProviderState::RuntimeMissing)
        }),
        cx,
    );
    assert!(shown(cx, "provider-0-get-runtime"));
    assert!(shown(cx, "provider-0-check"));
    assert!(shown(cx, "provider-0-detail"));
    assert!(!shown(cx, "provider-0-sign-in"));
}

#[gpui::test]
fn the_providers_are_listed_by_whether_they_can_run_a_bot(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    click(cx, "settings-providers");
    let named = |id: &str, name: &str, state| ProviderStatus {
        id: id.to_owned(),
        name: name.to_owned(),
        ..provider(state)
    };
    daemon.tell(
        ServerMessage::Providers(vec![
            named("claude", "Claude", ProviderState::SignedOut),
            named("codex", "ChatGPT", ProviderState::Ready),
            named("grok", "Grok", ProviderState::NotInstalled),
        ]),
        cx,
    );
    // The provider that is ready comes first, under its own heading.
    let rows = tops(cx, &["provider-1", "provider-0", "provider-2"]);
    assert!(rows.windows(2).all(|pair| pair[0] < pair[1]), "{rows:?}");
    let (first, second) = (bounds(cx, "provider-1"), bounds(cx, "provider-0"));
    assert!(
        second.top() - first.bottom() > px(24.),
        "the two groups stand apart"
    );
    assert_eq!(first.left(), second.left());
    assert_eq!(first.right(), second.right());
    assert_eq!(first.size.height, px(64.));

    // Every provider is ready: one group is left.
    daemon.tell(
        ServerMessage::Providers(vec![
            named("claude", "Claude", ProviderState::Ready),
            named("codex", "ChatGPT", ProviderState::Ready),
        ]),
        cx,
    );
    let (first, second) = (bounds(cx, "provider-0"), bounds(cx, "provider-1"));
    assert_eq!(second.top() - first.bottom(), px(1.), "one line between");
}

#[gpui::test]
fn a_narrow_window_keeps_every_control_of_the_settings_in_reach(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    cx.simulate_resize(size(layout::WINDOW_MIN_WIDTH, layout::WINDOW_MIN_HEIGHT));
    click(cx, "open-settings");
    daemon.tell(
        ServerMessage::Providers(vec![provider(ProviderState::SignedOut)]),
        cx,
    );
    let body = bounds(cx, "settings-body");
    for control in ["settings-theme", "settings-send"] {
        let select = bounds(cx, control);
        assert!(
            select.left() >= body.left() && select.right() <= body.right(),
            "{control} is cut: {select:?} in {body:?}"
        );
    }
    click(cx, "settings-providers");
    click(cx, "provider-0");
    let sign_in = bounds(cx, "provider-0-sign-in");
    assert!(sign_in.left() >= body.left() && sign_in.right() <= body.right());
}

#[gpui::test]
fn the_settings_page_keeps_its_measures(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    cx.simulate_resize(size(px(1186.), px(770.)));
    click(cx, "open-settings");
    let at = |x: f32, y: f32, width: f32, height: f32| Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    };
    // The title band: Back over the section column, then the section shown.
    assert_eq!(bounds(cx, "close-settings"), at(62., 5., 28., 28.));
    assert_eq!(bounds(cx, "settings-title").left(), px(340.));
    // The section column: 12 in from each side of the 280 of the sidebar,
    // the search 53 under the edge of the card, the sections 44 under it.
    assert_eq!(bounds(cx, "settings-search"), at(65., 91., 255., 32.));
    assert_eq!(bounds(cx, "settings-general"), at(65., 135., 255., 32.));
    assert_eq!(bounds(cx, "settings-providers").top(), px(171.));
    // The groups: a column of 720, centered, 51 under the edge of the card,
    // with rows of 64 and a select of 168 by 32, 16 in from the card.
    assert_eq!(bounds(cx, "settings-theme"), at(933., 105., 168., 32.));
    assert_eq!(bounds(cx, "settings-send").top(), px(105. + 64. + 50.));
    let body = bounds(cx, "settings-body");
    assert_eq!(
        bounds(cx, "settings-theme").right() + px(16. + 24. + 40.),
        body.right(),
        "the column is centered in {body:?}"
    );
    // A window wider than the column leaves the same room on each side of it.
    cx.simulate_resize(size(px(1600.), px(770.)));
    let (body, select) = (bounds(cx, "settings-body"), bounds(cx, "settings-theme"));
    let trailing = body.right() - select.right() - px(16.);
    assert_eq!(select.right() + px(16.) - px(720.) - body.left(), trailing);
}

#[gpui::test]
fn an_unreachable_daemon_disables_the_settings_and_says_why(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    daemon.tell(
        ServerMessage::Providers(vec![provider(ProviderState::SignedOut)]),
        cx,
    );
    assert!(!shown(cx, "connection-notice"));
    daemon.sent();
    let theme = cx.update(|_, cx| Theme::of(cx));

    daemon.link(
        ConnectionEvent::Disconnected("the pipe closed".to_owned()),
        cx,
    );
    assert!(shown(cx, "settings-page"));
    assert!(shown(cx, "connection-notice"));
    assert!(bounds(cx, "connection-notice").bottom() <= bounds(cx, "settings-theme").top());
    click(cx, "settings-theme");
    assert!(
        !shown(cx, "settings-select-menu"),
        "a select does not open while the daemon is away"
    );
    assert_eq!(cx.update(|_, cx| Theme::of(cx)), theme);
    assert_eq!(
        workspace.read_with(cx, |this, _| this.preferences.clone()),
        Preferences::default()
    );
    // The sections can still be read.
    click(cx, "settings-providers");
    click(cx, "provider-0");
    click(cx, "provider-0-sign-in");
    click(cx, "provider-0-check");
    assert_eq!(
        daemon.sent(),
        [],
        "nothing is sent to a daemon that is not there"
    );

    daemon.link(ConnectionEvent::Connected, cx);
    assert!(!shown(cx, "connection-notice"));
    let sent = daemon.sent();
    assert!(sent.contains(&ClientMessage::ListProviders), "{sent:?}");
    assert!(sent.contains(&ClientMessage::GetDaemonInfo), "{sent:?}");
}

// US-029: the search.

fn turn_events(first_id: i64, asked: &str, answered: &str) -> Vec<TimedEvent> {
    [
        Event::UserText(asked.to_owned()),
        Event::MessageText(answered.to_owned()),
        Event::TurnEnded(TurnEnd::Completed),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, event)| TimedEvent {
        id: first_id + index as i64,
        at_ms: NOON + (first_id + index as i64) * 1_000,
        event,
    })
    .collect()
}

fn results(request: u64, query: &str) -> SearchResults {
    SearchResults {
        request,
        query: query.to_owned(),
        bots: vec![2],
        conversations: vec![ConversationHit {
            conversation_id: 20,
            bot_id: 2,
            title: "Harbor hours".to_owned(),
            last_activity_ms: NOON,
        }],
        messages: vec![
            MessageHit {
                conversation_id: 10,
                bot_id: 1,
                event_id: 62,
                author: MessageAuthor::Bot,
                snippet: "The harbor opens at dawn.".to_owned(),
                at_ms: NOON,
            },
            MessageHit {
                conversation_id: 10,
                bot_id: 1,
                event_id: 8,
                author: MessageAuthor::Bot,
                snippet: "An old harbor story.".to_owned(),
                at_ms: NOON,
            },
        ],
        messages_searched: true,
    }
}

fn search_for(daemon: &mut Daemon, cx: &mut VisualTestContext, query: &str) -> u64 {
    cx.simulate_input(query);
    wait(cx, duration::SEARCH_DEBOUNCE);
    let sent = daemon.sent();
    let Some(ClientMessage::Search {
        request,
        query: asked,
    }) = sent.last()
    else {
        panic!("{sent:?}");
    };
    assert_eq!(asked, query);
    *request
}

fn with_chat(cx: &mut TestAppContext) -> (Entity<Workspace>, Daemon, &mut VisualTestContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "bot-1");
    let mut chat = timed_snapshot(
        10,
        1,
        (20..24)
            .flat_map(|turn| turn_events(turn * 3, "filler question", "filler answer"))
            .collect(),
    );
    chat.events.splice(
        1..2,
        [TimedEvent {
            id: 61,
            at_ms: NOON,
            event: Event::MessageText("The harbor opens at dawn.".to_owned()),
        }],
    );
    chat.older = Some(60);
    daemon.tell(ServerMessage::ConversationOpened(chat), cx);
    daemon.sent();
    (workspace, daemon, cx)
}

#[gpui::test]
fn the_search_keys_open_the_search_with_its_field_focused_and_close_it(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_chat(cx);
    assert!(!shown(cx, "search-palette"));

    search_keys(cx);
    assert!(shown(cx, "search-palette"));
    cx.simulate_input("har");
    assert_eq!(text_of(&workspace, cx, |this| &this.palette_query), "har");
    assert_eq!(text_of(&workspace, cx, |this| &this.composer), "");

    search_keys(cx);
    assert!(!shown(cx, "search-palette"));
    cx.simulate_input("back");
    assert_eq!(text_of(&workspace, cx, |this| &this.composer), "back");

    search_keys(cx);
    assert_eq!(
        text_of(&workspace, cx, |this| &this.palette_query),
        "",
        "the search starts empty"
    );
    assert!(shown(cx, "search-hint"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "search-palette"));

    search_keys(cx);
    let outside = bounds(cx, "search-palette").bottom_left() + point(px(-30.), px(60.));
    cx.simulate_click(outside, Modifiers::none());
    cx.run_until_parked();
    assert!(!shown(cx, "search-palette"));
    assert!(
        !daemon
            .sent()
            .iter()
            .any(|message| matches!(message, ClientMessage::Search { .. }))
    );
}

#[gpui::test]
fn results_show_bots_then_conversations_then_messages_and_a_typed_query_waits_for_a_pause(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = with_chat(cx);
    search_keys(cx);
    cx.simulate_input("har");
    wait(cx, duration::SEARCH_DEBOUNCE - Duration::from_millis(1));
    cx.simulate_input("bor");
    wait(cx, duration::SEARCH_DEBOUNCE - Duration::from_millis(1));
    assert_eq!(daemon.sent(), [], "the typing has not paused");
    wait(cx, Duration::from_millis(1));
    let sent = daemon.sent();
    let [ClientMessage::Search { request, query }] = &sent[..] else {
        panic!("{sent:?}");
    };
    assert_eq!(query, "harbor");
    assert!(shown(cx, "search-waiting"));

    // An answer to an earlier query is dropped.
    daemon.tell(
        ServerMessage::SearchResults(results(request - 1, "har")),
        cx,
    );
    assert!(!shown(cx, "search-result-0"));

    daemon.tell(
        ServerMessage::SearchResults(results(*request, "harbor")),
        cx,
    );
    assert!(!shown(cx, "search-waiting"));
    let order = tops(
        cx,
        &[
            "search-result-0",
            "search-result-1",
            "search-result-2",
            "search-result-3",
        ],
    );
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    assert!(!shown(cx, "search-result-4"));
    let hits = workspace.read_with(cx, |this, _| this.hits());
    assert_eq!(
        hits,
        [
            palette::Hit::Bot(2),
            palette::Hit::Conversation {
                bot_id: 2,
                conversation_id: 20
            },
            palette::Hit::Message {
                bot_id: 1,
                conversation_id: 10,
                event_id: 62
            },
            palette::Hit::Message {
                bot_id: 1,
                conversation_id: 10,
                event_id: 8
            },
        ]
    );

    // The arrows move through the results and wrap; Enter opens the active one.
    let active = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, _| this.palette.as_ref().unwrap().active)
    };
    assert_eq!(active(cx), 0);
    cx.simulate_keystrokes("up");
    assert_eq!(active(cx), 3);
    cx.simulate_keystrokes("down down");
    assert_eq!(active(cx), 1);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!shown(cx, "search-palette"));
    assert_eq!(
        workspace.read_with(cx, |this, _| (this.selected_bot, this.selected)),
        (Some(2), Some(20))
    );
    assert!(daemon.sent().contains(&ClientMessage::OpenConversation {
        conversation_id: 20
    }));

    // A click on a bot opens it.
    search_keys(cx);
    let request = search_for(&mut daemon, cx, "harbor");
    daemon.tell(ServerMessage::SearchResults(results(request, "harbor")), cx);
    click(cx, "search-result-0");
    assert!(!shown(cx, "search-palette"));
    assert_eq!(
        workspace.read_with(cx, |this, _| (this.selected_bot, this.selected)),
        (Some(2), Some(20))
    );
}

#[gpui::test]
fn a_query_with_no_match_says_so(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = with_chat(cx);
    search_keys(cx);
    let request = search_for(&mut daemon, cx, "zeppelin");
    assert!(!shown(cx, "search-empty"));
    daemon.tell(
        ServerMessage::SearchResults(SearchResults {
            request,
            query: "zeppelin".to_owned(),
            messages_searched: true,
            ..SearchResults::default()
        }),
        cx,
    );
    assert!(shown(cx, "search-empty"));
    assert!(!shown(cx, "search-result-0"));
    assert!(!shown(cx, "notice"));

    // Emptying the field goes back to the hint, with no request.
    select_all_and_delete(cx);
    wait(cx, duration::SEARCH_DEBOUNCE);
    assert!(shown(cx, "search-hint"));
    assert!(!shown(cx, "search-empty"));
    assert_eq!(daemon.sent(), []);
}

#[gpui::test]
fn the_search_palette_keeps_its_measures_and_fits_a_short_window(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_chat(cx);
    cx.simulate_resize(size(px(1080.), px(760.)));
    search_keys(cx);
    // 576 wide, centered, a tenth of the window under its top.
    let panel = bounds(cx, "search-palette");
    assert_eq!(
        (panel.left(), panel.top(), panel.size.width),
        (px(252.), px(76.), px(576.))
    );
    assert_eq!(bounds(cx, "search-hint").top(), px(76. + 1. + 48. + 6.));

    let request = search_for(&mut daemon, cx, "harbor");
    daemon.tell(ServerMessage::SearchResults(results(request, "harbor")), cx);
    // Rows 6 in from the edge of the panel, under the field of 48 and a
    // heading of 28; one line makes a row of 32, two lines a row of 48.
    assert_eq!(
        bounds(cx, "search-result-0"),
        Bounds {
            origin: point(px(259.), px(159.)),
            size: size(px(562.), px(32.)),
        }
    );
    assert_eq!(
        bounds(cx, "search-result-1").top(),
        px(159. + 32. + 6. + 28.),
        "groups stand 6 apart"
    );
    assert_eq!(bounds(cx, "search-result-2").size.height, px(48.));
    // The text of the field starts where the titles of the rows start.
    assert_eq!(bounds(cx, "search-query").left(), px(259. + 8. + 20. + 8.));
    let (panel, keys) = (bounds(cx, "search-palette"), bounds(cx, "search-keys"));
    assert_eq!(keys.size.height, px(41.));
    assert_eq!(keys.bottom() + px(1.), panel.bottom());

    // The pointer takes the highlight of the keys when it moves onto a row,
    // and gives it back to the keys while it stays still.
    let active = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, _| this.palette.as_ref().unwrap().active)
    };
    let onto = bounds(cx, "search-result-2").center();
    cx.simulate_mouse_move(onto, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(active(cx), 2);
    cx.simulate_keystrokes("up");
    cx.simulate_mouse_move(onto, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(active(cx), 1);

    // A window too short for the palette shrinks its list, which follows
    // the highlight; the key hints stay in the window.
    cx.simulate_resize(size(px(640.), px(420.)));
    let (panel, keys) = (bounds(cx, "search-palette"), bounds(cx, "search-keys"));
    assert_eq!(panel.top(), px(46.), "under the title band");
    assert_eq!(panel.bottom(), px(420. - 42.));
    assert_eq!(keys.bottom() + px(1.), panel.bottom());
    cx.simulate_keystrokes("down down");
    assert_eq!(active(cx), 3);
    let list = bounds(cx, "search-results");
    assert_eq!(
        bounds(cx, "search-result-3").bottom() + px(6.),
        list.bottom(),
        "the last result keeps the padding of the list under it"
    );
    assert!(
        bounds(cx, "search-result-0").top() < list.top() + px(6. + 28.),
        "the list scrolled"
    );
    cx.simulate_keystrokes("down");
    assert_eq!(active(cx), 0);
    assert_eq!(
        bounds(cx, "search-result-0").top(),
        list.top() + px(6. + 28.),
        "the first result brings its heading back"
    );
}

fn settle(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    for _ in 0..3 {
        workspace.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        shown(cx, "transcript");
    }
}

#[gpui::test]
fn a_message_result_opens_its_conversation_at_the_message_with_the_match_selected(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = with_chat(cx);
    click(cx, "bot-2");
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(20, 2, Vec::new())),
        cx,
    );
    search_keys(cx);
    let request = search_for(&mut daemon, cx, "HARBOR");
    daemon.tell(ServerMessage::SearchResults(results(request, "HARBOR")), cx);
    daemon.sent();

    click(cx, "search-result-2");
    assert_eq!(
        workspace.read_with(cx, |this, _| (this.selected_bot, this.selected)),
        (Some(1), Some(10))
    );
    assert!(daemon.sent().contains(&ClientMessage::OpenConversation {
        conversation_id: 10
    }));
    let mut chat = timed_snapshot(
        10,
        1,
        (20..40)
            .flat_map(|turn| turn_events(turn * 3, "filler question", "filler answer"))
            .collect(),
    );
    chat.events[2] = TimedEvent {
        id: 62,
        at_ms: NOON,
        event: Event::TurnEnded(TurnEnd::Completed),
    };
    chat.events[1] = TimedEvent {
        id: 61,
        at_ms: NOON,
        event: Event::MessageText("The harbor opens at dawn.".to_owned()),
    };
    chat.older = Some(60);
    daemon.tell(ServerMessage::ConversationOpened(chat), cx);
    settle(&workspace, cx);

    // The event 62 ends the turn of the answer stored as event 61: the row
    // of that answer is the one on screen, and its match is selected.
    assert!(shown(cx, "revealed-message"));
    assert_eq!(
        workspace.read_with(cx, |this, _| this.selected_text()),
        Some("harbor".to_owned())
    );
    let following = workspace.read_with(cx, |this, _| {
        this.open.as_ref().unwrap().list.is_following_tail()
    });
    assert!(!following, "the list stays on the message");
    assert!(
        !daemon
            .sent()
            .iter()
            .any(|message| matches!(message, ClientMessage::RevealEvent { .. }))
    );
}

#[gpui::test]
fn a_message_older_than_the_loaded_page_is_asked_for_then_shown(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_chat(cx);
    let time = Rc::new(Cell::new(Instant::now()));
    let clock = time.clone();
    workspace.update(cx, |this, _| this.clock = Rc::new(move || clock.get()));
    search_keys(cx);
    let request = search_for(&mut daemon, cx, "harbor");
    daemon.tell(ServerMessage::SearchResults(results(request, "harbor")), cx);
    daemon.sent();

    // The conversation is the open one: it loads again, so that every event
    // has its id, then the page that holds the message is asked for.
    click(cx, "search-result-3");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::OpenConversation {
            conversation_id: 10
        }]
    );
    let mut chat = timed_snapshot(
        10,
        1,
        (20..24)
            .flat_map(|turn| turn_events(turn * 3, "filler question", "filler answer"))
            .collect(),
    );
    chat.older = Some(60);
    daemon.tell(ServerMessage::ConversationOpened(chat), cx);
    let reveals: Vec<_> = daemon
        .sent()
        .into_iter()
        .filter(|message| matches!(message, ClientMessage::RevealEvent { .. }))
        .collect();
    assert_eq!(
        reveals,
        [ClientMessage::RevealEvent {
            conversation_id: 10,
            before: 60,
            event_id: 8,
        }]
    );
    assert!(workspace.read_with(cx, |this, _| this.open.as_ref().unwrap().loading_older));

    let mut older: Vec<TimedEvent> = (2..20)
        .flat_map(|turn| turn_events(turn * 3 + 1, "older question", "older answer"))
        .collect();
    older[1] = TimedEvent {
        id: 8,
        at_ms: NOON,
        event: Event::MessageText("An old harbor story.".to_owned()),
    };
    daemon.tell(
        ServerMessage::OlderEvents {
            conversation_id: 10,
            events: older,
            older: None,
        },
        cx,
    );
    settle(&workspace, cx);
    assert!(shown(cx, "revealed-message"));
    assert_eq!(
        workspace.read_with(cx, |this, _| this.selected_text()),
        Some("harbor".to_owned())
    );
    assert!(workspace.read_with(cx, |this, _| this.reveal.is_none()));

    // The highlight fades and the message stays.
    time.set(time.get() + duration::FLASH);
    settle(&workspace, cx);
    assert!(!shown(cx, "revealed-message"));
    assert_eq!(
        workspace.read_with(cx, |this, _| this.selected_text()),
        Some("harbor".to_owned())
    );
}

fn tail_chat() -> ConversationSnapshot {
    let mut chat = timed_snapshot(
        10,
        1,
        (20..24)
            .flat_map(|turn| turn_events(turn * 3, "filler question", "filler answer"))
            .collect(),
    );
    chat.older = Some(60);
    chat
}

/// Opens a message result that lies far before the loaded events: the daemon
/// answers with a part of the history around it.
fn with_far_hit(cx: &mut TestAppContext) -> (Entity<Workspace>, Daemon, &mut VisualTestContext) {
    let (workspace, mut daemon, cx) = with_chat(cx);
    search_keys(cx);
    let request = search_for(&mut daemon, cx, "harbor");
    daemon.tell(ServerMessage::SearchResults(results(request, "harbor")), cx);
    click(cx, "search-result-3");
    daemon.tell(ServerMessage::ConversationOpened(tail_chat()), cx);
    daemon.sent();
    let mut part: Vec<TimedEvent> = (2..9)
        .flat_map(|turn| turn_events(turn * 3 + 1, "older question", "older answer"))
        .collect();
    part[1] = TimedEvent {
        id: 8,
        at_ms: NOON,
        event: Event::MessageText("An old harbor story.".to_owned()),
    };
    daemon.tell(
        ServerMessage::EventsWindow {
            conversation_id: 10,
            events: part,
            older: Some(7),
            newer: Some(27),
        },
        cx,
    );
    settle(&workspace, cx);
    (workspace, daemon, cx)
}

fn questions(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
    workspace.read_with(cx, |this, _| {
        let rows = this.open.as_ref().unwrap().transcript.rows().iter();
        rows.filter_map(|row| match &row.kind {
            RowKind::User { text } => Some(text.to_string()),
            _ => None,
        })
        .collect()
    })
}

fn live_question(daemon: &mut Daemon, cx: &mut VisualTestContext) {
    daemon.tell(
        ServerMessage::ConversationEvent {
            conversation_id: 10,
            event: Event::UserText("live question".to_owned()),
            at_ms: NOON + 900_000,
        },
        cx,
    );
}

#[gpui::test]
fn a_message_far_from_the_loaded_events_shows_in_a_part_of_the_history(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_far_hit(cx);
    let detached = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, _| this.open.as_ref().unwrap().tail.is_some())
    };

    // The rows show the part around the message, with its match selected.
    assert!(detached(cx));
    assert_eq!(questions(&workspace, cx), ["older question"; 7]);
    assert!(shown(cx, "revealed-message"));
    assert_eq!(
        workspace.read_with(cx, |this, _| this.selected_text()),
        Some("harbor".to_owned())
    );
    assert!(shown(cx, "jump-to-latest"));

    // Its last row is on screen: the events that follow are asked for, once,
    // up to the first one that the live end holds.
    settle(&workspace, cx);
    let asked: Vec<_> = daemon
        .sent()
        .into_iter()
        .filter(|message| matches!(message, ClientMessage::LoadNewerEvents { .. }))
        .collect();
    assert_eq!(
        asked,
        [ClientMessage::LoadNewerEvents {
            conversation_id: 10,
            after: 27,
            until: 60,
        }]
    );

    // An event that arrives goes to the live end: the rows do not move, the
    // composer follows the turn, and the jump control counts the message.
    live_question(&mut daemon, cx);
    assert_eq!(questions(&workspace, cx), ["older question"; 7]);
    assert!(workspace.read_with(cx, |this, _| {
        this.open.as_ref().unwrap().live().turn_active()
    }));
    assert!(shown(cx, "jump-count"));
    let read = |message: &ClientMessage| {
        matches!(
            message,
            ClientMessage::MarkRead {
                following: true,
                ..
            }
        )
    };
    assert!(!daemon.sent().iter().any(read));

    // The page that reaches the live end joins both: every event shows once.
    daemon.tell(
        ServerMessage::NewerEvents {
            conversation_id: 10,
            events: (0..10)
                .flat_map(|turn| turn_events(28 + turn * 3, "between question", "between answer"))
                .collect(),
            newer: None,
        },
        cx,
    );
    settle(&workspace, cx);
    assert!(!detached(cx));
    let mut expected = vec!["older question"; 7];
    expected.extend(["between question"; 10]);
    expected.extend(["filler question"; 4]);
    expected.push("live question");
    assert_eq!(questions(&workspace, cx), expected);
    assert_eq!(
        workspace.read_with(cx, |this, _| this.open.as_ref().unwrap().transcript.older()),
        Some(7)
    );
    let asks_more =
        |message: &ClientMessage| matches!(message, ClientMessage::LoadNewerEvents { .. });
    assert!(!daemon.sent().iter().any(asks_more));
}

#[gpui::test]
fn jump_to_latest_leaves_the_part_of_the_history_for_the_live_end(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_far_hit(cx);
    live_question(&mut daemon, cx);
    daemon.sent();

    click(cx, "jump-to-latest");
    let mut expected = vec!["filler question"; 4];
    expected.push("live question");
    assert_eq!(questions(&workspace, cx), expected);
    let (detached, following, new) = workspace.read_with(cx, |this, _| {
        let open = this.open.as_ref().unwrap();
        (
            open.tail.is_some(),
            open.list.is_following_tail(),
            open.new_messages,
        )
    });
    assert!(!detached);
    assert!(following);
    assert_eq!(new, 0);
    assert!(!shown(cx, "revealed-message"));
    assert_eq!(
        daemon.sent(),
        [ClientMessage::MarkRead {
            conversation_id: 10,
            following: true,
        }]
    );

    // A page asked for before the jump has nowhere to go.
    daemon.tell(
        ServerMessage::NewerEvents {
            conversation_id: 10,
            events: turn_events(28, "between question", "between answer"),
            newer: None,
        },
        cx,
    );
    assert_eq!(questions(&workspace, cx), expected);
}

#[gpui::test]
fn later_messages_that_fail_to_load_are_reported_and_not_asked_again(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_far_hit(cx);
    daemon.sent();
    daemon.tell(
        ServerMessage::NewerEventsFailed {
            conversation_id: 10,
            message: "the database is locked".to_owned(),
        },
        cx,
    );
    settle(&workspace, cx);
    assert!(shown(cx, "notice"));
    assert_eq!(questions(&workspace, cx), ["older question"; 7]);
    assert!(shown(cx, "jump-to-latest"));
    assert_eq!(daemon.sent(), []);
}

#[gpui::test]
fn the_panel_sits_beside_the_conversation_when_the_window_has_room(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(vec![summary(1, "Atlas", BotState::Idle)], cx);
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(10, 1, Vec::new())),
        cx,
    );
    click(cx, "open-bot-settings");

    // In a narrow window the panel covers the conversation.
    let (send, panel) = (bounds(cx, "send-message"), bounds(cx, "side-panel"));
    assert!(send.right() > panel.left());
    assert_eq!(panel.size.width, layout::SIDE_PANEL);
    let margin = px(1000.) - panel.right();

    // With room for both, the conversation ends where the panel starts.
    cx.simulate_resize(size(px(1200.), px(700.)));
    let (send, panel) = (bounds(cx, "send-message"), bounds(cx, "side-panel"));
    assert!(send.right() <= panel.left());
    assert_eq!(panel.size.width, layout::SIDE_PANEL);
    assert_eq!(panel.right(), px(1200.) - margin);
    let room = panel.left() - bounds(cx, "sidebar").right();
    assert!(room >= px(layout::CONVERSATION_MIN), "{room:?}");

    // Closing the panel gives the room back.
    click(cx, "close-side-panel");
    assert!(bounds(cx, "send-message").right() > panel.left());
}

#[gpui::test]
fn sending_a_message_from_a_part_of_the_history_returns_to_the_live_end(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = with_far_hit(cx);
    daemon.sent();

    cx.simulate_input("hello");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = daemon.sent();
    assert!(sent.contains(&ClientMessage::SendMessage {
        conversation_id: 10,
        text: "hello".to_owned(),
    }));
    assert!(sent.contains(&ClientMessage::MarkRead {
        conversation_id: 10,
        following: true,
    }));
    assert_eq!(questions(&workspace, cx), ["filler question"; 4]);
    let (detached, following) = workspace.read_with(cx, |this, _| {
        let open = this.open.as_ref().unwrap();
        (open.tail.is_some(), open.list.is_following_tail())
    });
    assert!(!detached);
    assert!(following);
}
