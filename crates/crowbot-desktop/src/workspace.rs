mod bot_list;
mod conversation;
mod header;
mod overlays;
mod screens;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use crowbot_protocol::{
    ApprovalDecision, BotDraft, BotPatch, BotState, BotSummary, ClientMessage, ConversationSummary,
    Event, Preferences, QueueState, ServerMessage,
};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FollowMode, KeyBinding, KeyDownEvent,
    ListAlignment, ListState, Pixels, Point, Role, SharedString, Subscription, Task, Window,
    actions, div, prelude::*,
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::bots;
use crate::composer::{Composer, ComposerEvent, InputStyle};
use crate::connection::{Connection, ConnectionEvent};
use crate::layout::SidebarLayout;
use crate::menu::{self, MenuAction, MenuEntry};
use crate::sidebar;
use crate::theme::{Theme, font, layout, text};
use crate::transcript::Transcript;

actions!(workspace, [FocusNext, FocusPrevious]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
    ]);
}

#[derive(Clone, Debug, PartialEq)]
enum Link {
    Connecting,
    Starting,
    Connected,
    Reconnecting(SharedString),
    Failed(SharedString),
    Incompatible {
        daemon_protocol: u32,
        daemon_version: Option<SharedString>,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum Screen {
    Loading,
    Unreachable(SharedString),
    Incompatible {
        daemon_protocol: u32,
        daemon_version: Option<SharedString>,
    },
    Shell,
}

struct OpenConversation {
    id: i64,
    title: Option<String>,
    transcript: Transcript,
    list: ListState,
    queue: QueueState,
    at_end: Rc<Cell<bool>>,
    turn_model: Option<Option<String>>,
}

struct PendingChange {
    before: BotSummary,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuTarget {
    Bot(i64),
    Area,
}

struct Menu {
    target: MenuTarget,
    position: Option<Point<Pixels>>,
    highlighted: Option<usize>,
    restore_focus: Option<FocusHandle>,
}

struct ResizeSidebar;

struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub struct Workspace {
    connection: Connection,
    link: Link,
    loaded: bool,
    preferences: Preferences,
    sidebar: SidebarLayout,
    saved_sidebar: SidebarLayout,
    resizing: bool,
    bots: Vec<BotSummary>,
    pending: Vec<PendingChange>,
    deleting: HashSet<i64>,
    selected_bot: Option<i64>,
    selected: Option<i64>,
    open: Option<OpenConversation>,
    load_error: Option<SharedString>,
    history: Option<Vec<ConversationSummary>>,
    composer: Entity<Composer>,
    search: Entity<Composer>,
    editing: Option<i64>,
    notice: Option<SharedString>,
    menu: Option<Menu>,
    menu_focus: FocusHandle,
    confirm_delete: Option<i64>,
    dialog_focus: FocusHandle,
    dialog_confirm_focus: FocusHandle,
    side_panel: bool,
    hidden_open: bool,
    title_width: Rc<Cell<Pixels>>,
    inert: bool,
    retry_focus: FocusHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _events: Task<()>,
}

impl Workspace {
    pub fn new(
        connection: Connection,
        mut events: UnboundedReceiver<ConnectionEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| Composer::new("Message the bot", InputStyle::message(), cx));
        let search = cx.new(|cx| Composer::new("Search", InputStyle::search(), cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event: &ComposerEvent, window, cx| match event {
                    ComposerEvent::Submit => this.send_message(window, cx),
                },
            ),
            cx.observe(&search, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |this, _, cx| this.apply_theme(cx)),
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
        let mut workspace = Self {
            connection,
            link: Link::Connecting,
            loaded: false,
            preferences: Preferences::default(),
            sidebar: SidebarLayout::default(),
            saved_sidebar: SidebarLayout::default(),
            resizing: false,
            bots: Vec::new(),
            pending: Vec::new(),
            deleting: HashSet::new(),
            selected_bot: None,
            selected: None,
            open: None,
            load_error: None,
            history: None,
            composer,
            search,
            editing: None,
            notice: None,
            menu: None,
            menu_focus: cx.focus_handle(),
            confirm_delete: None,
            dialog_focus: cx.focus_handle(),
            dialog_confirm_focus: cx.focus_handle(),
            side_panel: false,
            hidden_open: false,
            title_width: Rc::new(Cell::new(Pixels::MAX)),
            inert: false,
            retry_focus: cx.focus_handle(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
            _events: events,
        };
        workspace.apply_theme(cx);
        workspace
    }

    fn apply_theme(&mut self, cx: &mut Context<Self>) {
        let theme = Theme::resolve(self.preferences.appearance, cx.window_appearance());
        if cx.try_global::<Theme>() != Some(&theme) {
            cx.set_global(theme);
            cx.refresh_windows();
        }
    }

    fn live(&self) -> bool {
        self.link == Link::Connected
    }

    fn screen(&self) -> Screen {
        match &self.link {
            Link::Incompatible {
                daemon_protocol,
                daemon_version,
            } => Screen::Incompatible {
                daemon_protocol: *daemon_protocol,
                daemon_version: daemon_version.clone(),
            },
            Link::Failed(reason) if !self.loaded => Screen::Unreachable(reason.clone()),
            _ if !self.loaded => Screen::Loading,
            _ => Screen::Shell,
        }
    }

    fn set_link(&mut self, link: Link, cx: &mut Context<Self>) {
        self.link = link;
        let live = self.live();
        if !live {
            self.menu = None;
            self.confirm_delete = None;
        }
        self.composer
            .update(cx, |composer, cx| composer.set_enabled(live, cx));
    }

    fn handle_connection(&mut self, event: ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::Connected => {
                self.set_link(Link::Connected, cx);
                self.load_error = None;
                self.connection.send(ClientMessage::GetPreferences);
                self.connection.send(ClientMessage::ListBots);
                if let Some(conversation_id) = self.selected {
                    self.connection
                        .send(ClientMessage::OpenConversation { conversation_id });
                }
            }
            ConnectionEvent::Starting => self.set_link(Link::Starting, cx),
            ConnectionEvent::Disconnected(reason) => {
                self.roll_back();
                self.set_link(Link::Reconnecting(reason.into()), cx);
            }
            ConnectionEvent::Failed(reason) => {
                self.roll_back();
                self.set_link(Link::Failed(reason.into()), cx);
            }
            ConnectionEvent::Incompatible {
                daemon_protocol,
                daemon_version,
            } => {
                self.roll_back();
                self.set_link(
                    Link::Incompatible {
                        daemon_protocol,
                        daemon_version: daemon_version.map(SharedString::from),
                    },
                    cx,
                );
            }
            ConnectionEvent::Message(message) => self.handle_message(*message, cx),
        }
        cx.notify();
    }

    fn handle_message(&mut self, message: ServerMessage, cx: &mut Context<Self>) {
        match message {
            ServerMessage::Welcome { .. } | ServerMessage::Incompatible { .. } => {}
            ServerMessage::Preferences(preferences) => {
                self.saved_sidebar = SidebarLayout::from_preferences(&preferences);
                if !self.resizing {
                    self.sidebar = self.saved_sidebar;
                }
                self.preferences = preferences;
                self.apply_theme(cx);
            }
            ServerMessage::Bots(bots) => {
                self.bots = bots;
                self.loaded = true;
                self.pending.clear();
                self.deleting.clear();
                if self.selected_bot().is_none() {
                    self.forget_selection();
                    self.select_first_bot();
                }
            }
            ServerMessage::BotChanged(summary) => {
                let id = summary.bot.id;
                if self.deleting.contains(&id) {
                    return;
                }
                self.pending.retain(|change| change.before.bot.id != id);
                match self.bots.iter_mut().find(|known| known.bot.id == id) {
                    Some(known) => *known = summary,
                    None => self.bots.push(summary),
                }
                if self.selected_bot.is_none() {
                    self.select_bot(id);
                }
            }
            ServerMessage::BotCreated { bot_id } => self.select_bot(bot_id),
            ServerMessage::BotDeleted { bot_id } => {
                self.deleting.remove(&bot_id);
                self.pending.retain(|change| change.before.bot.id != bot_id);
                self.bots.retain(|known| known.bot.id != bot_id);
                if self.confirm_delete == Some(bot_id) {
                    self.confirm_delete = None;
                }
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
                let turn_model = snapshot.turn_model.clone();
                let transcript = Transcript::from_snapshot(snapshot);
                let list = ListState::new(
                    transcript.items().len(),
                    ListAlignment::Top,
                    layout::LIST_OVERDRAW,
                );
                list.set_follow_mode(FollowMode::Tail);
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
                    turn_model,
                });
                self.load_error = None;
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
            ServerMessage::TurnModel {
                conversation_id,
                model,
            } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) {
                    open.turn_model = Some(model);
                }
            }
            ServerMessage::Error { message } | ServerMessage::InvalidField { message, .. } => {
                self.fail(message);
            }
        }
    }

    fn loading_conversation(&self) -> bool {
        self.selected.is_some() && self.open.is_none() && self.load_error.is_none()
    }

    fn fail(&mut self, message: String) {
        if !self.pending.is_empty() {
            self.roll_back();
            self.notice = Some(format!("The change was not saved: {message}").into());
        } else if self.loading_conversation() {
            self.load_error = Some(message.into());
        } else {
            self.notice = Some(message.into());
        }
    }

    fn roll_back(&mut self) {
        for change in std::mem::take(&mut self.pending).into_iter().rev() {
            let id = change.before.bot.id;
            match self.bots.iter_mut().find(|known| known.bot.id == id) {
                Some(known) => *known = change.before,
                None => {
                    let index = change.index.min(self.bots.len());
                    self.bots.insert(index, change.before);
                }
            }
        }
        self.deleting.clear();
        if self.selected_bot.is_none() {
            self.select_first_bot();
        }
    }

    fn change_bot(&mut self, bot_id: i64, change: impl FnOnce(&mut BotSummary)) -> bool {
        let Some(index) = self.bots.iter().position(|known| known.bot.id == bot_id) else {
            return false;
        };
        self.pending.push(PendingChange {
            before: self.bots[index].clone(),
            index,
        });
        change(&mut self.bots[index]);
        true
    }

    fn selected_bot(&self) -> Option<&BotSummary> {
        self.bot(self.selected_bot?)
    }

    fn bot(&self, bot_id: i64) -> Option<&BotSummary> {
        self.bots.iter().find(|known| known.bot.id == bot_id)
    }

    fn forget_selection(&mut self) {
        self.selected_bot = None;
        self.selected = None;
        self.open = None;
        self.load_error = None;
        self.history = None;
        self.editing = None;
    }

    fn select_first_bot(&mut self) {
        let first = sidebar::groups(&self.bots, "")
            .first()
            .and_then(|group| group.bots.first().copied());
        if let Some(first) = first {
            self.select_bot(first);
        }
    }

    fn select_bot(&mut self, bot_id: i64) {
        let Some(current) = self.bot(bot_id).map(|known| known.current_conversation_id) else {
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
        if event.ends_turn() {
            open.turn_model = None;
        }
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
        if open.at_end.get() && self.history.is_none() {
            self.connection.send(ClientMessage::MarkRead {
                conversation_id,
                following: true,
            });
        }
    }

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
        self.load_error = None;
        self.editing = None;
        self.connection
            .send(ClientMessage::OpenConversation { conversation_id });
    }

    fn retry_load(&mut self) {
        if let Some(conversation_id) = self.selected
            && self.live()
        {
            self.load_error = None;
            self.connection
                .send(ClientMessage::OpenConversation { conversation_id });
        }
    }

    fn retry_connection(&mut self, cx: &mut Context<Self>) {
        self.set_link(Link::Connecting, cx);
        self.connection.retry();
        cx.notify();
    }

    fn can_send(&self, cx: &App) -> bool {
        self.live()
            && self.open.is_some()
            && self.history.is_none()
            && !self.composer.read(cx).text().trim().is_empty()
    }

    fn can_stop(&self, cx: &App) -> bool {
        self.live()
            && self
                .open
                .as_ref()
                .is_some_and(|open| open.transcript.turn_active())
            && self.composer.read(cx).text().trim().is_empty()
    }

    fn stop_turn(&mut self) {
        if let Some(open) = self.open.as_ref()
            && self.live()
        {
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
        if let Some(open) = self.open.as_ref()
            && self.live()
        {
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
        self.select_bot(bot_id);
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn new_bot(&mut self) {
        if self.live() {
            self.connection.send(ClientMessage::CreateBot {
                draft: BotDraft {
                    name: bots::next_name(&self.bots),
                    ..BotDraft::default()
                },
            });
        }
    }

    fn new_chat(&mut self) {
        if let Some(bot_id) = self.selected_bot
            && self.live()
        {
            self.connection
                .send(ClientMessage::NewConversation { bot_id });
        }
    }

    fn toggle_history(&mut self) {
        if self.history.take().is_some() {
            self.conversation_shown();
            return;
        }
        if let Some(bot_id) = self.selected_bot
            && self.live()
        {
            self.history = Some(Vec::new());
            self.connection
                .send(ClientMessage::ListConversations { bot_id });
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
        if let Some(open) = self.open.as_ref()
            && self.live()
        {
            self.connection.send(ClientMessage::ResolveApproval {
                conversation_id: open.id,
                approval_id,
                decision,
            });
        }
    }

    fn pending_model(&self) -> Option<SharedString> {
        let open = self.open.as_ref()?;
        let running = open
            .turn_model
            .as_ref()
            .filter(|_| open.transcript.turn_active())?;
        let next = &self.selected_bot()?.bot.model;
        (running != next).then(|| model_label(next))
    }

    fn resize_sidebar(&mut self, pointer: Pixels, window: &Window, cx: &mut Context<Self>) {
        self.resizing = true;
        let before = self.sidebar;
        self.sidebar
            .drag(f32::from(pointer), f32::from(window.viewport_size().width));
        if self.sidebar != before {
            cx.notify();
        }
    }

    fn save_sidebar(&mut self) {
        if let Some(patch) = self.sidebar.changes_since(&self.saved_sidebar) {
            self.saved_sidebar = self.sidebar;
            self.connection
                .send(ClientMessage::SetPreferences { patch });
        }
    }

    fn finish_resize(&mut self) {
        self.resizing = false;
        self.save_sidebar();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar.toggle();
        self.save_sidebar();
        cx.notify();
    }

    fn resizer_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        let window_width = f32::from(window.viewport_size().width);
        match event.keystroke.key.as_str() {
            "left" => self.sidebar.step(-1., window_width),
            "right" => self.sidebar.step(1., window_width),
            "home" => {
                if !self.sidebar.is_compact() {
                    self.sidebar.toggle();
                }
            }
            "end" => self.sidebar.drag(layout::SIDEBAR_MAX, window_width),
            _ => return,
        }
        self.save_sidebar();
        cx.stop_propagation();
        cx.notify();
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.is_compact() {
            self.toggle_sidebar(cx);
        }
        window.focus(&self.search.focus_handle(cx), cx);
    }

    fn menu_entries(&self) -> Vec<MenuEntry> {
        match self.menu.as_ref().map(|menu| menu.target) {
            Some(MenuTarget::Bot(bot_id)) => {
                self.bot(bot_id).map(menu::bot_menu).unwrap_or_default()
            }
            Some(MenuTarget::Area) => menu::area_menu(&self.bots),
            None => Vec::new(),
        }
    }

    fn open_menu(
        &mut self,
        target: MenuTarget,
        position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.live() {
            return;
        }
        let restore_focus = match self.menu.take() {
            Some(open) => open.restore_focus,
            None => window.focused(cx),
        };
        self.menu = Some(Menu {
            target,
            position,
            highlighted: None,
            restore_focus,
        });
        if position.is_none() {
            let entries = self.menu_entries();
            if let Some(menu) = self.menu.as_mut() {
                menu.highlighted = menu::step(&entries, None, true);
            }
        }
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.take() {
            if self.menu_focus.contains_focused(window, cx) {
                match menu.restore_focus {
                    Some(focus) => window.focus(&focus, cx),
                    None => self.focus_composer(window, cx),
                }
            }
            cx.notify();
        }
    }

    fn menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.menu_entries();
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "down" => menu.highlighted = menu::step(&entries, menu.highlighted, true),
            "up" => menu.highlighted = menu::step(&entries, menu.highlighted, false),
            "home" => menu.highlighted = menu::step(&entries, None, true),
            "end" => menu.highlighted = menu::step(&entries, None, false),
            "escape" => self.close_menu(window, cx),
            "enter" | "space" => {
                let chosen = menu
                    .highlighted
                    .and_then(|index| entries.get(index))
                    .filter(|entry| entry.enabled)
                    .map(|entry| entry.action);
                if let Some(action) = chosen {
                    self.perform(action, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn perform(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
        if !self.live() {
            return;
        }
        self.notice = None;
        match action {
            MenuAction::MarkRead(bot_id) => self.mark_read(bot_id),
            MenuAction::MarkUnread(bot_id) => {
                if self.change_bot(bot_id, |summary| {
                    summary.unread = summary.unread.max(1);
                    if summary.state == BotState::Idle {
                        summary.state = BotState::Unread;
                    }
                }) {
                    self.connection
                        .send(ClientMessage::MarkBotUnread { bot_id });
                }
            }
            MenuAction::NewChat(bot_id) => {
                self.select_bot(bot_id);
                self.new_chat();
                self.focus_composer(window, cx);
            }
            MenuAction::Edit(bot_id) => {
                self.select_bot(bot_id);
                self.side_panel = true;
            }
            MenuAction::Duplicate(bot_id) => {
                self.connection.send(ClientMessage::DuplicateBot { bot_id });
            }
            MenuAction::SetPinned(bot_id, pinned) => {
                if self.change_bot(bot_id, |summary| summary.bot.pinned = pinned) {
                    self.connection.send(ClientMessage::UpdateBot {
                        bot_id,
                        patch: BotPatch {
                            pinned: Some(pinned),
                            ..BotPatch::default()
                        },
                    });
                }
            }
            MenuAction::SetHidden(bot_id, hidden) => {
                if self.change_bot(bot_id, |summary| summary.bot.hidden = hidden) {
                    self.connection.send(ClientMessage::UpdateBot {
                        bot_id,
                        patch: BotPatch {
                            hidden: Some(hidden),
                            ..BotPatch::default()
                        },
                    });
                }
            }
            MenuAction::Delete(bot_id) => {
                if self.bot(bot_id).is_some() {
                    self.confirm_delete = Some(bot_id);
                    window.focus(&self.dialog_focus, cx);
                }
            }
            MenuAction::NewBot => {
                self.new_bot();
                self.focus_composer(window, cx);
            }
            MenuAction::MarkAllRead => {
                for bot_id in sidebar::unread_bots(&self.bots) {
                    self.mark_read(bot_id);
                }
            }
        }
        cx.notify();
    }

    fn mark_read(&mut self, bot_id: i64) {
        if self.change_bot(bot_id, |summary| {
            summary.unread = 0;
            if matches!(summary.state, BotState::Unread | BotState::Failed) {
                summary.state = BotState::Idle;
            }
        }) {
            self.connection.send(ClientMessage::MarkBotRead { bot_id });
        }
    }

    fn cancel_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_delete.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    fn delete_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bot_id) = self.confirm_delete.take() else {
            return;
        };
        self.focus_composer(window, cx);
        cx.notify();
        if !self.live() {
            return;
        }
        let Some(index) = self.bots.iter().position(|known| known.bot.id == bot_id) else {
            return;
        };
        let before = self.bots.remove(index);
        self.pending.push(PendingChange { before, index });
        self.deleting.insert(bot_id);
        if self.selected_bot == Some(bot_id) {
            self.forget_selection();
            self.select_first_bot();
        }
        self.connection.send(ClientMessage::DeleteBot { bot_id });
    }

    fn toggle_side_panel(&mut self, cx: &mut Context<Self>) {
        self.side_panel = !self.side_panel;
        cx.notify();
    }
}

fn model_label(model: &Option<String>) -> SharedString {
    match model {
        Some(model) => model.clone().into(),
        None => "Default model".into(),
    }
}

fn row_id(bot_id: i64) -> usize {
    usize::try_from(bot_id).unwrap_or_default()
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.resizing && !cx.has_active_drag() {
            self.finish_resize();
        }
        let theme = Theme::of(cx);
        let root = div()
            .id("workspace")
            .role(Role::Application)
            .aria_label("Crow Bot")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &FocusNext, window, cx| {
                this.focus_step(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevious, window, cx| {
                this.focus_step(false, window, cx);
            }))
            .on_drag_move::<ResizeSidebar>(cx.listener(
                |this, event: &gpui::DragMoveEvent<ResizeSidebar>, window, cx| {
                    this.resize_sidebar(event.event.position.x, window, cx);
                },
            ))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.frame)
            .text_color(theme.text)
            .font(font::sans())
            .text_size(text::MD.size)
            .line_height(text::MD.leading);
        let root = match self.screen() {
            Screen::Shell => self.render_shell(root, &theme, window, cx),
            screen => root.child(self.render_screen(&screen, &theme, window, cx)),
        };
        root.children(self.render_floating_menu(&theme, cx))
            .children(self.render_delete_dialog(&theme, cx))
            .children(self.render_toast(&theme, cx))
    }
}

impl Workspace {
    fn focus_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.close_menu(window, cx);
        } else if !self.live() {
            if matches!(self.link, Link::Failed(_) | Link::Incompatible { .. }) {
                window.focus(&self.retry_focus, cx);
            }
        } else if self.confirm_delete.is_some() {
            let cancel = self.dialog_focus.is_focused(window);
            let next = if cancel {
                &self.dialog_confirm_focus
            } else {
                &self.dialog_focus
            };
            window.focus(next, cx);
        } else if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
    }
}
