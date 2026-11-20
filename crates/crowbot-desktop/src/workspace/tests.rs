#[cfg(target_os = "linux")]
mod chrome;
mod conversation;
mod providers;
mod setup;
#[cfg(target_os = "windows")]
mod windows;

use crowbot_protocol::{
    Appearance, BotPatch, BotState, BotSummary, ClientMessage, ConversationSnapshot,
    ConversationSummary, Event, Preferences, PreferencesPatch, QueueState, ServerMessage,
    TimedEvent, TurnEnd,
};
use gpui::{
    Bounds, Entity, KeyUpEvent, Keystroke, Modifiers, MouseButton, Pixels, TestAppContext,
    VisualTestContext, point, px, size,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::*;
use crate::sidebar::tests::summary;
use crate::theme::layout;
use crate::transcript::tests::NOON;

struct Daemon {
    events: UnboundedSender<ConnectionEvent>,
    sent: UnboundedReceiver<ClientMessage>,
}

impl Daemon {
    fn link(&self, event: ConnectionEvent, cx: &mut VisualTestContext) {
        self.events.send(event).unwrap();
        cx.run_until_parked();
    }

    fn tell(&self, message: ServerMessage, cx: &mut VisualTestContext) {
        self.link(ConnectionEvent::Message(Box::new(message)), cx);
    }

    fn sent(&mut self) -> Vec<ClientMessage> {
        std::iter::from_fn(|| self.sent.try_recv().ok()).collect()
    }

    fn connect(&mut self, bots: Vec<BotSummary>, cx: &mut VisualTestContext) {
        self.link(ConnectionEvent::Connected, cx);
        self.tell(ServerMessage::Preferences(Preferences::default()), cx);
        self.tell(ServerMessage::Bots(bots), cx);
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Workspace>, Daemon, &mut VisualTestContext) {
    cx.update(|cx| {
        crate::composer::bind_keys(cx);
        bind_keys(cx);
    });
    let (connection, sent) = Connection::for_test();
    let (events, received) = mpsc::unbounded_channel();
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(connection, received, window, cx));
    cx.simulate_resize(size(px(1000.), px(700.)));
    cx.run_until_parked();
    (workspace, Daemon { events, sent }, cx)
}

fn snapshot(id: i64, bot_id: i64, events: Vec<Event>) -> ConversationSnapshot {
    let timed = events
        .into_iter()
        .enumerate()
        .map(|(index, event)| TimedEvent {
            id: index as i64 + 1,
            at_ms: NOON + index as i64 * 1_000,
            event,
        });
    timed_snapshot(id, bot_id, timed.collect())
}

fn timed_snapshot(id: i64, bot_id: i64, events: Vec<TimedEvent>) -> ConversationSnapshot {
    ConversationSnapshot {
        conversation: ConversationSummary {
            id,
            bot_id,
            title: None,
            created_at_ms: 0,
            last_activity_ms: 0,
        },
        events,
        older: None,
        draft: String::new(),
        turn_active: false,
        pending_approvals: Vec::new(),
        queue: QueueState::default(),
        turn_model: None,
    }
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.run_until_parked();
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let center = bounds(cx, selector).center();
    cx.simulate_click(center, Modifiers::none());
    cx.run_until_parked();
}

/// Clicks the name of the creation form, which opens with no field focused,
/// and replaces what it holds with `text`.
fn type_name(cx: &mut VisualTestContext, text: &str) {
    click(cx, "creation-name");
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    cx.simulate_input(text);
}

fn press_enter(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").unwrap(),
    });
    cx.run_until_parked();
}

fn right_click(cx: &mut VisualTestContext, at: gpui::Point<Pixels>) {
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
}

fn names(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
    workspace.read_with(cx, |this, _| {
        this.bots
            .iter()
            .map(|summary| summary.bot.name.clone())
            .collect()
    })
}

fn two_bots() -> Vec<BotSummary> {
    vec![
        summary(1, "Atlas", BotState::Idle),
        summary(2, "Wren", BotState::Unread),
    ]
}

#[gpui::test]
fn the_loading_cover_stays_until_the_first_bot_list_arrives(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    let screen = |cx: &mut VisualTestContext| workspace.read_with(cx, |this, _| this.screen());
    assert_eq!(screen(cx), Screen::Loading);
    assert!(shown(cx, "loading"));

    daemon.link(ConnectionEvent::Starting, cx);
    assert_eq!(screen(cx), Screen::Loading);

    daemon.link(ConnectionEvent::Connected, cx);
    assert_eq!(
        daemon.sent(),
        [
            ClientMessage::GetPreferences,
            ClientMessage::ListBots,
            ClientMessage::ListProviders
        ]
    );
    assert_eq!(screen(cx), Screen::Loading);
    assert!(shown(cx, "loading"));
    assert!(!shown(cx, "sidebar"));

    daemon.tell(ServerMessage::Bots(Vec::new()), cx);
    assert_eq!(screen(cx), Screen::Shell);
    assert!(!shown(cx, "loading"));
    assert!(shown(cx, "sidebar"));
    assert!(shown(cx, "create-first-bot"));
}

#[gpui::test]
fn the_empty_list_opens_bot_creation(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(Vec::new(), cx);
    daemon.sent();

    assert!(shown(cx, "creation"));
    click(cx, "create-first-bot");
    assert_eq!(daemon.sent(), [], "no bot is made before the form is sent");
    assert!(shown(cx, "creation"));
    type_name(cx, "Atlas");
    workspace.read_with(cx, |this, cx| {
        assert_eq!(this.creation_name.read(cx).text(), "Atlas");
    });
}

#[gpui::test]
fn no_open_bot_means_no_header_controls(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(Vec::new(), cx);
    assert!(shown(cx, "creation-header"));
    assert!(!shown(cx, "conversation-header"));
    for control in [
        "open-bot-settings",
        "current-model",
        "toggle-history",
        "new-chat",
        "toggle-side-panel",
    ] {
        assert!(!shown(cx, control), "{control}");
    }

    daemon.tell(
        ServerMessage::BotChanged(summary(1, "Atlas", BotState::Idle)),
        cx,
    );
    for control in [
        "open-bot-settings",
        "current-model",
        "toggle-history",
        "new-chat",
        "toggle-side-panel",
    ] {
        assert!(shown(cx, control), "{control}");
    }
}

#[gpui::test]
fn a_dropped_link_leaves_the_panes_inert_and_a_reconnect_reloads_without_duplicates(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(vec![summary(1, "Atlas", BotState::Idle)], cx);
    let turn = vec![
        Event::UserText("hi".to_owned()),
        Event::MessageText("caw".to_owned()),
        Event::TurnEnded(TurnEnd::Completed),
    ];
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(10, 1, turn.clone())),
        cx,
    );
    let items = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, _| {
            this.open.as_ref().map(|open| open.transcript.rows().len())
        })
    };
    let before = items(cx);
    assert!(before.is_some_and(|count| count > 0));
    assert!(!shown(cx, "connection-notice"));
    daemon.sent();

    daemon.link(
        ConnectionEvent::Disconnected("the pipe closed".to_owned()),
        cx,
    );
    assert!(shown(cx, "connection-notice"));
    assert!(shown(cx, "inert"));
    assert!(shown(cx, "sidebar"));
    assert!(shown(cx, "bot-1"));
    assert_eq!(items(cx), before);
    let inert = bounds(cx, "inert");
    assert!(inert.contains(&bounds(cx, "bot-1").center()));
    assert!(inert.contains(&bounds(cx, "send-message").center()));
    click(cx, "new-bot");
    click(cx, "bot-1");
    cx.simulate_input("lost");
    cx.simulate_keystrokes("enter");
    assert_eq!(daemon.sent(), []);
    workspace.read_with(cx, |this, cx| {
        assert!(this.composer.read(cx).text().is_empty());
    });

    daemon.link(ConnectionEvent::Connected, cx);
    assert_eq!(
        daemon.sent(),
        [
            ClientMessage::GetPreferences,
            ClientMessage::ListBots,
            ClientMessage::OpenConversation {
                conversation_id: 10
            },
            ClientMessage::ListProviders,
        ]
    );
    daemon.tell(
        ServerMessage::Bots(vec![summary(1, "Atlas", BotState::Idle)]),
        cx,
    );
    daemon.tell(ServerMessage::ConversationOpened(snapshot(10, 1, turn)), cx);
    assert_eq!(items(cx), before);
    assert!(!shown(cx, "connection-notice"));
    assert!(!shown(cx, "inert"));
    daemon.sent();

    cx.simulate_input("back");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SendMessage {
            conversation_id: 10,
            text: "back".to_owned(),
        }]
    );
}

#[gpui::test]
fn a_daemon_of_another_version_shows_both_versions_and_is_not_retried(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.link(
        ConnectionEvent::Incompatible {
            daemon_protocol: 3,
            daemon_version: None,
        },
        cx,
    );
    assert_eq!(
        workspace.read_with(cx, |this, _| this.screen()),
        Screen::Incompatible {
            daemon_protocol: 3,
            daemon_version: None,
        }
    );
    assert!(shown(cx, "incompatible"));
    assert!(!shown(cx, "loading"));
    assert_eq!(daemon.sent(), []);

    click(cx, "retry-connection");
    assert_eq!(
        workspace.read_with(cx, |this, _| this.link.clone()),
        Link::Connecting
    );
    assert_eq!(daemon.sent(), []);

    daemon.connect(two_bots(), cx);
    daemon.link(
        ConnectionEvent::Incompatible {
            daemon_protocol: 9,
            daemon_version: Some("9.9.9".to_owned()),
        },
        cx,
    );
    assert!(shown(cx, "incompatible"));
    assert!(!shown(cx, "sidebar"));
}

#[gpui::test]
fn a_daemon_that_was_stopped_is_shown_as_stopped_with_a_retry(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(vec![summary(1, "Atlas", BotState::Idle)], cx);
    daemon.link(
        ConnectionEvent::Failed(crate::connection::STOPPED.to_owned()),
        cx,
    );
    assert!(shown(cx, "connection-notice"));
    assert!(shown(cx, "retry-connection"));
    assert!(shown(cx, "inert"));
    assert_eq!(
        workspace.read_with(cx, |this, _| this.link.clone()),
        Link::Failed(crate::connection::STOPPED.into())
    );
    daemon.sent();
    click(cx, "retry-connection");
    assert!(workspace.read_with(cx, |this, _| this.link
        != Link::Failed(crate::connection::STOPPED.into())));
}

#[gpui::test]
fn a_daemon_that_cannot_be_reached_at_start_offers_a_retry(cx: &mut TestAppContext) {
    let (workspace, daemon, cx) = open(cx);
    daemon.link(ConnectionEvent::Failed("no daemon answers".to_owned()), cx);
    assert_eq!(
        workspace.read_with(cx, |this, _| this.screen()),
        Screen::Unreachable("no daemon answers".into())
    );
    assert!(shown(cx, "unreachable"));

    click(cx, "retry-connection");
    assert_eq!(
        workspace.read_with(cx, |this, _| this.screen()),
        Screen::Loading
    );
}

#[gpui::test]
fn a_conversation_that_fails_to_load_shows_the_reason_and_a_retry(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(vec![summary(1, "Atlas", BotState::Idle)], cx);
    assert_eq!(
        daemon.sent().last(),
        Some(&ClientMessage::OpenConversation {
            conversation_id: 10
        })
    );
    assert!(shown(cx, "conversation-loading"));

    daemon.tell(
        ServerMessage::Error {
            message: "the conversation is damaged".to_owned(),
        },
        cx,
    );
    assert_eq!(
        workspace.read_with(cx, |this, _| this.load_error.clone()),
        Some("the conversation is damaged".into())
    );
    assert!(shown(cx, "conversation-error"));
    assert!(!shown(cx, "notice"));
    assert!(!shown(cx, "send-message"));

    click(cx, "retry-load");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::OpenConversation {
            conversation_id: 10
        }]
    );
    assert!(shown(cx, "conversation-loading"));

    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(10, 1, Vec::new())),
        cx,
    );
    assert!(!shown(cx, "conversation-error"));
    assert!(shown(cx, "send-message"));
}

#[gpui::test]
fn a_right_click_on_a_bot_opens_its_menu_and_a_refused_change_is_undone(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    daemon.sent();

    let row = bounds(cx, "bot-1").center();
    right_click(cx, row);
    let entries = workspace.read_with(cx, |this, _| this.menu_entries());
    let labels: Vec<_> = entries.iter().map(|entry| entry.label).collect();
    assert_eq!(
        labels,
        [
            "Mark unread",
            "New chat",
            "Edit",
            "Duplicate",
            "Pin",
            "Hide",
            "Delete"
        ]
    );
    assert!(shown(cx, "context-menu"));

    click(cx, "menu-item-4");
    assert!(!shown(cx, "context-menu"));
    assert_eq!(
        daemon.sent(),
        [ClientMessage::UpdateBot {
            bot_id: 1,
            patch: BotPatch {
                pinned: Some(true),
                ..BotPatch::default()
            },
        }]
    );
    assert!(workspace.read_with(cx, |this, _| this.bots[0].bot.pinned));

    daemon.tell(
        ServerMessage::Error {
            message: "the database is locked".to_owned(),
        },
        cx,
    );
    workspace.read_with(cx, |this, _| {
        assert!(!this.bots[0].bot.pinned);
        assert_eq!(
            this.notice,
            Some("The change was not saved: the database is locked".into())
        );
    });
    assert!(shown(cx, "notice"));
    click(cx, "dismiss-notice");
    assert!(!shown(cx, "notice"));
}

#[gpui::test]
fn the_menu_keys_open_the_same_menu_on_a_focused_row(cx: &mut TestAppContext) {
    for keys in ["shift-f10", "menu"] {
        let (workspace, mut daemon, cx) = open(cx);
        daemon.connect(two_bots(), cx);
        daemon.sent();

        let mut opened = false;
        for _ in 0..24 {
            cx.simulate_keystrokes("tab");
            cx.simulate_keystrokes(keys);
            let target = workspace.read_with(cx, |this, _| {
                this.menu.as_ref().map(|menu| (menu.target, menu.position))
            });
            if let Some(target) = target {
                assert_eq!(target, (MenuTarget::Bot(2), None), "{keys}");
                opened = true;
                break;
            }
        }
        assert!(opened, "{keys} opened no menu");
        assert!(shown(cx, "context-menu"));
        let labels: Vec<_> = workspace.read_with(cx, |this, _| {
            this.menu_entries()
                .iter()
                .map(|entry| entry.label)
                .collect()
        });
        assert_eq!(labels[0], "Mark read");
        assert_eq!(labels.len(), 7);

        cx.simulate_keystrokes("enter");
        assert_eq!(daemon.sent(), [ClientMessage::MarkBotRead { bot_id: 2 }]);
        workspace.read_with(cx, |this, _| {
            assert!(this.menu.is_none());
            assert_eq!(this.bots[1].state, BotState::Idle);
        });
    }
}

#[gpui::test]
fn escape_closes_the_menu_and_changes_nothing(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    daemon.sent();
    let row = bounds(cx, "bot-2").center();
    right_click(cx, row);
    assert!(shown(cx, "context-menu"));

    cx.simulate_keystrokes("down down escape");
    assert!(!shown(cx, "context-menu"));
    assert_eq!(daemon.sent(), []);
    assert_eq!(names(&workspace, cx), ["Atlas", "Wren"]);
}

#[gpui::test]
fn the_empty_area_menu_creates_a_bot_and_marks_all_read(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(vec![summary(1, "Atlas", BotState::Idle)], cx);
    daemon.sent();
    let list = bounds(cx, "bots");
    let empty = point(list.center().x, list.bottom() - px(20.));

    right_click(cx, empty);
    let entries = workspace.read_with(cx, |this, _| this.menu_entries());
    assert_eq!(entries[0].label, "New bot");
    assert_eq!(entries[1].label, "Mark all read");
    assert!(!entries[1].enabled);
    click(cx, "menu-item-1");
    assert_eq!(daemon.sent(), []);
    cx.simulate_keystrokes("escape");

    daemon.tell(
        ServerMessage::BotChanged(summary(2, "Wren", BotState::Unread)),
        cx,
    );
    daemon.tell(
        ServerMessage::BotChanged(summary(3, "Finch", BotState::Failed)),
        cx,
    );
    right_click(cx, empty);
    assert!(workspace.read_with(cx, |this, _| this.menu_entries()[1].enabled));
    click(cx, "menu-item-1");
    let mut sent = daemon.sent();
    sent.sort_by_key(|message| format!("{message:?}"));
    assert_eq!(
        sent,
        [
            ClientMessage::MarkBotRead { bot_id: 2 },
            ClientMessage::MarkBotRead { bot_id: 3 },
        ]
    );

    right_click(cx, empty);
    click(cx, "menu-item-0");
    // The providers were asked for at the connection, for the rail.
    assert_eq!(daemon.sent(), []);
    assert!(shown(cx, "creation"));
    assert!(shown(cx, "cancel-creation"));
}

#[gpui::test]
fn delete_asks_first_and_a_refused_delete_puts_the_bot_back(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    daemon.sent();

    let row = bounds(cx, "bot-1").center();
    right_click(cx, row);
    click(cx, "menu-item-6");
    assert!(shown(cx, "delete-dialog"));
    assert_eq!(daemon.sent(), []);
    assert_eq!(names(&workspace, cx), ["Atlas", "Wren"]);

    cx.simulate_keystrokes("escape");
    assert!(!shown(cx, "delete-dialog"));
    assert_eq!(daemon.sent(), []);

    right_click(cx, row);
    click(cx, "menu-item-6");
    cx.simulate_keystrokes("tab");
    press_enter(cx);
    assert!(!shown(cx, "delete-dialog"));
    assert!(
        daemon
            .sent()
            .contains(&ClientMessage::DeleteBot { bot_id: 1 })
    );
    assert_eq!(names(&workspace, cx), ["Wren"]);

    daemon.tell(
        ServerMessage::Error {
            message: "the bot is busy".to_owned(),
        },
        cx,
    );
    assert_eq!(names(&workspace, cx), ["Atlas", "Wren"]);
    assert!(shown(cx, "notice"));

    right_click(cx, row);
    click(cx, "menu-item-6");
    click(cx, "confirm-delete");
    daemon.tell(ServerMessage::BotDeleted { bot_id: 1 }, cx);
    assert_eq!(names(&workspace, cx), ["Wren"]);
    assert!(!shown(cx, "bot-1"));
}

#[gpui::test]
fn a_state_change_moves_the_bot_to_its_group(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(
        vec![
            summary(1, "Atlas", BotState::Idle),
            summary(2, "Wren", BotState::Idle),
        ],
        cx,
    );
    let atlas = bounds(cx, "bot-1").top();
    let wren = bounds(cx, "bot-2").top();
    assert!(atlas < wren);

    daemon.tell(
        ServerMessage::BotChanged(summary(2, "Wren", BotState::Waiting)),
        cx,
    );
    assert!(bounds(cx, "bot-2").top() < bounds(cx, "bot-1").top());

    daemon.tell(
        ServerMessage::BotChanged(summary(2, "Wren", BotState::Idle)),
        cx,
    );
    assert!(bounds(cx, "bot-1").top() < bounds(cx, "bot-2").top());
}

#[gpui::test]
fn search_and_new_bot_sit_side_by_side_over_the_list(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(two_bots(), cx);
    // Search sits on the leading edge of the sidebar and the new bot on the
    // trailing edge, both above the list.
    let (search, create) = (bounds(cx, "open-search"), bounds(cx, "new-bot"));
    assert!(search.right() < create.left());
    assert_eq!(search.size, create.size);
    assert!(create.bottom() <= bounds(cx, "bot-1").top());
    // Their edges line up with the edges of the rows under them.
    let row = bounds(cx, "bot-1");
    assert_eq!(search.left(), row.left());
    assert_eq!(create.right(), row.right());

    click(cx, "open-search");
    assert!(shown(cx, "search-palette"));
    cx.simulate_keystrokes("escape");
    click(cx, "new-bot");
    assert!(shown(cx, "creation"));
}

#[gpui::test]
fn the_badge_names_the_model_of_the_next_turn_while_a_turn_runs(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    let mut atlas = summary(1, "Atlas", BotState::Working);
    atlas.bot.model = Some("sonnet".to_owned());
    daemon.connect(vec![atlas.clone()], cx);
    let mut running = snapshot(10, 1, vec![Event::UserText("go".to_owned())]);
    running.turn_active = true;
    running.turn_model = Some(Some("sonnet".to_owned()));
    daemon.tell(ServerMessage::ConversationOpened(running), cx);
    assert!(!shown(cx, "pending-model"));
    assert!(shown(cx, "current-model"));

    atlas.bot.model = Some("opus".to_owned());
    daemon.tell(ServerMessage::BotChanged(atlas.clone()), cx);
    assert_eq!(
        workspace.read_with(cx, |this, _| this.pending_model()),
        Some("opus".into())
    );
    assert!(shown(cx, "pending-model"));

    daemon.tell(
        ServerMessage::ConversationEvent {
            conversation_id: 10,
            event: Event::TurnEnded(TurnEnd::Completed),
            at_ms: NOON,
        },
        cx,
    );
    assert!(!shown(cx, "pending-model"));

    daemon.tell(
        ServerMessage::TurnModel {
            conversation_id: 10,
            model: Some("opus".to_owned()),
        },
        cx,
    );
    assert_eq!(
        workspace.read_with(cx, |this, _| this.pending_model()),
        None
    );
}

#[gpui::test]
fn the_header_opens_the_bot_settings_in_the_side_panel(cx: &mut TestAppContext) {
    let (_workspace, mut daemon, cx) = open(cx);
    daemon.connect(vec![summary(1, "Atlas", BotState::Idle)], cx);
    assert!(!shown(cx, "side-panel"));

    click(cx, "open-bot-settings");
    assert!(shown(cx, "side-panel"));
    click(cx, "toggle-side-panel");
    assert!(!shown(cx, "side-panel"));
    click(cx, "toggle-side-panel");
    let panel = bounds(cx, "side-panel");
    assert_eq!(panel.size.width, layout::SIDE_PANEL);
    click(cx, "close-side-panel");
    assert!(!shown(cx, "side-panel"));
}

#[gpui::test]
fn the_sidebar_is_restored_from_the_preferences_and_saved_when_it_changes(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    daemon.link(ConnectionEvent::Connected, cx);
    daemon.tell(ServerMessage::Bots(two_bots()), cx);
    assert_eq!(
        bounds(cx, "sidebar").size.width,
        px(layout::SIDEBAR_DEFAULT)
    );

    daemon.tell(
        ServerMessage::Preferences(Preferences {
            appearance: Appearance::Light,
            sidebar_width: Some(320),
            sidebar_collapsed: false,
            ..Preferences::default()
        }),
        cx,
    );
    assert_eq!(bounds(cx, "sidebar").size.width, px(320.));
    cx.update(|_, cx| assert_eq!(Theme::of(cx), Theme::light()));

    daemon.tell(
        ServerMessage::Preferences(Preferences {
            appearance: Appearance::Dark,
            sidebar_width: Some(320),
            sidebar_collapsed: true,
            ..Preferences::default()
        }),
        cx,
    );
    assert_eq!(
        bounds(cx, "sidebar").size.width,
        px(layout::SIDEBAR_COMPACT)
    );
    cx.update(|_, cx| assert_eq!(Theme::of(cx), Theme::dark()));
    daemon.sent();

    click(cx, "toggle-sidebar");
    assert_eq!(bounds(cx, "sidebar").size.width, px(320.));
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                sidebar_collapsed: Some(false),
                ..PreferencesPatch::default()
            },
        }]
    );

    let handle = bounds(cx, "sidebar-resizer").center();
    cx.simulate_mouse_down(handle, MouseButton::Left, Modifiers::none());
    for x in [330., 360., 250.] {
        cx.simulate_mouse_move(point(px(x), handle.y), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    }
    assert_eq!(bounds(cx, "sidebar").size.width, px(250.));
    assert_eq!(daemon.sent(), []);
    cx.simulate_mouse_move(
        point(px(100.), handle.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(
        bounds(cx, "sidebar").size.width,
        px(layout::SIDEBAR_COMPACT)
    );
    cx.simulate_mouse_move(
        point(px(119.), handle.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(bounds(cx, "sidebar").size.width, px(layout::SIDEBAR_MIN));
    cx.simulate_mouse_move(
        point(px(900.), handle.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(bounds(cx, "sidebar").size.width, px(layout::SIDEBAR_MAX));
    cx.simulate_mouse_up(
        point(px(900.), handle.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    assert_eq!(
        daemon.sent(),
        [ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                sidebar_width: Some(400),
                ..PreferencesPatch::default()
            },
        }]
    );
    assert!(!workspace.read_with(cx, |this, _| this.resizing));

    cx.simulate_resize(size(px(700.), px(700.)));
    let narrow = bounds(cx, "sidebar").size.width;
    assert_eq!(narrow, px(700. - layout::CONVERSATION_MIN));
    assert_eq!(daemon.sent(), []);
}

#[gpui::test]
fn unreadable_preferences_give_the_defaults_with_no_error(cx: &mut TestAppContext) {
    let (workspace, daemon, cx) = open(cx);
    daemon.link(ConnectionEvent::Connected, cx);
    let unreadable: ServerMessage = serde_json::from_str(
        r#"{"type":"preferences","data":{"appearance":"sepia","sidebar_width":90000}}"#,
    )
    .unwrap();
    daemon.tell(unreadable, cx);
    daemon.tell(ServerMessage::Bots(two_bots()), cx);
    assert_eq!(
        bounds(cx, "sidebar").size.width,
        px(layout::SIDEBAR_DEFAULT)
    );
    workspace.read_with(cx, |this, _| {
        assert_eq!(this.preferences.appearance, Appearance::System);
        assert_eq!(this.notice, None);
    });
    assert!(!shown(cx, "notice"));
}
