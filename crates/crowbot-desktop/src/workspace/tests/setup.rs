//! Setup and settings: the creation form, the settings panel of a bot, the
//! settings dialog, and the search.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crowbot_protocol::{
    AVATAR_COLORS, BOT_INSTRUCTIONS_LENGTH, ConversationHit, DaemonInfo, MessageAuthor, MessageHit,
    PermissionLevel, ProviderState, ProviderStatus, SearchResults, SendShortcut,
};

use super::*;
use crate::composer::Composer;
use crate::theme::{Theme, duration};
use crate::transcript::RowKind;
use crate::workspace::bot_panel::SaveState;
use crate::workspace::creation::TEMPLATES;
use crate::workspace::forms::GLYPHS;

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

    assert!(shown(cx, "first-bot-heading"));
    assert!(!shown(cx, "cancel-creation"));
    assert!(!shown(cx, "send-message"));
    let pane = bounds(cx, "creation");
    assert!(pane.left() >= bounds(cx, "sidebar").right() - px(1.));
    for part in [
        "creation-avatar",
        "creation-color-0",
        "creation-color-7",
        "creation-glyph-0",
        "creation-glyph-11",
        "creation-name",
        "creation-purpose",
        "creation-model",
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
            "creation-glyph-0",
            "creation-name",
            "creation-purpose",
            "creation-model",
            "create-bot",
            "templates",
        ],
    );
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");

    // The name has the focus, and a suggestion fills the name and the purpose.
    cx.simulate_input("Atlas");
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
fn create_shows_a_creating_state_then_the_bot_opens_with_the_composer_focused(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(Vec::new(), cx);
    daemon.sent();

    cx.simulate_input("  Atlas ");
    click(cx, "creation-purpose");
    cx.simulate_input("Plan the week.");
    click(cx, "creation-model");
    cx.simulate_input("smart");
    click(cx, "creation-color-3");
    click(cx, "creation-glyph-2");
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
    assert_eq!(avatar.glyph, GLYPHS[1]);

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

    // The next form starts empty.
    click(cx, "new-bot");
    assert_eq!(text_of(&workspace, cx, |this| &this.creation_name), "");
    assert_eq!(text_of(&workspace, cx, |this| &this.creation_purpose), "");
}

#[gpui::test]
fn an_empty_name_shows_an_error_on_the_name_field_and_creates_nothing(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(Vec::new(), cx);
    daemon.sent();

    click(cx, "create-bot");
    assert!(shown(cx, "creation-name-error"));
    let (field, error) = (
        bounds(cx, "creation-name"),
        bounds(cx, "creation-name-error"),
    );
    assert!(error.top() >= field.bottom() && error.top() < field.bottom() + px(12.));
    cx.simulate_input("   ");
    assert!(!shown(cx, "creation-name-error"), "typing clears the error");
    press_enter(cx);
    assert!(shown(cx, "creation-name-error"));
    assert_eq!(daemon.sent(), []);
    assert!(workspace.read_with(cx, |this, _| this.bots.is_empty()));
    assert!(!shown(cx, "creating-bot"));

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
    daemon.connect(Vec::new(), cx);
    daemon.sent();
    assert!(!shown(cx, "creation-provider-notice"));
    let sign_in = || ClientMessage::SignIn {
        provider: "claude".to_owned(),
    };

    daemon.tell(
        ServerMessage::Providers(vec![provider(ProviderState::SignedOut)]),
        cx,
    );
    assert!(shown(cx, "creation-provider-notice"));
    assert!(bounds(cx, "creation-provider-notice").bottom() <= bounds(cx, "create-bot").top());
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
    assert_eq!(
        daemon.sent(),
        [ClientMessage::CheckProvider {
            provider: "claude".to_owned()
        }]
    );

    // Create stays available.
    click(cx, "creation-name");
    cx.simulate_input("Atlas");
    click(cx, "create-bot");
    assert!(matches!(
        &daemon.sent()[..],
        [ClientMessage::CreateBot { .. }]
    ));
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
    assert_eq!(daemon.sent(), [ClientMessage::ListProviders]);
    assert!(shown(cx, "creation"));
    assert!(shown(cx, "cancel-creation"));
    assert!(!shown(cx, "first-bot-heading"));
    for part in [
        "creation-name",
        "creation-purpose",
        "creation-model",
        "create-bot",
        "template-0",
    ] {
        assert!(shown(cx, part), "{part}");
    }
    assert!(!shown(cx, "send-message"));
    cx.simulate_input("Finch");
    assert_eq!(text_of(&workspace, cx, |this| &this.creation_name), "Finch");

    click(cx, "cancel-creation");
    assert!(!shown(cx, "creation"));
    assert!(shown(cx, "send-message"));

    click(cx, "new-bot");
    assert_eq!(text_of(&workspace, cx, |this| &this.creation_name), "");
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
    cx.simulate_input("Mine");
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

    // The avatar: a color and a picture.
    click(cx, "panel-avatar");
    click(cx, "panel-color-2");
    click(cx, "panel-glyph-3");
    let avatars: Vec<_> = updates(&daemon.sent())
        .into_iter()
        .filter_map(|patch| patch.avatar)
        .collect();
    assert_eq!(avatars.len(), 2);
    assert_eq!(avatars[0].color, AVATAR_COLORS[2]);
    assert_eq!(avatars[1].glyph, GLYPHS[2]);
    assert_eq!(bot(cx).avatar, avatars[1]);

    // The fields follow the bot on screen.
    click(cx, "bot-2");
    assert_eq!(text_of(&workspace, cx, |this| &this.panel_name), "Wren");
}

// US-028: the settings dialog.

fn info() -> DaemonInfo {
    DaemonInfo {
        daemon_version: "9.8.7".to_owned(),
        protocol_version: 6,
        database: "C:\\Users\\me\\.crowbot\\crowbot.db".to_owned(),
    }
}

#[gpui::test]
fn the_settings_dialog_has_general_providers_and_about(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    daemon.sent();
    assert!(!shown(cx, "settings-dialog"));

    click(cx, "open-settings");
    assert!(shown(cx, "settings-dialog"));
    assert_eq!(
        daemon.sent(),
        [ClientMessage::ListProviders, ClientMessage::GetDaemonInfo]
    );
    let sections = tops(
        cx,
        &["settings-general", "settings-providers", "settings-about"],
    );
    assert!(sections.windows(2).all(|pair| pair[0] < pair[1]));
    for control in [
        "appearance-system",
        "appearance-light",
        "appearance-dark",
        "send-enter",
        "send-mod-enter",
    ] {
        assert!(shown(cx, control), "{control}");
    }

    click(cx, "settings-providers");
    assert!(shown(cx, "providers-loading"));
    assert!(!shown(cx, "appearance-system"));
    daemon.tell(
        ServerMessage::Providers(vec![provider(ProviderState::Ready)]),
        cx,
    );
    assert!(shown(cx, "provider-0-state"));

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

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "settings-dialog"));
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    });
    cx.run_until_parked();
    assert!(
        shown(cx, "appearance-system"),
        "the dialog opens on General"
    );
    click(cx, "close-settings");
    assert!(!shown(cx, "settings-dialog"));
}

#[gpui::test]
fn a_preference_change_applies_at_once_and_follows_the_other_clients(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    daemon.sent();
    let theme = |cx: &mut VisualTestContext| cx.update(|_, cx| Theme::of(cx));

    click(cx, "appearance-dark");
    assert_eq!(theme(cx), Theme::dark());
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                appearance: Some(Appearance::Dark),
                ..PreferencesPatch::default()
            },
        }]
    );
    click(cx, "appearance-dark");
    assert_eq!(daemon.sent(), [], "the same value is not sent again");
    click(cx, "appearance-light");
    assert_eq!(theme(cx), Theme::light());
    daemon.sent();

    click(cx, "send-mod-enter");
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
    assert!(!shown(cx, "provider-0-sign-in"));

    daemon.tell(
        ServerMessage::ProviderChanged(provider(ProviderState::SignedOut)),
        cx,
    );
    click(cx, "provider-0-sign-in");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SignIn {
            provider: "claude".to_owned()
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
fn an_unreachable_daemon_disables_the_settings_and_says_why(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    click(cx, "open-settings");
    daemon.tell(
        ServerMessage::Providers(vec![provider(ProviderState::SignedOut)]),
        cx,
    );
    assert!(!shown(cx, "settings-notice"));
    daemon.sent();
    let theme = cx.update(|_, cx| Theme::of(cx));

    daemon.link(
        ConnectionEvent::Disconnected("the pipe closed".to_owned()),
        cx,
    );
    assert!(shown(cx, "settings-dialog"));
    assert!(shown(cx, "settings-notice"));
    assert!(bounds(cx, "settings-notice").top() < bounds(cx, "appearance-dark").top());
    click(cx, "appearance-dark");
    click(cx, "send-mod-enter");
    assert_eq!(cx.update(|_, cx| Theme::of(cx)), theme);
    assert_eq!(
        workspace.read_with(cx, |this, _| this.preferences.clone()),
        Preferences::default()
    );
    click(cx, "settings-providers");
    click(cx, "provider-0-sign-in");
    click(cx, "provider-0-check");
    assert_eq!(
        daemon.sent(),
        [],
        "nothing is sent to a daemon that is not there"
    );

    daemon.link(ConnectionEvent::Connected, cx);
    assert!(!shown(cx, "settings-notice"));
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
