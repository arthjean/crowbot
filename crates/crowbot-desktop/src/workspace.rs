use std::cell::Cell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crowbot_protocol::{
    ApprovalDecision, ApprovalReason, BotDraft, BotState, BotSummary, ClientMessage,
    ConversationSummary, Event, QueueState, ServerMessage, ToolStatus,
};
use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FocusHandle, Focusable, FollowMode,
    FontWeight, KeyBinding, ListAlignment, ListState, Role, SharedString, Stateful, Subscription,
    Task, Text, Window, actions, div, list, prelude::*, px,
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::bots;
use crate::composer::{Composer, ComposerEvent};
use crate::connection::{Connection, ConnectionEvent};
use crate::theme::Theme;
use crate::transcript::{ApprovalState, Item, Transcript};

const COLUMN_WIDTH: f32 = 760.;
const SIDEBAR_WIDTH: f32 = 260.;

actions!(workspace, [FocusNext, FocusPrevious]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
    ]);
}

enum Status {
    Connecting,
    /// No daemon was listening, so the client is starting one.
    Starting,
    Connected,
    /// The connection dropped and the client reconnects on its own.
    Disconnected(SharedString),
    /// The client cannot attach until the user retries.
    Failed(SharedString),
}

struct OpenConversation {
    id: i64,
    title: Option<String>,
    transcript: Transcript,
    list: ListState,
    queue: QueueState,
    /// Whether the list showed its end at the last scroll, as the daemon was told.
    at_end: Rc<Cell<bool>>,
}

pub struct Workspace {
    connection: Connection,
    status: Status,
    bots: Vec<BotSummary>,
    selected_bot: Option<i64>,
    /// The conversation the pane shows: the bot's current one, or one picked
    /// from its history.
    selected: Option<i64>,
    open: Option<OpenConversation>,
    /// The conversations of the selected bot, while the pane shows its history.
    history: Option<Vec<ConversationSummary>>,
    composer: Entity<Composer>,
    /// The queued message that the composer is editing.
    editing: Option<i64>,
    notice: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _events: Task<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonStyle {
    Solid,
    Quiet,
    Disabled,
}

impl Workspace {
    pub fn new(
        connection: Connection,
        mut events: UnboundedReceiver<ConnectionEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| Composer::new("Message the bot", cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event: &ComposerEvent, window, cx| match event {
                    ComposerEvent::Submit => this.send_message(window, cx),
                },
            ),
            cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
        ];
        let events = cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.handle_connection(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        window.focus(&composer.focus_handle(cx), cx);
        Self {
            connection,
            status: Status::Connecting,
            bots: Vec::new(),
            selected_bot: None,
            selected: None,
            open: None,
            history: None,
            composer,
            editing: None,
            notice: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
            _events: events,
        }
    }

    fn handle_connection(&mut self, event: ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::Connected => {
                self.status = Status::Connected;
                self.notice = None;
                self.connection.send(ClientMessage::ListBots);
                if let Some(conversation_id) = self.selected {
                    self.connection
                        .send(ClientMessage::OpenConversation { conversation_id });
                }
            }
            ConnectionEvent::Starting => self.status = Status::Starting,
            ConnectionEvent::Disconnected(reason) => {
                self.status = Status::Disconnected(reason.into());
            }
            ConnectionEvent::Failed(reason) => self.status = Status::Failed(reason.into()),
            ConnectionEvent::Message(message) => self.handle_message(*message),
        }
        cx.notify();
    }

    fn handle_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Welcome { .. } => {}
            ServerMessage::Bots(bots) => {
                self.bots = bots;
                if self.selected_bot().is_none() {
                    self.forget_selection();
                    self.select_first_bot();
                }
            }
            ServerMessage::BotChanged(summary) => {
                let id = summary.bot.id;
                match self.bots.iter_mut().find(|known| known.bot.id == id) {
                    Some(known) => *known = summary,
                    None => self.bots.push(summary),
                }
                if self.selected_bot.is_none() {
                    self.select_bot(id);
                }
            }
            // This client made the bot: it opens it.
            ServerMessage::BotCreated { bot_id } => self.select_bot(bot_id),
            ServerMessage::BotDeleted { bot_id } => {
                self.bots.retain(|known| known.bot.id != bot_id);
                if self.selected_bot == Some(bot_id) {
                    self.forget_selection();
                    self.select_first_bot();
                }
            }
            ServerMessage::Conversations {
                bot_id,
                conversations,
            } => {
                if self.selected_bot == Some(bot_id) && self.history.is_some() {
                    self.history = Some(conversations);
                }
            }
            // This client started the conversation: it shows it.
            ServerMessage::ConversationCreated(summary) => {
                if self.selected_bot == Some(summary.bot_id) {
                    self.history = None;
                    self.select(summary.id);
                }
            }
            ServerMessage::ConversationChanged(summary) => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == summary.id) {
                    open.title.clone_from(&summary.title);
                }
                if self.selected_bot == Some(summary.bot_id)
                    && let Some(history) = self.history.as_mut()
                {
                    match history.iter_mut().find(|known| known.id == summary.id) {
                        Some(known) => *known = summary,
                        None => history.insert(0, summary),
                    }
                }
            }
            ServerMessage::ConversationOpened(snapshot) => {
                let id = snapshot.conversation.id;
                if self.selected != Some(id) {
                    return;
                }
                let title = snapshot.conversation.title.clone();
                let queue = snapshot.queue.clone();
                let transcript = Transcript::from_snapshot(snapshot);
                let list = ListState::new(transcript.items().len(), ListAlignment::Top, px(600.));
                list.set_follow_mode(FollowMode::Tail);
                // The daemon learns when the list leaves its end and when it
                // comes back: it reads what arrives only while the end shows.
                let at_end = Rc::new(Cell::new(true));
                let (connection, told) = (self.connection.clone(), at_end.clone());
                list.set_scroll_handler(move |scroll, _, _| {
                    let following = scroll.is_following_tail;
                    if told.replace(following) != following {
                        connection.send(ClientMessage::MarkRead {
                            conversation_id: id,
                            following,
                        });
                    }
                });
                self.open = Some(OpenConversation {
                    id,
                    title,
                    transcript,
                    list,
                    queue,
                    at_end,
                });
                // The conversation opens at its end: all of it is shown,
                // unless the history covers it.
                self.conversation_shown();
                self.forget_started_edit();
            }
            ServerMessage::QueueChanged {
                conversation_id,
                queue,
            } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) {
                    open.queue = queue;
                    self.forget_started_edit();
                }
            }
            ServerMessage::ConversationEvent {
                conversation_id,
                event,
            } => self.apply(conversation_id, event),
            ServerMessage::Error { message } | ServerMessage::InvalidField { message, .. } => {
                self.notice = Some(message.into());
            }
        }
    }

    fn selected_bot(&self) -> Option<&BotSummary> {
        let id = self.selected_bot?;
        self.bots.iter().find(|known| known.bot.id == id)
    }

    /// Drops what the pane shows without telling the daemon: the bot is gone.
    fn forget_selection(&mut self) {
        self.selected_bot = None;
        self.selected = None;
        self.open = None;
        self.history = None;
        self.editing = None;
    }

    fn select_first_bot(&mut self) {
        if let Some(first) = self.bots.first().map(|known| known.bot.id) {
            self.select_bot(first);
        }
    }

    /// Opens a bot on its current conversation.
    fn select_bot(&mut self, bot_id: i64) {
        let Some(current) = self
            .bots
            .iter()
            .find(|known| known.bot.id == bot_id)
            .map(|known| known.current_conversation_id)
        else {
            return;
        };
        self.selected_bot = Some(bot_id);
        self.history = None;
        self.select(current);
    }

    fn apply(&mut self, conversation_id: i64, event: Event) {
        let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) else {
            return;
        };
        let marks_read = bots::marks_read(&event);
        let before = open.transcript.items().len();
        let first_changed = open.transcript.apply(event);
        let after = open.transcript.items().len();
        if first_changed < before {
            open.list.remeasure_items(first_changed..before);
        }
        if after > before {
            open.list.splice(before..before, after - before);
        }
        if !marks_read {
            return;
        }
        // The list shows the conversation at its end: what arrived is read.
        if open.at_end.get() && self.history.is_none() {
            self.connection.send(ClientMessage::MarkRead {
                conversation_id,
                following: true,
            });
        }
    }

    /// A queued message that started, or was deleted elsewhere, can no longer
    /// be edited: the text in the composer then goes out as a new message.
    fn forget_started_edit(&mut self) {
        let queued = |id| {
            self.open
                .as_ref()
                .is_some_and(|open| open.queue.messages.iter().any(|message| message.id == id))
        };
        if self.editing.is_some_and(|id| !queued(id)) {
            self.editing = None;
        }
    }

    fn select(&mut self, conversation_id: i64) {
        if self.selected == Some(conversation_id) {
            return;
        }
        if let Some(previous) = self.selected {
            self.connection.send(ClientMessage::CloseConversation {
                conversation_id: previous,
            });
        }
        self.selected = Some(conversation_id);
        self.open = None;
        self.editing = None;
        self.notice = None;
        self.connection
            .send(ClientMessage::OpenConversation { conversation_id });
    }

    fn can_send(&self, cx: &App) -> bool {
        matches!(self.status, Status::Connected)
            && self.open.is_some()
            && self.history.is_none()
            && !self.composer.read(cx).text().trim().is_empty()
    }

    /// The bot works and the composer is empty: the send control stops the turn.
    fn can_stop(&self, cx: &App) -> bool {
        matches!(self.status, Status::Connected)
            && self
                .open
                .as_ref()
                .is_some_and(|open| open.transcript.turn_active())
            && self.composer.read(cx).text().trim().is_empty()
    }

    fn stop_turn(&mut self) {
        if let Some(open) = self.open.as_ref() {
            self.connection.send(ClientMessage::StopTurn {
                conversation_id: open.id,
            });
        }
    }

    fn edit_queued(&mut self, queue_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.open.as_ref().and_then(|open| {
            let queued = open.queue.messages.iter();
            let message = queued.into_iter().find(|message| message.id == queue_id)?;
            Some(message.text.clone())
        }) else {
            return;
        };
        self.editing = Some(queue_id);
        self.composer
            .update(cx, |composer, cx| composer.set_text(&text, window, cx));
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.composer
            .update(cx, |composer, cx| composer.clear(window, cx));
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn queue_action(&mut self, action: impl FnOnce(i64) -> ClientMessage) {
        if let Some(open) = self.open.as_ref() {
            self.connection.send(action(open.id));
        }
    }

    fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_send(cx) {
            return;
        }
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let text = self.composer.read(cx).text().to_owned();
        self.connection.send(match self.editing.take() {
            Some(queue_id) => ClientMessage::EditQueuedMessage {
                conversation_id: open.id,
                queue_id,
                text,
            },
            None => ClientMessage::SendMessage {
                conversation_id: open.id,
                text,
            },
        });
        open.list.set_follow_mode(FollowMode::Tail);
        self.notice = None;
        self.composer
            .update(cx, |composer, cx| composer.clear(window, cx));
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn open_bot(&mut self, bot_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = None;
        self.select_bot(bot_id);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Creates a bot with a default name. The daemon answers with the bot,
    /// and this client opens it.
    fn new_bot(&mut self) {
        if matches!(self.status, Status::Connected) {
            self.connection.send(ClientMessage::CreateBot {
                draft: BotDraft {
                    name: bots::next_name(&self.bots),
                    ..BotDraft::default()
                },
            });
        }
    }

    /// Starts a conversation of the selected bot. The daemon makes it the
    /// bot's current one, and this client shows it.
    fn new_chat(&mut self) {
        if let Some(bot_id) = self.selected_bot
            && matches!(self.status, Status::Connected)
        {
            self.connection
                .send(ClientMessage::NewConversation { bot_id });
        }
    }

    /// Shows the conversations of the selected bot, or goes back to the one
    /// the pane showed.
    fn toggle_history(&mut self) {
        if self.history.take().is_some() {
            self.conversation_shown();
            return;
        }
        if let Some(bot_id) = self.selected_bot {
            self.history = Some(Vec::new());
            self.connection
                .send(ClientMessage::ListConversations { bot_id });
            // The history covers the conversation: what was shown is read,
            // and what arrives from now on is not.
            if let Some(open) = self.open.as_ref()
                && open.at_end.get()
            {
                self.connection.send(ClientMessage::MarkRead {
                    conversation_id: open.id,
                    following: false,
                });
            }
        }
    }

    fn open_from_history(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.history = None;
        if self.selected == Some(id) {
            self.conversation_shown();
        }
        self.select(id);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// The pane shows the conversation: when the list is at its end, all of
    /// it is read, and so is what arrives while the end stays on screen.
    fn conversation_shown(&self) {
        if let Some(open) = self.open.as_ref()
            && open.at_end.get()
            && self.history.is_none()
        {
            self.connection.send(ClientMessage::MarkRead {
                conversation_id: open.id,
                following: true,
            });
        }
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.composer.focus_handle(cx), cx);
    }

    fn resolve_approval(&mut self, approval_id: String, decision: ApprovalDecision) {
        if let Some(open) = self.open.as_ref() {
            self.connection.send(ClientMessage::ResolveApproval {
                conversation_id: open.id,
                approval_id,
                decision,
            });
        }
    }

    fn render_sidebar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(self.status, Status::Connected);
        let rows = self.bots.iter().map(|summary| {
            let id = summary.bot.id;
            let selected = self.selected_bot == Some(id);
            let name: SharedString = summary.bot.name.clone().into();
            let status = bots::status_label(summary);
            let status_color = match summary.state {
                BotState::Waiting => theme.attention_text,
                BotState::Failed => theme.danger_text,
                BotState::Unread => theme.text,
                BotState::Working | BotState::Idle => theme.text_secondary,
            };
            let label: SharedString = match &status {
                Some(status) => format!("{name}, {status}").into(),
                None => name.clone(),
            };
            let hover = theme.hover;
            div()
                .id(("bot", usize::try_from(id).unwrap_or_default()))
                .focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(label)
                .flex_none()
                .h(px(36.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(6.))
                .border_2()
                .border_color(gpui::transparent_black())
                .focus_visible(|style| style.border_color(theme.focus_ring))
                .cursor_pointer()
                .when(selected, |row| {
                    row.bg(theme.selected).font_weight(FontWeight::MEDIUM)
                })
                .when(!selected, |row| row.hover(move |style| style.bg(hover)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_bot(id, window, cx);
                }))
                .child(
                    div()
                        .flex_none()
                        .size(px(22.))
                        .rounded_full()
                        .bg(bots::avatar_color(&summary.bot.avatar.color)
                            .unwrap_or(theme.accent_solid))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(theme.on_accent)
                        .child(SharedString::from(summary.bot.avatar.glyph.clone())),
                )
                .child(div().flex_1().min_w_0().truncate().child(name))
                .children(status.map(|status| {
                    div()
                        .flex_none()
                        .text_color(status_color)
                        .child(SharedString::from(status))
                }))
        });
        let (status, status_color) = match &self.status {
            Status::Connected => ("Connected to the daemon", theme.text_secondary),
            Status::Connecting => ("Connecting to the daemon…", theme.text_secondary),
            Status::Starting => ("Starting the daemon…", theme.text_secondary),
            Status::Disconnected(_) => ("Reconnecting to the daemon…", theme.danger_text),
            Status::Failed(_) => ("Daemon not connected", theme.danger_text),
        };
        div()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.separator)
            .child(
                div().p(px(12.)).child(
                    button(
                        "new-bot",
                        "New bot",
                        if connected {
                            ButtonStyle::Quiet
                        } else {
                            ButtonStyle::Disabled
                        },
                        theme,
                    )
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_bot();
                        this.focus_composer(window, cx);
                    })),
                ),
            )
            .child(
                div()
                    .id("bots")
                    .role(Role::List)
                    .aria_label("Bots")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(rows),
            )
            .child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(theme.separator)
                    .text_color(status_color)
                    .child(status),
            )
    }

    /// The bot the pane shows, the title of its conversation, and the way to
    /// its other conversations.
    fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let bot = self.selected_bot()?;
        let title: SharedString = if self.history.is_some() {
            "History".into()
        } else {
            self.open
                .as_ref()
                .and_then(|open| open.title.clone())
                .unwrap_or_else(|| "New chat".to_owned())
                .into()
        };
        Some(
            div()
                .id("conversation-header")
                .flex_none()
                .px(px(24.))
                .py(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .border_b_1()
                .border_color(theme.separator)
                .child(
                    div()
                        .flex_none()
                        .max_w(px(240.))
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(bot.bot.name.clone())),
                )
                .child(
                    div()
                        .id("conversation-title")
                        .role(Role::Heading)
                        .aria_label(title.clone())
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text_secondary)
                        .child(title),
                )
                .child(
                    button(
                        "toggle-history",
                        if self.history.is_some() {
                            "Back to chat"
                        } else {
                            "History"
                        },
                        ButtonStyle::Quiet,
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_history();
                        cx.notify();
                    })),
                )
                .child(
                    button("new-chat", "New chat", ButtonStyle::Quiet, theme).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.new_chat();
                            this.focus_composer(window, cx);
                        }),
                    ),
                ),
        )
    }

    /// The conversations of the selected bot, the most recently active first.
    fn render_history(
        &self,
        conversations: &[ConversationSummary],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.selected_bot().map(|bot| bot.current_conversation_id);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .unwrap_or_default();
        let rows = conversations.iter().map(|summary| {
            let id = summary.id;
            let title: SharedString = summary
                .title
                .clone()
                .unwrap_or_else(|| "New chat".to_owned())
                .into();
            let mut detail = bots::ago(now_ms, summary.last_activity_ms);
            if current == Some(id) {
                detail = format!("Current · {detail}");
            }
            let hover = theme.hover;
            div()
                .id(("history", usize::try_from(id).unwrap_or_default()))
                .focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(SharedString::from(format!("{title}, {detail}")))
                .flex_none()
                .h(px(36.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(12.))
                .rounded(px(6.))
                .border_2()
                .border_color(gpui::transparent_black())
                .focus_visible(|style| style.border_color(theme.focus_ring))
                .cursor_pointer()
                .when(self.selected == Some(id), |row| row.bg(theme.selected))
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_from_history(id, window, cx);
                }))
                .child(div().flex_1().min_w_0().truncate().child(title))
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.text_secondary)
                        .child(SharedString::from(detail)),
                )
        });
        div()
            .size_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("history")
                    .role(Role::List)
                    .aria_label("Conversations of this bot")
                    .w_full()
                    .max_w(px(COLUMN_WIDTH))
                    .h_full()
                    .overflow_y_scroll()
                    .px(px(24.))
                    .py(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_item(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::for_appearance(window.appearance());
        let Some((item, last)) = self.open.as_ref().and_then(|open| {
            let items = open.transcript.items();
            Some((items.get(index)?, index + 1 == items.len()))
        }) else {
            return div().into_any_element();
        };
        let (top, body) = match item {
            Item::User(text) => (
                24.,
                div()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .max_w(px(COLUMN_WIDTH * 0.8))
                            .px(px(14.))
                            .py(px(9.))
                            .rounded(px(12.))
                            .bg(theme.selected)
                            .text_size(px(15.))
                            .line_height(px(22.))
                            .child(readable(index, text)),
                    )
                    .into_any_element(),
            ),
            Item::Assistant(text) => (
                16.,
                div()
                    .text_size(px(15.))
                    .line_height(px(22.))
                    .child(readable(index, text.trim_matches('\n')))
                    .into_any_element(),
            ),
            Item::Thought(text) => (
                12.,
                div()
                    .pl(px(10.))
                    .border_l_2()
                    .border_color(theme.separator)
                    .text_color(theme.text_secondary)
                    .child(readable(index, text.trim_matches('\n')))
                    .into_any_element(),
            ),
            Item::Tool { title, status, .. } => {
                let (label, color) = match status {
                    Some(ToolStatus::Pending) => ("Waiting", theme.text_secondary),
                    Some(ToolStatus::InProgress) => ("Running", theme.text_secondary),
                    Some(ToolStatus::Completed) => ("Done", theme.success_text),
                    Some(ToolStatus::Failed) => ("Failed", theme.danger_text),
                    None => ("", theme.text_secondary),
                };
                (
                    8.,
                    div()
                        .flex()
                        .gap(px(8.))
                        .text_color(theme.text_secondary)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(title.clone())),
                        )
                        .child(div().flex_none().text_color(color).child(label))
                        .into_any_element(),
                )
            }
            Item::Approval {
                id,
                title,
                state,
                automatic,
            } => (
                12.,
                self.render_approval(
                    index,
                    id.clone(),
                    title.clone(),
                    (*state, *automatic),
                    &theme,
                    cx,
                ),
            ),
            Item::Notice(text) => (
                12.,
                div()
                    .text_color(theme.danger_text)
                    .child(readable(index, text))
                    .into_any_element(),
            ),
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN_WIDTH))
                    .px(px(24.))
                    .pt(px(top))
                    .when(last, |column| column.pb(px(24.)))
                    .child(body),
            )
            .into_any_element()
    }

    fn render_approval(
        &self,
        index: usize,
        id: Option<String>,
        title: Option<String>,
        (state, automatic): (ApprovalState, Option<ApprovalReason>),
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = title.unwrap_or_else(|| "use a tool".to_owned());
        let settled = |label: &'static str, color| {
            div()
                .flex()
                .gap(px(8.))
                .child(div().flex_none().text_color(color).child(label))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text_secondary)
                        .child(SharedString::from(title.clone())),
                )
                .into_any_element()
        };
        let id = match (state, id) {
            (ApprovalState::Waiting, Some(id)) => id,
            (ApprovalState::Allowed, _) => {
                let label = match automatic {
                    Some(ApprovalReason::PermissionLevel) => "Allowed by Full access",
                    _ => "Allowed",
                };
                return settled(label, theme.success_text);
            }
            (ApprovalState::Declined, _) => {
                let label = match automatic {
                    Some(ApprovalReason::NoAllowOnce) => "Declined: it could not be allowed once",
                    _ => "Declined",
                };
                return settled(label, theme.danger_text);
            }
            (ApprovalState::Cancelled, _) => return settled("Cancelled", theme.text_secondary),
            (ApprovalState::Expired | ApprovalState::Waiting, _) => {
                return settled("Not answered", theme.text_secondary);
            }
        };
        let allow_id = id.clone();
        div()
            .id(("approval", index))
            .role(Role::Group)
            .aria_label(SharedString::from(format!("The bot asks to {title}")))
            .p(px(14.))
            .rounded(px(10.))
            .bg(theme.attention_surface)
            .border_1()
            .border_color(theme.attention_border)
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_color(theme.attention_text)
                            .font_weight(FontWeight::MEDIUM)
                            .child("The bot asks for your approval"),
                    )
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(22.))
                            .child(SharedString::from(title)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        button(("allow", index), "Allow once", ButtonStyle::Solid, theme).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.resolve_approval(allow_id.clone(), ApprovalDecision::Allowed);
                                this.focus_composer(window, cx);
                            }),
                        ),
                    )
                    .child(
                        button(("decline", index), "Decline", ButtonStyle::Quiet, theme).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.resolve_approval(id.clone(), ApprovalDecision::Declined);
                                this.focus_composer(window, cx);
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_transcript(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let message = |text: &'static str| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.text_secondary)
                .child(text)
                .into_any_element()
        };
        if let Some(history) = &self.history {
            return self.render_history(history, theme, cx);
        }
        match &self.open {
            Some(open) if open.transcript.items().is_empty() => {
                message("Send a message to start this conversation.")
            }
            Some(open) => list(
                open.list.clone(),
                cx.processor(|this, index, window, cx| this.render_item(index, window, cx)),
            )
            .size_full()
            .into_any_element(),
            None if !matches!(self.status, Status::Connected) => {
                message("Bots appear here once the daemon is connected.")
            }
            None if self.selected_bot.is_none() => message("Create a bot to start."),
            None => message("Opening the conversation…"),
        }
    }

    /// The messages that wait for the running turn to end, above the composer.
    fn render_queue(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let queue = &self.open.as_ref()?.queue;
        if queue.messages.is_empty() {
            return None;
        }
        let heading = if queue.paused {
            "Queue paused: the last turn did not finish."
        } else if queue.messages.len() == 1 {
            "1 message waits for the bot."
        } else {
            "Messages wait for the bot, in this order."
        };
        let rows = queue.messages.iter().enumerate().map(|(index, message)| {
            let id = message.id;
            let editing = self.editing == Some(id);
            let label = SharedString::from(message.text.clone());
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .id(("queued", index))
                        .role(Role::ListItem)
                        .aria_label(label.clone())
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(editing, |text| text.text_color(theme.text_secondary))
                        .child(label),
                )
                .child(
                    button(("queue-edit", index), "Edit", ButtonStyle::Quiet, theme).on_click(
                        cx.listener(move |this, _, window, cx| this.edit_queued(id, window, cx)),
                    ),
                )
                .child(
                    button(
                        ("queue-send-now", index),
                        "Send now",
                        ButtonStyle::Quiet,
                        theme,
                    )
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.queue_action(|conversation_id| ClientMessage::SendQueuedMessageNow {
                            conversation_id,
                            queue_id: id,
                        });
                    })),
                )
                .child(
                    button(("queue-delete", index), "Delete", ButtonStyle::Quiet, theme).on_click(
                        cx.listener(move |this, _, _, _| {
                            this.queue_action(|conversation_id| {
                                ClientMessage::DeleteQueuedMessage {
                                    conversation_id,
                                    queue_id: id,
                                }
                            });
                        }),
                    ),
                )
        });
        Some(
            div()
                .id("queue")
                .role(Role::List)
                .aria_label("Queued messages")
                .p(px(10.))
                .rounded(px(10.))
                .border_1()
                .border_color(if queue.paused {
                    theme.attention_border
                } else {
                    theme.separator
                })
                .when(queue.paused, |panel| panel.bg(theme.attention_surface))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(if queue.paused {
                                    theme.attention_text
                                } else {
                                    theme.text_secondary
                                })
                                .child(heading),
                        )
                        .children(queue.paused.then(|| {
                            button("queue-resume", "Resume", ButtonStyle::Solid, theme).on_click(
                                cx.listener(|this, _, _, _| {
                                    this.queue_action(|conversation_id| {
                                        ClientMessage::ResumeQueue { conversation_id }
                                    });
                                }),
                            )
                        })),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    fn render_composer(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_send = self.can_send(cx);
        let can_stop = self.can_stop(cx);
        let send_control = if can_stop {
            button("stop-turn", "Stop", ButtonStyle::Quiet, theme)
                .on_click(cx.listener(|this, _, _, _| this.stop_turn()))
        } else {
            button(
                "send-message",
                if self.editing.is_some() {
                    "Save"
                } else {
                    "Send"
                },
                if can_send {
                    ButtonStyle::Solid
                } else {
                    ButtonStyle::Disabled
                },
                theme,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.send_message(window, cx);
            }))
        };
        let focused = self.composer.focus_handle(cx).is_focused(window);
        let activity = self.open.as_ref().and_then(|open| {
            if !open.transcript.turn_active() {
                return None;
            }
            let waiting = open.transcript.items().iter().any(|item| {
                matches!(
                    item,
                    Item::Approval {
                        state: ApprovalState::Waiting,
                        ..
                    }
                )
            });
            Some(if waiting {
                "The bot is waiting for your approval."
            } else {
                "The bot is working…"
            })
        });
        div()
            .w_full()
            .max_w(px(COLUMN_WIDTH))
            .mx_auto()
            .px(px(24.))
            .pb(px(20.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(self.notice.clone().map(|notice| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.danger_text)
                            .child(notice),
                    )
                    .child(
                        button("dismiss-notice", "Dismiss", ButtonStyle::Quiet, theme).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.notice = None;
                                this.focus_composer(window, cx);
                                cx.notify();
                            }),
                        ),
                    )
            }))
            .children(self.render_queue(theme, cx))
            .child(
                div()
                    .id("activity")
                    .role(Role::Status)
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_color(theme.text_secondary)
                    .children(if self.editing.is_some() {
                        Some("Editing a queued message.")
                    } else {
                        activity
                    })
                    .children(self.editing.map(|_| {
                        button("cancel-edit", "Cancel", ButtonStyle::Quiet, theme).on_click(
                            cx.listener(|this, _, window, cx| this.cancel_edit(window, cx)),
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(8.))
                    .p(px(6.))
                    .rounded(px(12.))
                    .bg(theme.raised)
                    .border_1()
                    .border_color(if focused {
                        theme.focus_ring
                    } else {
                        theme.control_border
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(8.))
                            .py(px(5.))
                            .child(self.composer.clone()),
                    )
                    .child(send_control),
            )
    }
}

fn button(
    id: impl Into<ElementId>,
    label: &'static str,
    style: ButtonStyle,
    theme: &Theme,
) -> Stateful<Div> {
    let focus_ring = theme.focus_ring;
    let base = div()
        .id(id)
        .focusable()
        .tab_stop(style != ButtonStyle::Disabled)
        .role(Role::Button)
        .aria_label(label)
        .flex_none()
        .h(px(32.))
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_2()
        .font_weight(FontWeight::MEDIUM)
        .focus_visible(move |style| style.border_color(focus_ring))
        .child(label);
    match style {
        ButtonStyle::Solid => {
            let hover = theme.accent_solid_hover;
            base.bg(theme.accent_solid)
                .border_color(theme.accent_solid)
                .text_color(theme.on_accent)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
        }
        ButtonStyle::Quiet => {
            let hover = theme.selected;
            base.bg(theme.hover)
                .border_color(theme.hover)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
        }
        ButtonStyle::Disabled => base
            .bg(theme.hover)
            .border_color(theme.hover)
            .text_color(theme.text_secondary),
    }
}

fn readable(index: usize, text: &str) -> Text {
    Text::new(
        ElementId::from(("message", index)),
        SharedString::from(text.to_owned()),
    )
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let banner = |text: SharedString, retry: bool| {
            div()
                .id("connection-notice")
                .role(Role::Status)
                .aria_label(text.clone())
                .px(px(24.))
                .py(px(10.))
                .flex()
                .items_center()
                .gap(px(12.))
                .bg(theme.attention_surface)
                .border_b_1()
                .border_color(theme.attention_border)
                .child(div().flex_1().min_w_0().child(text))
                .children(retry.then(|| {
                    button("retry-connection", "Retry", ButtonStyle::Solid, &theme).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.status = Status::Connecting;
                            this.connection.retry();
                            cx.notify();
                        }),
                    )
                }))
        };
        let banner = match &self.status {
            Status::Starting => Some(banner("Starting the daemon…".into(), false)),
            Status::Disconnected(reason) if reason.is_empty() => Some(banner(
                "The daemon is not reachable. Reconnecting.".into(),
                false,
            )),
            Status::Disconnected(reason) => Some(banner(
                format!("The daemon is not reachable. Reconnecting. ({reason})").into(),
                false,
            )),
            Status::Failed(reason) => Some(banner(reason.clone(), true)),
            Status::Connecting | Status::Connected => None,
        };
        div()
            .id("workspace")
            .role(Role::Application)
            .aria_label("Crow Bot")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &FocusNext, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPrevious, window, cx| window.focus_prev(cx)))
            .size_full()
            .flex()
            .bg(theme.background)
            .text_color(theme.text)
            .text_size(px(13.))
            .line_height(px(20.))
            .child(self.render_sidebar(&theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .children(banner)
                    .children(self.render_header(&theme, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_transcript(&theme, cx)),
                    )
                    .children(
                        self.history
                            .is_none()
                            .then(|| self.render_composer(&theme, window, cx)),
                    ),
            )
    }
}
