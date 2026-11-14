use crowbot_protocol::{
    ApprovalDecision, ClientMessage, ConversationSummary, Event, QueueState, ServerMessage,
    ToolStatus,
};
use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FocusHandle, Focusable, FollowMode,
    FontWeight, KeyBinding, ListAlignment, ListState, Role, SharedString, Stateful, Subscription,
    Task, Text, Window, actions, div, list, prelude::*, px,
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::composer::{Composer, ComposerEvent};
use crate::connection::{Connection, ConnectionEvent};
use crate::theme::Theme;
use crate::transcript::{ApprovalState, Item, Transcript};

const COLUMN_WIDTH: f32 = 760.;
const SIDEBAR_WIDTH: f32 = 260.;
const TITLE_LENGTH: usize = 80;

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
    transcript: Transcript,
    list: ListState,
    queue: QueueState,
}

pub struct Workspace {
    connection: Connection,
    status: Status,
    conversations: Vec<ConversationSummary>,
    selected: Option<i64>,
    open: Option<OpenConversation>,
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
            conversations: Vec::new(),
            selected: None,
            open: None,
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
                self.connection.send(ClientMessage::ListConversations);
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
            ConnectionEvent::Message(message) => self.handle_message(message),
        }
        cx.notify();
    }

    fn handle_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Welcome { .. } => {}
            ServerMessage::Conversations(conversations) => {
                self.conversations = conversations;
                if self.selected.is_none() {
                    match self.conversations.first() {
                        Some(latest) => self.select(latest.id),
                        None => self.connection.send(ClientMessage::CreateConversation),
                    }
                }
            }
            ServerMessage::ConversationCreated(summary) => {
                let id = summary.id;
                self.conversations.insert(0, summary);
                self.select(id);
            }
            ServerMessage::ConversationOpened(snapshot) => {
                let id = snapshot.conversation.id;
                if self.selected != Some(id) {
                    return;
                }
                self.remember(snapshot.conversation.clone());
                let queue = snapshot.queue.clone();
                let transcript = Transcript::from_snapshot(snapshot);
                let list = ListState::new(transcript.items().len(), ListAlignment::Top, px(600.));
                list.set_follow_mode(FollowMode::Tail);
                self.open = Some(OpenConversation {
                    id,
                    transcript,
                    list,
                    queue,
                });
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
            ServerMessage::Error { message } => self.notice = Some(message.into()),
        }
    }

    fn remember(&mut self, summary: ConversationSummary) {
        match self
            .conversations
            .iter_mut()
            .find(|known| known.id == summary.id)
        {
            Some(known) => *known = summary,
            None => self.conversations.insert(0, summary),
        }
    }

    fn apply(&mut self, conversation_id: i64, event: Event) {
        if let Event::UserText(text) = &event
            && let Some(summary) = self
                .conversations
                .iter_mut()
                .find(|summary| summary.id == conversation_id)
            && summary.title.is_none()
        {
            summary.title = title_from(text);
        }
        let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) else {
            return;
        };
        let before = open.transcript.items().len();
        let first_changed = open.transcript.apply(event);
        let after = open.transcript.items().len();
        if first_changed < before {
            open.list.remeasure_items(first_changed..before);
        }
        if after > before {
            open.list.splice(before..before, after - before);
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

    fn open_from_sidebar(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.select(id);
        self.focus_composer(window, cx);
        cx.notify();
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
        let rows = self.conversations.iter().map(|summary| {
            let id = summary.id;
            let selected = self.selected == Some(id);
            let title: SharedString = summary
                .title
                .clone()
                .unwrap_or_else(|| "New conversation".to_owned())
                .into();
            let hover = theme.hover;
            div()
                .id(("conversation", usize::try_from(id).unwrap_or_default()))
                .focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(title.clone())
                .flex_none()
                .h(px(32.))
                .px(px(10.))
                .flex()
                .items_center()
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
                    this.open_from_sidebar(id, window, cx);
                }))
                .child(div().truncate().child(title))
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
                        "new-conversation",
                        "New conversation",
                        if connected {
                            ButtonStyle::Quiet
                        } else {
                            ButtonStyle::Disabled
                        },
                        theme,
                    )
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| {
                        if matches!(this.status, Status::Connected) {
                            this.connection.send(ClientMessage::CreateConversation);
                            this.focus_composer(window, cx);
                        }
                    })),
                ),
            )
            .child(
                div()
                    .id("conversations")
                    .role(Role::List)
                    .aria_label("Conversations")
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
            Item::Approval { id, title, state } => (
                12.,
                self.render_approval(index, id.clone(), title.clone(), *state, &theme, cx),
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
        state: ApprovalState,
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
            (ApprovalState::Allowed, _) => return settled("Allowed", theme.success_text),
            (ApprovalState::Declined, _) => return settled("Declined", theme.danger_text),
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
            None if matches!(self.status, Status::Connected) => {
                message("Opening the conversation…")
            }
            None => message("Conversations appear here once the daemon is connected."),
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

fn title_from(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?.trim();
    let mut title: String = line.chars().take(TITLE_LENGTH).collect();
    if line.chars().count() > TITLE_LENGTH {
        title.push('…');
    }
    Some(title)
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
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_transcript(&theme, cx)),
                    )
                    .child(self.render_composer(&theme, window, cx)),
            )
    }
}
