mod bot_list;
mod bot_panel;
mod compose;
mod conversation;
mod creation;
mod forms;
mod header;
mod overlays;
mod palette;
mod rail;
mod rows;
mod screens;
mod settings;
mod steps;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::Instant;

use crowbot_protocol::{
    ApprovalDecision, BOT_INSTRUCTIONS_LENGTH, BOT_MODEL_LENGTH, BOT_NAME_LENGTH, BOT_TITLE_LENGTH,
    BotPatch, BotState, BotSummary, ClientMessage, ConversationSummary, DaemonInfo, Event,
    FailureCause, MESSAGE_LENGTH, PermissionLevel, Preferences, ProviderModels, ProviderStatus,
    QueueState, SEARCH_QUERY_LENGTH, SendShortcut, ServerMessage, TimedEvent,
};
use gpui::{
    App, ClipboardItem, Context, Entity, FocusHandle, Focusable, FollowMode, KeyBinding,
    KeyDownEvent, ListAlignment, ListOffset, ListState, Pixels, Point, Role, ScrollHandle,
    SharedString, Subscription, Task, Window, actions, div, prelude::*,
};
use tokio::sync::mpsc::UnboundedReceiver;

use self::bot_panel::{BotPanel, PanelField};
use self::creation::Creation;
use self::palette::{Palette, RevealTarget};
use self::settings::{EXECUTABLE_LENGTH, ExecutableEdit, SETTINGS_QUERY_LENGTH, SettingsPage};
use crate::bots;
use crate::composer::{self, Composer, ComposerEvent, InputStyle};
use crate::connection::{Connection, ConnectionEvent};
use crate::frames::{self, FrameLog};
use crate::layout::SidebarLayout;
use crate::markdown::{Document, SharedSelection};
use crate::menu::{self, MenuAction, MenuEntry};
use crate::reveal::Reveal;
use crate::sidebar;
use crate::theme::{Theme, duration, font, layout, text};
use crate::transcript::{RowKind, Status, Transcript};

actions!(
    workspace,
    [
        FocusNext,
        FocusPrevious,
        CopySelection,
        ToggleSearch,
        ToggleSettings
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
        KeyBinding::new(COPY_KEYS, CopySelection, None),
        KeyBinding::new(SEARCH_KEYS, ToggleSearch, None),
        KeyBinding::new(SETTINGS_KEYS, ToggleSettings, None),
    ]);
}

#[cfg(target_os = "macos")]
const COPY_KEYS: &str = "cmd-c";
#[cfg(not(target_os = "macos"))]
const COPY_KEYS: &str = "ctrl-c";
#[cfg(target_os = "macos")]
const SEARCH_KEYS: &str = "cmd-k";
#[cfg(not(target_os = "macos"))]
const SEARCH_KEYS: &str = "ctrl-k";
#[cfg(target_os = "macos")]
const SETTINGS_KEYS: &str = "cmd-,";
#[cfg(not(target_os = "macos"))]
const SETTINGS_KEYS: &str = "ctrl-,";

const HEAD: usize = 1;

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

struct RowText {
    document: Document,
    reveal: Reveal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Copied {
    Message(i64),
    Code(i64, usize),
}

/// The live end of a conversation, kept aside while the rows show an older
/// part of it. The events that arrive still go there.
struct Tail {
    transcript: Transcript,
    list: ListState,
    texts: HashMap<i64, RowText>,
    toggled_steps: HashSet<i64>,
    shown_outputs: HashSet<(i64, usize)>,
    step_lists: HashMap<i64, ScrollHandle>,
    loading_older: bool,
    older_error: Option<SharedString>,
}

struct OpenConversation {
    id: i64,
    title: Option<String>,
    transcript: Transcript,
    list: ListState,
    queue: QueueState,
    at_end: Rc<Cell<bool>>,
    turn_model: Option<Option<String>>,
    texts: HashMap<i64, RowText>,
    toggled_steps: HashSet<i64>,
    shown_outputs: HashSet<(i64, usize)>,
    step_lists: HashMap<i64, ScrollHandle>,
    answering: HashSet<String>,
    new_messages: usize,
    loading_older: bool,
    older_error: Option<SharedString>,
    flash: Option<(i64, Instant)>,
    /// With a value, the rows show a part of the history that stops before
    /// the live end, and this holds what the rows show otherwise.
    tail: Option<Box<Tail>>,
    /// The last event of the part on screen, when later ones are not loaded.
    newer: Option<i64>,
    loading_newer: bool,
    newer_failed: bool,
}

impl OpenConversation {
    /// Swaps what the rows show with the live end kept aside.
    fn swap_tail(&mut self) {
        let Some(tail) = self.tail.as_mut() else {
            return;
        };
        std::mem::swap(&mut self.transcript, &mut tail.transcript);
        std::mem::swap(&mut self.list, &mut tail.list);
        std::mem::swap(&mut self.texts, &mut tail.texts);
        std::mem::swap(&mut self.toggled_steps, &mut tail.toggled_steps);
        std::mem::swap(&mut self.shown_outputs, &mut tail.shown_outputs);
        std::mem::swap(&mut self.step_lists, &mut tail.step_lists);
        std::mem::swap(&mut self.loading_older, &mut tail.loading_older);
        std::mem::swap(&mut self.older_error, &mut tail.older_error);
    }

    /// Shows the live end again and drops the part of the history.
    fn restore_tail(&mut self) {
        self.swap_tail();
        self.tail = None;
        self.newer = None;
        self.loading_newer = false;
        self.newer_failed = false;
    }

    /// The events of the live end, whatever the rows show.
    fn live(&self) -> &Transcript {
        self.tail
            .as_ref()
            .map_or(&self.transcript, |tail| &tail.transcript)
    }

    fn distance_from_end(&self) -> f32 {
        f32::from(
            self.list.max_offset_for_scrollbar().y + self.list.scroll_px_offset_for_scrollbar().y,
        )
    }

    fn away_from_end(&self) -> bool {
        self.tail.is_some()
            || (!self.list.is_following_tail()
                && self.distance_from_end() > layout::STICK_THRESHOLD)
    }

    fn messages(&self) -> usize {
        let rows = self.transcript.rows().iter();
        rows.filter(|row| row.is_message()).count()
    }
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

/// What a dialog asks before the client sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    DeleteHistory(i64),
    FullAccess(i64),
}

/// A focus move decided where no window is at hand: the next frame makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusRequest {
    Composer,
    /// The creation form itself, so that no field looks focused.
    Creation,
    CreationName,
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
    editing: Option<i64>,
    selection: SharedSelection,
    copied: Option<Copied>,
    copied_reset: Option<Task<()>>,
    send_error: Option<SharedString>,
    unconfirmed: VecDeque<(i64, String)>,
    saved_draft: String,
    typed: String,
    draft_save: Option<Task<()>>,
    composer_text: Option<String>,
    confirm_full_access: Option<String>,
    clock: Rc<dyn Fn() -> Instant>,
    frames: Option<Rc<FrameLog>>,
    full_access_focus: FocusHandle,
    full_access_confirm_focus: FocusHandle,
    notice: Option<SharedString>,
    menu: Option<Menu>,
    menu_focus: FocusHandle,
    confirm_delete: Option<i64>,
    dialog_focus: FocusHandle,
    dialog_confirm_focus: FocusHandle,
    confirm: Option<Confirm>,
    confirm_focus: FocusHandle,
    confirm_ok_focus: FocusHandle,
    creation: Option<Creation>,
    creation_name: Entity<Composer>,
    creation_purpose: Entity<Composer>,
    creation_model: Entity<Composer>,
    panel: BotPanel,
    panel_name: Entity<Composer>,
    panel_title: Entity<Composer>,
    panel_instructions: Entity<Composer>,
    panel_model: Entity<Composer>,
    /// The bot updates that wait for the daemon's answer, oldest first, each
    /// with the bot and the field of the settings panel that sent it.
    updates: VecDeque<Option<(i64, PanelField)>>,
    settings: Option<SettingsPage>,
    settings_focus: FocusHandle,
    settings_query: Entity<Composer>,
    /// The search result of the settings page that Enter opens.
    settings_hit: usize,
    providers: Vec<ProviderStatus>,
    providers_asked: bool,
    /// What each provider offers, as the daemon last told it.
    models: HashMap<String, ProviderModels>,
    /// The providers asked for their models since the connection opened.
    models_asked: HashSet<String>,
    /// The providers whose sign-in this window started, until it opened the
    /// page where the person finishes it.
    sign_in_pages: HashSet<String>,
    provider_executable: Entity<Composer>,
    executable_edit: Option<ExecutableEdit>,
    daemon_info: Option<DaemonInfo>,
    palette: Option<Palette>,
    palette_query: Entity<Composer>,
    reveal: Option<RevealTarget>,
    focus_request: Option<FocusRequest>,
    side_panel: bool,
    hidden_open: bool,
    title_width: Rc<Cell<Pixels>>,
    inert: bool,
    retry_focus: FocusHandle,
    creation_focus: FocusHandle,
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
        let field = |label, placeholder: &'static str, limit, cx: &mut Context<Self>| {
            cx.new(|cx| {
                Composer::new(placeholder, InputStyle::field(label), cx).with_max_length(limit)
            })
        };
        let area = |label, placeholder: &'static str, rows: (f32, f32), cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input =
                    Composer::new(placeholder, InputStyle::area(label, rows.0, rows.1), cx)
                        .with_max_length(BOT_INSTRUCTIONS_LENGTH);
                input.set_enter_submits(false);
                input
            })
        };
        let creation_name = cx.new(|cx| {
            Composer::new("Name your bot", InputStyle::form_field("Name"), cx)
                .with_max_length(BOT_NAME_LENGTH)
        });
        let creation_purpose = cx.new(|cx| {
            let mut input = Composer::new(
                "Research a topic, review code, or draft a weekly update.",
                InputStyle::form_area(
                    "Purpose",
                    layout::PURPOSE_MIN_ROWS,
                    layout::PURPOSE_MAX_ROWS,
                ),
                cx,
            )
            .with_max_length(BOT_INSTRUCTIONS_LENGTH);
            input.set_enter_submits(false);
            input
        });
        let creation_model = field("Search models", "Search models", BOT_MODEL_LENGTH, cx);
        let provider_executable = field(
            "Executable path",
            "Full path of the executable",
            EXECUTABLE_LENGTH,
            cx,
        );
        let panel_name = field("Bot name", "Name", BOT_NAME_LENGTH, cx);
        let panel_title = field(
            "Bot title",
            "A short role, such as Researcher",
            BOT_TITLE_LENGTH,
            cx,
        );
        let panel_instructions = area(
            "Bot instructions",
            "What this bot does, and how",
            (layout::INSTRUCTIONS_MIN_ROWS, layout::INSTRUCTIONS_MAX_ROWS),
            cx,
        );
        let panel_model = field("Bot model", "Default model", BOT_MODEL_LENGTH, cx);
        let palette_query = field(
            "Search everything",
            "Search bots, conversations, and messages",
            SEARCH_QUERY_LENGTH,
            cx,
        );
        let settings_query = field(
            "Search settings",
            "Search settings",
            SETTINGS_QUERY_LENGTH,
            cx,
        );
        let mut subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event: &ComposerEvent, window, cx| {
                    if matches!(event, ComposerEvent::Submit) {
                        this.send_message(window, cx);
                    }
                },
            ),
            cx.observe(&composer, |this, _, cx| this.schedule_draft_save(cx)),
            cx.observe_window_appearance(window, |this, _, cx| this.apply_theme(cx)),
            cx.on_app_quit(|this, cx| {
                this.save_draft();
                this.flush_panel(cx);
                cx.background_executor().timer(duration::QUIT_FLUSH)
            }),
            cx.subscribe_in(
                &creation_name,
                window,
                |this, _, event: &ComposerEvent, window, cx| {
                    if matches!(event, ComposerEvent::Submit) {
                        this.create_bot(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &creation_model,
                window,
                |this, _, event: &ComposerEvent, window, cx| {
                    if matches!(event, ComposerEvent::Submit) {
                        this.submit_model_search(window, cx);
                    }
                },
            ),
            cx.observe(&creation_model, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &provider_executable,
                window,
                |this, _, event: &ComposerEvent, _, cx| {
                    if matches!(event, ComposerEvent::Submit) {
                        this.save_executable(cx);
                    }
                },
            ),
            cx.observe(&creation_name, |this, _, cx| {
                if let Some(creation) = this.creation.as_mut() {
                    creation.name_error = None;
                }
                cx.notify();
            }),
            cx.subscribe_in(
                &palette_query,
                window,
                |this, _, event: &ComposerEvent, window, cx| match event {
                    ComposerEvent::Submit => this.open_active_hit(window, cx),
                    ComposerEvent::Step(forward) => this.palette_step(*forward, cx),
                },
            ),
            cx.observe(&palette_query, |this, _, cx| this.palette_typed(cx)),
            cx.subscribe_in(
                &settings_query,
                window,
                |this, _, event: &ComposerEvent, window, cx| match event {
                    ComposerEvent::Submit => this.open_active_setting(window, cx),
                    ComposerEvent::Step(forward) => this.settings_step(*forward, cx),
                },
            ),
            cx.observe(&settings_query, |this, _, cx| {
                this.settings_hit = 0;
                cx.notify();
            }),
        ];
        for (input, field) in [
            (&panel_name, PanelField::Name),
            (&panel_title, PanelField::Title),
            (&panel_instructions, PanelField::Instructions),
            (&panel_model, PanelField::Model),
        ] {
            subscriptions.push(cx.observe(input, move |this, _, cx| this.panel_edited(field, cx)));
            let handle = input.focus_handle(cx);
            subscriptions.push(cx.on_blur(&handle, window, move |this, window, cx| {
                this.save_panel_field(field, Some(window), cx);
            }));
        }
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
            editing: None,
            selection: SharedSelection::default(),
            copied: None,
            copied_reset: None,
            send_error: None,
            unconfirmed: VecDeque::new(),
            saved_draft: String::new(),
            typed: String::new(),
            draft_save: None,
            composer_text: None,
            confirm_full_access: None,
            clock: Rc::new(Instant::now),
            frames: FrameLog::from_environment(),
            full_access_focus: cx.focus_handle(),
            full_access_confirm_focus: cx.focus_handle(),
            notice: None,
            menu: None,
            menu_focus: cx.focus_handle(),
            confirm_delete: None,
            dialog_focus: cx.focus_handle(),
            dialog_confirm_focus: cx.focus_handle(),
            confirm: None,
            confirm_focus: cx.focus_handle(),
            confirm_ok_focus: cx.focus_handle(),
            creation: None,
            creation_name,
            creation_purpose,
            creation_model,
            panel: BotPanel::default(),
            panel_name,
            panel_title,
            panel_instructions,
            panel_model,
            updates: VecDeque::new(),
            settings: None,
            settings_focus: cx.focus_handle(),
            settings_query,
            settings_hit: 0,
            providers: Vec::new(),
            providers_asked: false,
            models: HashMap::new(),
            models_asked: HashSet::new(),
            sign_in_pages: HashSet::new(),
            provider_executable,
            executable_edit: None,
            daemon_info: None,
            palette: None,
            palette_query,
            reveal: None,
            focus_request: None,
            side_panel: false,
            hidden_open: false,
            title_width: Rc::new(Cell::new(Pixels::MAX)),
            inert: false,
            retry_focus: cx.focus_handle(),
            creation_focus: cx.focus_handle(),
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
            self.confirm_full_access = None;
            self.confirm = None;
            self.palette = None;
            self.link_lost();
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
                self.providers_asked = false;
                self.models_asked.clear();
                // The rail shows the state of every provider.
                self.ask_providers();
                if self.settings.is_some() {
                    self.connection.send(ClientMessage::GetDaemonInfo);
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
                self.creation_follows_bots();
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
                self.creation_yields(cx);
                if self.selected_bot.is_none() {
                    self.select_bot(id);
                }
            }
            ServerMessage::BotCreated { bot_id } => {
                if self
                    .creation
                    .as_ref()
                    .is_some_and(|creation| creation.creating)
                {
                    self.creation = None;
                    self.focus_request = Some(FocusRequest::Composer);
                }
                self.select_bot(bot_id);
            }
            ServerMessage::BotUpdated { .. } => {
                if let Some(Some((bot_id, field))) = self.updates.pop_front() {
                    self.panel_saved(bot_id, field);
                }
            }
            ServerMessage::HistoryDeleted {
                bot_id,
                current_conversation_id,
            } => {
                if let Some(known) = self.bots.iter_mut().find(|known| known.bot.id == bot_id) {
                    known.current_conversation_id = current_conversation_id;
                }
                if self.selected_bot == Some(bot_id) {
                    // The conversation on screen is gone: there is nothing to close.
                    self.selected = None;
                    self.open = None;
                    self.history = None;
                    self.reveal = None;
                    self.select(current_conversation_id);
                }
            }
            ServerMessage::Providers(providers) => self.providers = providers,
            ServerMessage::ProviderChanged(status) => {
                self.executable_applied(&status);
                // The window that started a sign-in opens its page once; the
                // others show a button for it.
                if let Some(page) = &status.sign_in_url
                    && self.sign_in_pages.remove(&status.id)
                {
                    self.open_link(page, cx);
                }
                if !status.signing_in {
                    self.sign_in_pages.remove(&status.id);
                }
                let known = self
                    .providers
                    .iter_mut()
                    .find(|known| known.id == status.id);
                // A provider that changed state or program may list other
                // models: it is asked again when a picker shows them.
                let same = known.as_ref().is_some_and(|known| {
                    known.state == status.state && known.executable == status.executable
                });
                if !same {
                    self.models_asked.remove(&status.id);
                }
                match known {
                    Some(known) => *known = status,
                    None => self.providers.push(status),
                }
            }
            ServerMessage::ProviderModels(models) => {
                self.models.insert(models.provider.clone(), models);
            }
            ServerMessage::DaemonInfo(info) => self.daemon_info = Some(info),
            ServerMessage::SearchResults(results) => self.palette_results(results),
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
                if self.confirm == Some(Confirm::DeleteHistory(bot_id))
                    || self.confirm == Some(Confirm::FullAccess(bot_id))
                {
                    self.confirm = None;
                }
                self.creation_follows_bots();
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
                let reopened = self.open.as_ref().is_some_and(|open| open.id == id);
                let title = snapshot.conversation.title.clone();
                let queue = snapshot.queue.clone();
                let turn_model = snapshot.turn_model.clone();
                let draft = snapshot.draft.clone();
                let transcript = Transcript::from_snapshot(snapshot);
                let list = ListState::new(
                    transcript.rows().len() + HEAD,
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
                    texts: HashMap::new(),
                    toggled_steps: HashSet::new(),
                    shown_outputs: HashSet::new(),
                    step_lists: HashMap::new(),
                    answering: HashSet::new(),
                    new_messages: 0,
                    loading_older: false,
                    older_error: None,
                    flash: None,
                    tail: None,
                    newer: None,
                    loading_newer: false,
                    newer_failed: false,
                });
                self.selection.take();
                self.confirm_full_access = None;
                if !reopened {
                    self.saved_draft.clone_from(&draft);
                    self.typed.clone_from(&draft);
                    self.composer_text = Some(draft);
                }
                self.load_error = None;
                self.conversation_shown();
                self.forget_started_edit();
                self.continue_reveal();
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
                at_ms,
            } => self.apply(conversation_id, event, at_ms),
            ServerMessage::OlderEvents {
                conversation_id,
                events,
                older,
            } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) {
                    let top = open.list.logical_scroll_top();
                    let first = top.item_ix.saturating_sub(HEAD);
                    let rows = open.transcript.rows().iter().enumerate();
                    let anchor = rows
                        .skip(first)
                        .find(|(_, row)| !matches!(row.kind, RowKind::Day))
                        .map(|(index, row)| (index, index + HEAD == top.item_ix, row.key));
                    let added = open.transcript.prepend(events, older);
                    open.list.splice(HEAD..HEAD, added);
                    open.list.remeasure();
                    let rows = open.transcript.rows();
                    if let Some((old_index, same_row, key)) = anchor
                        && !open.list.is_following_tail()
                    {
                        // A row that the older page completes (the steps or the
                        // answer of a turn the page cut) takes a new key: it
                        // still sits as far from the end as before.
                        let index = rows
                            .iter()
                            .position(|row| row.key == key)
                            .unwrap_or(old_index + added)
                            .min(rows.len().saturating_sub(1));
                        open.list.scroll_to(ListOffset {
                            item_ix: HEAD + index,
                            offset_in_item: if same_row {
                                top.offset_in_item
                            } else {
                                Pixels::ZERO
                            },
                        });
                    }
                    open.loading_older = false;
                    open.older_error = None;
                    self.continue_reveal();
                }
            }
            ServerMessage::OlderEventsFailed {
                conversation_id,
                message,
            } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) {
                    open.loading_older = false;
                    open.older_error = Some(message.into());
                    open.list.remeasure_items(0..HEAD);
                    self.reveal = None;
                }
            }
            ServerMessage::EventsWindow {
                conversation_id,
                events,
                older,
                newer,
            } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) {
                    open.restore_tail();
                    open.loading_older = false;
                    open.older_error = None;
                    open.list.remeasure_items(0..HEAD);
                    open.list.set_follow_mode(FollowMode::Tail);
                    let transcript = Transcript::from_events(events, older);
                    let list = ListState::new(
                        transcript.rows().len() + HEAD,
                        ListAlignment::Top,
                        layout::LIST_OVERDRAW,
                    );
                    open.tail = Some(Box::new(Tail {
                        transcript,
                        list,
                        texts: HashMap::new(),
                        toggled_steps: HashSet::new(),
                        shown_outputs: HashSet::new(),
                        step_lists: HashMap::new(),
                        loading_older: false,
                        older_error: None,
                    }));
                    open.swap_tail();
                    open.newer = newer;
                    self.selection.take();
                    self.continue_reveal();
                }
            }
            ServerMessage::NewerEvents {
                conversation_id,
                events,
                newer,
            } => {
                let Some(open) = self.open.as_mut().filter(|open| {
                    open.id == conversation_id && open.tail.is_some() && open.loading_newer
                }) else {
                    return;
                };
                open.loading_newer = false;
                open.newer = newer;
                // The page may complete the turn that the part on screen cut:
                // the rows are built again, and the list keeps the ones that
                // did not change.
                let top = open.list.logical_scroll_top();
                let older = open.transcript.older();
                let mut previous = std::mem::take(&mut open.transcript);
                let mut all = previous.take_events();
                all.extend(events);
                open.transcript = Transcript::from_events(all, older);
                let rows = open.transcript.rows();
                let same = previous
                    .rows()
                    .iter()
                    .zip(rows)
                    .take_while(|(before, after)| before == after)
                    .count();
                for row in &rows[same..] {
                    open.texts.remove(&row.key);
                }
                open.list
                    .splice(HEAD + same..HEAD + previous.rows().len(), rows.len() - same);
                if top.item_ix >= HEAD + same {
                    open.list.scroll_to(top);
                }
                if newer.is_none() {
                    self.join_tail();
                }
            }
            ServerMessage::NewerEventsFailed {
                conversation_id,
                message,
            } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) {
                    open.loading_newer = false;
                    open.newer_failed = true;
                    self.notice =
                        Some(format!("The later messages were not loaded: {message}").into());
                }
            }
            ServerMessage::MessageAccepted { conversation_id } => {
                // Two conversations answer in no fixed order: the accepted
                // message is the oldest one sent to this conversation.
                if let Some(index) = self
                    .unconfirmed
                    .iter()
                    .position(|(sent_to, _)| *sent_to == conversation_id)
                {
                    self.unconfirmed.remove(index);
                }
            }
            ServerMessage::MessageRefused {
                conversation_id,
                message,
            } => {
                let refused = self
                    .unconfirmed
                    .iter()
                    .position(|(sent_to, _)| *sent_to == conversation_id)
                    .and_then(|index| self.unconfirmed.remove(index));
                if let Some((_, text)) = refused {
                    self.give_back(text);
                }
                self.send_error = Some(message.into());
            }
            ServerMessage::ApprovalNotResolved {
                conversation_id,
                approval_id,
                message,
            } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) {
                    open.answering.remove(&approval_id);
                    open.list.remeasure();
                }
                self.notice = Some(format!("The answer was not sent: {message}").into());
            }
            ServerMessage::TurnModel {
                conversation_id,
                model,
            } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) {
                    open.turn_model = Some(model);
                }
            }
            ServerMessage::Error { message } => self.refused(None, message),
            ServerMessage::InvalidField { field, message } => self.refused(Some(&field), message),
        }
    }

    /// The daemon did not do what was asked. The refusal goes to the form
    /// that waits for an answer, when one does.
    fn refused(&mut self, field: Option<&str>, message: String) {
        if field == Some("executable") {
            match self.executable_edit.as_mut() {
                Some(edit) => {
                    edit.saving = false;
                    edit.error = Some(message.into());
                }
                None => self.fail(message),
            }
            return;
        }
        if let Some(creation) = self.creation.as_mut().filter(|creation| creation.creating) {
            creation.creating = false;
            if field == Some("name") {
                creation.name_error = Some(message.into());
                self.focus_request = Some(FocusRequest::CreationName);
            } else {
                creation.error = Some(message.into());
            }
            return;
        }
        if field != Some("sidebar_width")
            && let Some(Some((bot_id, field))) = self.updates.pop_front()
        {
            self.panel_refused(bot_id, field, message);
            return;
        }
        self.fail(message);
    }

    /// Sends a change to a bot. The daemon answers each one, in order.
    fn send_update(&mut self, bot_id: i64, patch: BotPatch, field: Option<PanelField>) {
        self.updates.push_back(field.map(|field| (bot_id, field)));
        self.connection
            .send(ClientMessage::UpdateBot { bot_id, patch });
    }

    /// Asks for the state of the providers, once per connection.
    fn ask_providers(&mut self) {
        if self.live() && !self.providers_asked {
            self.providers_asked = true;
            self.connection.send(ClientMessage::ListProviders);
        }
    }

    /// Asks a provider for its models, once per connection and per state of
    /// the provider.
    fn ask_models(&mut self, provider: &str) {
        if self.live() && !provider.is_empty() && self.models_asked.insert(provider.to_owned()) {
            self.connection.send(ClientMessage::ListModels {
                provider: provider.to_owned(),
            });
        }
    }

    /// Asks for what the pickers on screen show, and for the state of the
    /// provider of a turn that failed for the lack of a sign-in.
    fn sync_models(&mut self) {
        if self.creation.is_some() {
            let provider = self.creation_provider();
            self.ask_models(&provider);
            // The picker lists the models of the tab it shows.
            let tab = self.picker_tab();
            self.ask_models(&tab);
        } else if self.side_panel
            && let Some(provider) = self.selected_bot().map(|bot| bot.bot.provider.clone())
        {
            self.ask_models(&provider);
        }
        let sign_in_needed = self.open.as_ref().is_some_and(|open| {
            matches!(
                open.transcript.rows().last().map(|row| &row.kind),
                Some(RowKind::Status(Status::Failed {
                    cause: Some(FailureCause::SignInRequired),
                    ..
                }))
            )
        });
        if sign_in_needed {
            self.ask_providers();
        }
    }

    fn loading_conversation(&self) -> bool {
        self.selected.is_some() && self.open.is_none() && self.load_error.is_none()
    }

    fn give_back(&mut self, text: String) {
        let typed = self
            .composer_text
            .clone()
            .unwrap_or_else(|| self.typed.clone());
        self.composer_text = Some(if typed.is_empty() {
            text
        } else {
            format!("{text}\n{typed}")
        });
    }

    fn link_lost(&mut self) {
        self.updates.clear();
        self.panel_link_lost();
        self.reveal = None;
        if let Some(creation) = self.creation.as_mut().filter(|creation| creation.creating) {
            creation.creating = false;
            creation.error = Some("The bot was not created: the daemon cannot be reached.".into());
        }
        let open_id = self.open.as_ref().map(|open| open.id);
        let unsent: Vec<String> = std::mem::take(&mut self.unconfirmed)
            .into_iter()
            .filter(|(sent_to, _)| Some(*sent_to) == open_id)
            .map(|(_, text)| text)
            .collect();
        if !unsent.is_empty() {
            self.composer_text = Some(unsent.join("\n"));
            self.send_error =
                Some("The message was not sent: the daemon cannot be reached.".into());
        }
        if let Some(open) = self.open.as_mut() {
            open.loading_older = false;
            if !open.answering.is_empty() {
                open.answering.clear();
                open.list.remeasure();
                self.notice = Some("The answer was not sent: the daemon cannot be reached.".into());
            }
        }
    }

    fn fail(&mut self, message: String) {
        if let Some(open) = self.open.as_mut()
            && !open.answering.is_empty()
        {
            open.answering.clear();
            open.list.remeasure();
        }
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
        let first = sidebar::groups(&self.bots)
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

    /// An event of the open conversation arrived. While the rows show an
    /// older part of the history, it goes to the live end kept aside.
    fn apply(&mut self, conversation_id: i64, event: Event, at_ms: i64) {
        let hidden = self
            .open
            .as_mut()
            .filter(|open| open.id == conversation_id)
            .is_some_and(|open| {
                open.swap_tail();
                open.tail.is_some()
            });
        self.apply_to_end(conversation_id, event, at_ms, hidden);
        if hidden && let Some(open) = self.open.as_mut() {
            open.swap_tail();
        }
    }

    fn apply_to_end(&mut self, conversation_id: i64, event: Event, at_ms: i64, hidden: bool) {
        let Some(open) = self.open.as_mut().filter(|open| open.id == conversation_id) else {
            return;
        };
        let now = (self.clock)();
        let marks_read = bots::marks_read(&event);
        let ends = event.ends_turn();
        if ends {
            open.turn_model = None;
        }
        let streamed = match &event {
            Event::MessageText(text) => Some(text.len()),
            _ => None,
        };
        match &event {
            Event::ApprovalResolved { id: Some(id), .. } | Event::ApprovalClosed { id, .. } => {
                open.answering.remove(id);
                // A request that was settled elsewhere no longer backs the
                // confirmation to move the bot to Full access.
                if self.confirm_full_access.as_ref() == Some(id) {
                    self.confirm_full_access = None;
                }
            }
            _ => {}
        }
        if ends {
            self.confirm_full_access = None;
        }
        let away = (hidden || !open.list.is_following_tail()).then(|| open.messages());
        let change = open.transcript.apply(TimedEvent {
            id: 0,
            at_ms,
            event,
        });
        if change.first < change.before {
            open.list
                .remeasure_items(change.first + HEAD..change.before + HEAD);
        }
        if change.after > change.before {
            let end = change.before + HEAD;
            open.list.splice(end..end, change.after - change.before);
        }
        if let Some(length) = streamed {
            let rows = open.transcript.rows().iter().rev();
            let answer = rows.take_while(|row| !matches!(row.kind, RowKind::User { .. }));
            let streaming = answer.into_iter().find_map(|row| match &row.kind {
                RowKind::Answer { text, .. } => Some((row.key, text.len())),
                _ => None,
            });
            if let Some((key, received)) = streaming {
                let text = open.texts.entry(key).or_insert_with(|| RowText {
                    document: Document::default(),
                    reveal: Reveal::shown_from(received.saturating_sub(length), now),
                });
                text.reveal.receive(received, now);
            }
        }
        if ends {
            for text in open.texts.values_mut() {
                text.reveal.finish(now);
            }
            let rows = open.transcript.rows();
            for row in &rows[change.first.min(rows.len())..] {
                open.toggled_steps.remove(&row.key);
            }
        }
        if let Some(before) = away {
            if !hidden && open.distance_from_end() <= layout::STICK_THRESHOLD {
                open.list.set_follow_mode(FollowMode::Tail);
                if !open.at_end.replace(true) && self.history.is_none() {
                    self.connection.send(ClientMessage::MarkRead {
                        conversation_id,
                        following: true,
                    });
                }
            } else {
                open.new_messages += open.messages().saturating_sub(before);
            }
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
            self.save_draft();
            self.connection.send(ClientMessage::CloseConversation {
                conversation_id: previous,
            });
        }
        self.selected = Some(conversation_id);
        self.open = None;
        self.load_error = None;
        self.editing = None;
        self.send_error = None;
        self.confirm_full_access = None;
        self.composer_text = Some(String::new());
        self.saved_draft.clear();
        self.typed.clear();
        self.selection.take();
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

    fn schedule_draft_save(&mut self, cx: &mut Context<Self>) {
        self.typed = self.composer.read(cx).text().to_owned();
        self.draft_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration::DRAFT_SAVE).await;
            this.update(cx, |this, _| this.save_draft()).ok();
        }));
        cx.notify();
    }

    fn save_draft(&mut self) {
        self.draft_save = None;
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if !self.live()
            || self.editing.is_some()
            || self.typed == self.saved_draft
            || self.typed.chars().count() > MESSAGE_LENGTH
        {
            return;
        }
        self.saved_draft.clone_from(&self.typed);
        self.connection.send(ClientMessage::SetDraft {
            conversation_id: open.id,
            text: self.typed.clone(),
        });
    }

    fn load_older(&mut self) {
        let live = self.live();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if let Some(before) = open.transcript.older()
            && live
            && !open.loading_older
        {
            open.loading_older = true;
            open.older_error = None;
            open.list.remeasure_items(0..HEAD);
            self.connection.send(ClientMessage::LoadOlderEvents {
                conversation_id: open.id,
                before,
            });
        }
    }

    /// Asks for the events that follow the part of the history on screen.
    pub(super) fn load_newer(&mut self) {
        let live = self.live();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let until = open
            .tail
            .as_ref()
            .and_then(|tail| tail.transcript.first_event_id());
        if let (Some(after), Some(until)) = (open.newer, until)
            && live
            && !open.loading_newer
            && !open.newer_failed
        {
            open.loading_newer = true;
            self.connection.send(ClientMessage::LoadNewerEvents {
                conversation_id: open.id,
                after,
                until,
            });
        }
    }

    /// The part of the history on screen reached the live end: both become
    /// one, and the rows on screen stay where they are.
    fn join_tail(&mut self) {
        let Some(open) = self.open.as_mut().filter(|open| open.tail.is_some()) else {
            return;
        };
        let top = open.list.logical_scroll_top();
        let older = open.transcript.older();
        let events = open.transcript.take_events();
        open.restore_tail();
        let added = open.transcript.prepend(events, older);
        open.list.splice(HEAD..HEAD, added);
        open.list.remeasure();
        open.list.set_follow_mode(FollowMode::Normal);
        open.list.scroll_to(top);
        open.flash = None;
        self.selection.take();
    }

    fn follow_scroll(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.list.is_following_tail() {
            open.new_messages = 0;
        }
        let fits = open.list.viewport_bounds().size.height > Pixels::ZERO
            && open.list.max_offset_for_scrollbar().y <= Pixels::ZERO;
        let at_top =
            !open.list.is_following_tail() && open.list.logical_scroll_top().item_ix < HEAD;
        if (at_top || fits) && open.older_error.is_none() {
            self.load_older();
        }
    }

    fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut() {
            if open.tail.is_some() {
                open.restore_tail();
                open.flash = None;
                self.selection.take();
                self.reveal = None;
            }
            open.list.set_follow_mode(FollowMode::Tail);
            open.new_messages = 0;
            if !open.at_end.replace(true) && self.history.is_none() {
                self.connection.send(ClientMessage::MarkRead {
                    conversation_id: open.id,
                    following: true,
                });
            }
            cx.notify();
        }
    }

    fn toggle_steps(&mut self, key: i64, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut() {
            if !open.toggled_steps.remove(&key) {
                open.toggled_steps.insert(key);
            }
            open.list.remeasure();
            cx.notify();
        }
    }

    fn toggle_output(&mut self, key: i64, step: usize, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut() {
            if !open.shown_outputs.remove(&(key, step)) {
                open.shown_outputs.insert((key, step));
            }
            open.list.remeasure();
            cx.notify();
        }
    }

    fn copy(&mut self, what: Copied, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(what);
        self.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration::COPIED).await;
            this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn copy_message(&mut self, key: i64, cx: &mut Context<Self>) {
        let source = self.open.as_ref().and_then(|open| {
            let row = open.transcript.rows().iter().find(|row| row.key == key)?;
            match &row.kind {
                RowKind::User { text }
                | RowKind::Answer { text, .. }
                | RowKind::Thought { text } => Some(text.trim_matches('\n').to_owned()),
                _ => None,
            }
        });
        if let Some(source) = source {
            self.copy(Copied::Message(key), source, cx);
        }
    }

    fn selected_text(&self) -> Option<String> {
        let selection = (*self.selection.borrow())?;
        let rendered = self
            .open
            .as_ref()?
            .texts
            .get(&selection.key)?
            .document
            .text();
        let clamp = |offset: usize| {
            let mut offset = offset.min(rendered.len());
            while !rendered.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        };
        let range = selection.range();
        let (start, end) = (clamp(range.start), clamp(range.end));
        (start < end).then(|| rendered[start..end].to_owned())
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn open_link(&mut self, target: &str, cx: &mut Context<Self>) {
        let scheme = target
            .split_once(':')
            .map(|(scheme, _)| scheme.to_ascii_lowercase());
        if matches!(scheme.as_deref(), Some("http" | "https" | "mailto")) {
            cx.open_url(target);
        } else {
            self.notice = Some(format!("This link was not opened: {target}").into());
        }
        cx.notify();
    }

    fn can_stop(&self, cx: &App) -> bool {
        self.live()
            && self
                .open
                .as_ref()
                .is_some_and(|open| open.live().turn_active())
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
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let text = self.composer.read(cx).text().to_owned();
        let length = text.chars().count();
        if length > MESSAGE_LENGTH {
            self.send_error = Some(
                format!(
                    "A message can hold 100,000 characters at most. This one has {}.",
                    grouped(length)
                )
                .into(),
            );
            cx.notify();
            return;
        }
        self.connection.send(match self.editing.take() {
            Some(queue_id) => ClientMessage::EditQueuedMessage {
                conversation_id: open.id,
                queue_id,
                text,
            },
            None => {
                self.unconfirmed.push_back((open.id, text.clone()));
                self.saved_draft.clear();
                ClientMessage::SendMessage {
                    conversation_id: open.id,
                    text,
                }
            }
        });
        if open.tail.is_some() {
            // A message sent from a part of the history shows at the live end.
            open.restore_tail();
            open.flash = None;
            self.selection.take();
            self.reveal = None;
            if !open.at_end.replace(true) && self.history.is_none() {
                self.connection.send(ClientMessage::MarkRead {
                    conversation_id: open.id,
                    following: true,
                });
            }
        }
        open.list.set_follow_mode(FollowMode::Tail);
        open.new_messages = 0;
        self.notice = None;
        self.send_error = None;
        self.composer
            .update(cx, |composer, cx| composer.clear(window, cx));
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn open_bot(&mut self, bot_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.creation = None;
        self.select_bot(bot_id);
        self.focus_composer(window, cx);
        cx.notify();
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
        let live = self.live();
        if let Some(open) = self.open.as_mut()
            && live
            && open.answering.insert(approval_id.clone())
        {
            open.list.remeasure();
            self.connection.send(ClientMessage::ResolveApproval {
                conversation_id: open.id,
                approval_id,
                decision,
            });
        }
    }

    fn ask_full_access(
        &mut self,
        approval_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.live() {
            self.confirm_full_access = Some(approval_id);
            window.focus(&self.full_access_focus, cx);
            cx.notify();
        }
    }

    fn cancel_full_access(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_full_access.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    fn full_access_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(approval_id) = self.confirm_full_access.take() else {
            return;
        };
        self.focus_composer(window, cx);
        cx.notify();
        let (Some(bot_id), true) = (self.selected_bot, self.live()) else {
            return;
        };
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.answering.insert(approval_id) {
            open.list.remeasure();
            self.send_update(
                bot_id,
                BotPatch {
                    permission_level: Some(PermissionLevel::FullAccess),
                    ..BotPatch::default()
                },
                None,
            );
        }
    }

    fn pending_model(&self) -> Option<SharedString> {
        let open = self.open.as_ref()?;
        let running = open
            .turn_model
            .as_ref()
            .filter(|_| open.live().turn_active())?;
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
                self.creation = None;
                self.select_bot(bot_id);
                self.side_panel = true;
                self.ask_providers();
            }
            MenuAction::Duplicate(bot_id) => {
                self.connection.send(ClientMessage::DuplicateBot { bot_id });
            }
            MenuAction::SetPinned(bot_id, pinned) => {
                if self.change_bot(bot_id, |summary| summary.bot.pinned = pinned) {
                    self.send_update(
                        bot_id,
                        BotPatch {
                            pinned: Some(pinned),
                            ..BotPatch::default()
                        },
                        None,
                    );
                }
            }
            MenuAction::SetHidden(bot_id, hidden) => {
                if self.change_bot(bot_id, |summary| summary.bot.hidden = hidden) {
                    self.send_update(
                        bot_id,
                        BotPatch {
                            hidden: Some(hidden),
                            ..BotPatch::default()
                        },
                        None,
                    );
                }
            }
            MenuAction::Delete(bot_id) => {
                if self.bot(bot_id).is_some() {
                    self.confirm_delete = Some(bot_id);
                    window.focus(&self.dialog_focus, cx);
                }
            }
            MenuAction::NewBot => self.open_creation(window, cx),
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
        if self.side_panel {
            self.ask_providers();
        }
        cx.notify();
    }

    fn cancel_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    fn confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        self.focus_composer(window, cx);
        cx.notify();
        if !self.live() {
            return;
        }
        match confirm {
            Confirm::DeleteHistory(bot_id) => self.delete_history(bot_id),
            Confirm::FullAccess(bot_id) => self.save_level(bot_id, PermissionLevel::FullAccess),
        }
    }
}

fn grouped(number: usize) -> String {
    let digits = number.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
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
        if let Some(frames) = &self.frames {
            frames.start();
        }
        if self.resizing && !cx.has_active_drag() {
            self.finish_resize();
        }
        if let Some(text) = self.composer_text.take() {
            self.composer
                .update(cx, |composer, cx| composer.set_text(&text, window, cx));
        }
        self.sync_panel(window, cx);
        self.sync_models();
        match self.focus_request.take() {
            Some(FocusRequest::Composer) => self.focus_composer(window, cx),
            Some(FocusRequest::Creation) => window.focus(&self.creation_focus, cx),
            Some(FocusRequest::CreationName) => {
                window.focus(&self.creation_name.focus_handle(cx), cx);
            }
            None => {}
        }
        self.highlight_revealed(window);
        let enter_sends = self.preferences.send_shortcut == SendShortcut::Enter;
        self.composer
            .update(cx, |composer, _| composer.set_enter_submits(enter_sends));
        self.follow_scroll();
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
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &ToggleSearch, window, cx| {
                this.toggle_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSettings, window, cx| {
                this.toggle_settings(window, cx);
            }))
            .on_action(cx.listener(|this, _: &composer::Copy, _, cx| this.copy_selection(cx)))
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                if this.selection.take().is_some() {
                    cx.notify();
                }
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
        let shell = self.screen() == Screen::Shell;
        let root = match self.screen() {
            Screen::Shell => self.render_shell(root, &theme, window, cx),
            screen => root.child(self.render_screen(&screen, &theme, window, cx)),
        };
        let root = root
            .children(self.render_floating_menu(&theme, cx))
            .children(self.render_delete_dialog(&theme, cx))
            .children(self.render_full_access_dialog(&theme, cx))
            .children(self.render_confirm_dialog(&theme, cx))
            .children(
                shell
                    .then(|| self.render_settings_select(&theme, window, cx))
                    .flatten(),
            )
            .children(
                shell
                    .then(|| self.render_palette(&theme, window, cx))
                    .flatten(),
            )
            .children(self.render_toast(&theme, cx))
            .into_any_element();
        match &self.frames {
            Some(log) => frames::timed(root, log.clone()).into_any_element(),
            None => root,
        }
    }
}

impl Workspace {
    fn focus_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.close_menu(window, cx);
        } else if self.palette.is_some() {
            window.focus(&self.palette_query.focus_handle(cx), cx);
        } else if self.settings.is_some() {
            // The settings page takes the focus while the daemon is away
            // too: its sections can still be read.
            self.close_settings_select(cx);
            if forward {
                window.focus_next(cx);
            } else {
                window.focus_prev(cx);
            }
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
        } else if self.confirm.is_some() {
            let cancel = self.confirm_focus.is_focused(window);
            let next = if cancel {
                &self.confirm_ok_focus
            } else {
                &self.confirm_focus
            };
            window.focus(next, cx);
        } else if self.confirm_full_access.is_some() {
            let cancel = self.full_access_focus.is_focused(window);
            let next = if cancel {
                &self.full_access_confirm_focus
            } else {
                &self.full_access_focus
            };
            window.focus(next, cx);
        } else if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
    }
}
