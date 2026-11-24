use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crowbot_protocol::{
    ApprovalClosure, ApprovalDecision, ApprovalReason, Bot, ConversationSnapshot, Event,
    FailureCause, Interruption, MESSAGE_LENGTH, PermissionLevel, QueueState, ServerMessage,
    TurnEnd, TurnOrigin, UserMessage,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use crate::daemon::Daemon;
use crate::guard::{self, INACTIVITY_LIMIT, Progress, RETRY_DELAYS};
use crate::handoff::{self, HANDOFF_CHARACTERS};
use crate::mcp::{SessionEndpoint, ToolSignal};
use crate::prompt::Sections;
use crate::provider::acp::{
    self, AgentEvent, ApprovalAnswer, SessionCommand, SessionConfig, SessionFailure,
};
use crate::store::{
    BotRow, Conversation, EventsPage, ProviderSession, Recorder, Reported, SharedStore, StoreError,
    Tokens, session_fingerprint,
};
use crate::tools::{Outputs, SessionKind, ToolContext, TurnGate};

/// How long an agent gets to end its turn after a stop, before its process is closed.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const PAGE_EVENTS: usize = 200;
/// How many events a client gets in one answer so that an event it asked for
/// joins the ones it holds. Past that, it gets a page around the event.
const JOINED_EVENTS: usize = 5 * PAGE_EVENTS;

pub type Sink = UnboundedSender<ServerMessage>;

pub enum Command {
    Subscribe {
        connection: u64,
        sink: Sink,
    },
    Unsubscribe {
        connection: u64,
    },
    Prompt {
        text: String,
        sink: Sink,
    },
    LoadOlder {
        before: i64,
        sink: Sink,
    },
    /// A client wants an event that it has not loaded. It gets the events from
    /// the start of that event's turn up to the oldest one it holds, or a page
    /// around the event when too many lie between.
    Reveal {
        event_id: i64,
        before: i64,
        sink: Sink,
    },
    LoadNewer {
        after: i64,
        until: i64,
        sink: Sink,
    },
    SetDraft {
        text: String,
        sink: Sink,
    },
    ResolveApproval {
        approval_id: String,
        decision: ApprovalDecision,
        sink: Sink,
    },
    Stop,
    EditQueued {
        queue_id: i64,
        text: String,
        sink: Sink,
    },
    DeleteQueued {
        queue_id: i64,
        sink: Sink,
    },
    SendQueuedNow {
        queue_id: i64,
        sink: Sink,
    },
    ResumeQueue {
        sink: Sink,
    },
    /// A client shows the conversation up to its end. With `following`, it
    /// keeps the end on screen, so the turns that end next are read as well.
    MarkRead {
        connection: u64,
        following: bool,
        sink: Sink,
    },
    /// The bot moved to Full access: the approvals that wait are allowed.
    PermissionLevelChanged,
    /// The bot is being deleted, or the daemon stops: the running turn stops,
    /// then the task ends and tells so through `done`. An agent that has not
    /// ended its turn after `patience` is closed.
    ShutDown {
        done: oneshot::Sender<()>,
        patience: Duration,
    },
}

enum Input {
    Command(Command),
    Agent {
        generation: u64,
        event: AgentEvent,
    },
    SessionClosed {
        generation: u64,
        result: Result<(), SessionFailure>,
    },
    /// A call to a daemon tool, through the session's endpoint.
    Tool {
        generation: u64,
        signal: ToolSignal,
    },
}

#[derive(Clone)]
pub struct ConversationHandle {
    inputs: UnboundedSender<Input>,
}

impl ConversationHandle {
    pub fn send(&self, command: Command) {
        self.inputs.send(Input::Command(command)).ok();
    }
}

pub fn spawn(daemon: Arc<Daemon>, conversation: &Conversation) -> ConversationHandle {
    let (inputs, receiver) = mpsc::unbounded_channel();
    let store = daemon.store().clone();
    let actor = Actor {
        id: conversation.id,
        bot_id: conversation.bot_id,
        titled: conversation.title.is_some(),
        recorder: Recorder::new(store.clone(), conversation.id),
        store,
        daemon,
        closing: None,
        finished: false,
        followers: HashSet::new(),
        inputs: inputs.clone(),
        subscribers: Vec::new(),
        session: None,
        generation: 0,
        approvals: Vec::new(),
        next_approval: now_ms(),
        turn_active: false,
        turn: 0,
        gate: TurnGate::default(),
        stop_due: None,
        next_after_stop: false,
        running: RunningTurn::default(),
        retry: None,
    };
    tokio::spawn(actor.run(receiver));
    ConversationHandle { inputs }
}

struct Subscriber {
    connection: u64,
    sink: Sink,
}

struct Session {
    generation: u64,
    /// The revision of the bot's session settings this session was started with.
    revision: i64,
    /// The bot as it was when the session started. Its model is the one the
    /// session runs on.
    bot: Bot,
    /// The provider that ran the conversation before, when it is another
    /// one. Taken when the session starts, to record the change once.
    moved_from: Option<String>,
    commands: UnboundedSender<SessionCommand>,
    task: AbortHandle,
    /// The versions of the system prompt's sections the session has seen:
    /// at its start, then in each note since.
    sections: BTreeMap<String, String>,
    /// The daemon's tools for this session. Dropped with the session.
    _endpoint: Option<SessionEndpoint>,
    /// The provider's id of the session, once the agent started it.
    provider_session_id: Option<String>,
}

/// What the daemon watches in the turn that runs (`guard.rs`).
struct RunningTurn {
    origin: TurnOrigin,
    /// The message that started the turn, which a retry sends again.
    text: String,
    /// The retries sent before this turn: 0 for a turn of its own.
    retries: usize,
    /// The agent said something, started a tool call, or asked to run one:
    /// a failure that follows is not retried.
    said: bool,
    progress: Progress,
    /// What the agent reported the turn consumed, and the provider and the
    /// provider session that reported it.
    usage: Option<(String, Option<String>, Reported)>,
    /// Why the daemon stopped the turn itself, recorded as its end.
    stop_cause: Option<Interruption>,
    /// When the turn last heard from its agent or from a person.
    last_activity: Instant,
}

impl Default for RunningTurn {
    fn default() -> Self {
        Self {
            origin: TurnOrigin::User,
            text: String::new(),
            retries: 0,
            said: false,
            progress: Progress::default(),
            usage: None,
            stop_cause: None,
            last_activity: Instant::now(),
        }
    }
}

/// The same message, to send again once `due` passes.
struct Retry {
    text: String,
    /// The retries sent before this one.
    retries: usize,
    due: Instant,
}

/// How a message reaches an agent: through the session that runs, or a
/// session to start.
enum Delivery {
    /// The note is the one the message is stored with, and `seen` the
    /// versions it tells, which the session has seen once it reads it.
    Live {
        note: Option<String>,
        seen: BTreeMap<String, String>,
    },
    Start(Box<Plan>),
}

impl Delivery {
    fn note(&self) -> Option<String> {
        match self {
            Self::Live { note, .. } => note.clone(),
            Self::Start(plan) => plan.note.clone(),
        }
    }
}

/// A provider session to start, read before the message is recorded so that
/// the message keeps its note.
struct Plan {
    revision: i64,
    bot: Bot,
    resumed: Option<ProviderSession>,
    moved_from: Option<String>,
    system_prompt: String,
    sections: BTreeMap<String, String>,
    note: Option<String>,
}

/// Waits until the turn is stopped. A stop that comes before the provider
/// session exists ends the turn here: no agent runs yet to end it.
async fn stopped(commands: &mut UnboundedReceiver<SessionCommand>) {
    loop {
        match commands.recv().await {
            Some(SessionCommand::Cancel { approvals }) => {
                for approval in approvals {
                    approval.send(ApprovalAnswer::Cancel).ok();
                }
                return;
            }
            Some(SessionCommand::Prompt { .. }) => {}
            None => std::future::pending().await,
        }
    }
}

struct PendingApproval {
    id: String,
    tool_call_id: String,
    title: Option<String>,
    cut: bool,
    reply: oneshot::Sender<ApprovalAnswer>,
}

struct TurnFailure {
    message: String,
    cause: Option<FailureCause>,
    provider: Option<String>,
}

impl TurnFailure {
    fn before_session(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
            provider: None,
        }
    }

    fn of_provider(provider: &str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
            provider: Some(provider.to_owned()),
        }
    }

    fn into_event(self) -> Event {
        Event::TurnFailed {
            message: self.message,
            cause: self.cause,
            provider: self.provider,
        }
    }
}

struct Actor {
    id: i64,
    bot_id: i64,
    /// The conversation has its title, so no later message can give it one.
    titled: bool,
    daemon: Arc<Daemon>,
    /// Set while the bot is being deleted: answered once no turn runs.
    closing: Option<oneshot::Sender<()>>,
    finished: bool,
    /// The attached clients that keep the end of the conversation on screen.
    followers: HashSet<u64>,
    store: SharedStore,
    recorder: Recorder,
    inputs: UnboundedSender<Input>,
    subscribers: Vec<Subscriber>,
    session: Option<Session>,
    generation: u64,
    approvals: Vec<PendingApproval>,
    next_approval: u64,
    turn_active: bool,
    /// The number of the turn that runs, or that ran last.
    turn: u64,
    /// Open while a turn runs and no stop is asked: the daemon's tools run
    /// only with a permit of it (`tools.rs`).
    gate: TurnGate,
    /// Set while a stop waits for the agent to end the turn.
    stop_due: Option<Instant>,
    /// Set by "send now": the next queued message starts however the turn ends.
    next_after_stop: bool,
    running: RunningTurn,
    /// A failed turn waits to be sent again.
    retry: Option<Retry>,
}

impl Actor {
    async fn run(mut self, mut inputs: UnboundedReceiver<Input>) {
        loop {
            let input = match self.next_deadline() {
                Some(deadline) => tokio::select! {
                    input = inputs.recv() => input,
                    () = tokio::time::sleep_until(deadline.into()) => {
                        self.deadline_passed();
                        continue;
                    }
                },
                None => inputs.recv().await,
            };
            match input {
                Some(Input::Command(command)) => self.handle_command(command),
                Some(Input::Agent { generation, event }) => {
                    if self.is_current(generation) {
                        self.running.last_activity = Instant::now();
                        self.handle_agent(event);
                    }
                }
                Some(Input::SessionClosed { generation, result }) => {
                    self.session_closed(generation, result);
                }
                Some(Input::Tool { generation, signal }) => {
                    if self.is_current(generation) {
                        self.running.last_activity = Instant::now();
                    }
                    self.tool_signal(generation, signal);
                }
                None => break,
            }
            if self.finished {
                break;
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.recorder.save_due(),
            self.stop_due,
            self.inactive_due(),
            self.retry.as_ref().map(|retry| retry.due),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// When a turn the daemon started, which waits on no person, has heard
    /// nothing from its agent for too long.
    fn inactive_due(&self) -> Option<Instant> {
        (self.turn_active
            && self.running.origin != TurnOrigin::User
            && self.approvals.is_empty()
            && self.stop_due.is_none())
        .then(|| self.running.last_activity + guard::scaled(INACTIVITY_LIMIT))
    }

    fn deadline_passed(&mut self) {
        let now = Instant::now();
        if self.recorder.save_due().is_some_and(|due| due <= now) {
            let saved = self.recorder.save();
            self.report_store(saved);
        }
        if self.stop_due.is_some_and(|due| due <= now) {
            self.close_session();
            self.end_turn(Event::TurnInterrupted {
                cause: Interruption::AgentUnresponsive,
            });
        }
        if self.inactive_due().is_some_and(|due| due <= now) {
            self.guard_stop(Interruption::Inactive);
        }
        if self.retry.as_ref().is_some_and(|retry| retry.due <= now) {
            self.send_retry();
        }
    }

    fn is_current(&self, generation: u64) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.generation == generation)
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Subscribe { connection, sink } => self.subscribe(connection, sink),
            Command::Unsubscribe { connection } => {
                self.subscribers
                    .retain(|subscriber| subscriber.connection != connection);
                self.followers.remove(&connection);
            }
            Command::MarkRead {
                connection,
                following,
                sink,
            } => self.mark_read(connection, following, &sink),
            Command::Prompt { text, sink } => self.prompt(text, &sink),
            Command::LoadOlder { before, sink } => {
                let page = self
                    .store
                    .with(|store| store.events_page(self.id, Some(before), PAGE_EVENTS));
                self.send_older(page, &sink);
            }
            Command::Reveal {
                event_id,
                before,
                sink,
            } => {
                let window = self.store.with(|store| {
                    store.events_around(self.id, event_id, before, JOINED_EVENTS, PAGE_EVENTS)
                });
                let answer = match window {
                    Ok(window) if window.newer.is_some() => ServerMessage::EventsWindow {
                        conversation_id: self.id,
                        events: window.events,
                        older: window.older,
                        newer: window.newer,
                    },
                    Ok(window) => ServerMessage::OlderEvents {
                        conversation_id: self.id,
                        events: window.events,
                        older: window.older,
                    },
                    Err(error) => ServerMessage::OlderEventsFailed {
                        conversation_id: self.id,
                        message: error.to_string(),
                    },
                };
                sink.send(answer).ok();
            }
            Command::LoadNewer { after, until, sink } => {
                let page = self
                    .store
                    .with(|store| store.events_between(self.id, after, until, PAGE_EVENTS));
                let answer = match page {
                    Ok(page) => ServerMessage::NewerEvents {
                        conversation_id: self.id,
                        events: page.events,
                        newer: page.newer,
                    },
                    Err(error) => ServerMessage::NewerEventsFailed {
                        conversation_id: self.id,
                        message: error.to_string(),
                    },
                };
                sink.send(answer).ok();
            }
            Command::SetDraft { text, sink } => {
                if let Err(error) = self.store.with(|store| store.set_draft(self.id, &text)) {
                    report(&sink, error.to_string());
                }
            }
            Command::ResolveApproval {
                approval_id,
                decision,
                sink,
            } => self.resolve_approval(&approval_id, decision, &sink),
            Command::Stop => {
                // A person who stops the conversation stops its retry too.
                self.retry = None;
                self.stop(STOP_TIMEOUT);
            }
            Command::EditQueued {
                queue_id,
                text,
                sink,
            } => self.edit_queued(queue_id, &text, &sink),
            Command::DeleteQueued { queue_id, sink } => self.delete_queued(queue_id, &sink),
            Command::SendQueuedNow { queue_id, sink } => self.send_queued_now(queue_id, &sink),
            Command::ResumeQueue { sink } => self.resume_queue(&sink),
            Command::PermissionLevelChanged => self.permission_level_changed(),
            Command::ShutDown { done, patience } => self.shut_down(done, patience),
        }
    }

    fn mark_read(&mut self, connection: u64, following: bool, sink: &Sink) {
        let subscribed = self
            .subscribers
            .iter()
            .any(|subscriber| subscriber.connection == connection);
        if following && subscribed {
            self.followers.insert(connection);
        } else {
            self.followers.remove(&connection);
        }
        match self.store.with(|store| store.mark_read(self.id)) {
            Ok(_) => self.daemon.publish_bot(self.bot_id),
            Err(error) => report(sink, error.to_string()),
        }
    }

    /// Tells the daemon what this conversation is doing, for the bot's state.
    fn sync(&self) {
        self.daemon.report_activity(
            self.id,
            self.bot_id,
            self.turn_active,
            !self.approvals.is_empty(),
        );
    }

    /// The level comes from the bot's record and from nowhere else: no prompt,
    /// tool result, or file reaches it. A record that cannot be read asks first.
    fn permission_level(&self) -> PermissionLevel {
        match self.store.with(|store| store.bot(self.bot_id)) {
            Ok(Some(row)) => row.bot.permission_level,
            Ok(None) | Err(_) => PermissionLevel::AskFirst,
        }
    }

    fn permission_level_changed(&mut self) {
        if self.stop_due.is_some() || self.permission_level() != PermissionLevel::FullAccess {
            return;
        }
        // A request whose text was cut was shown as one that can only be
        // declined: it keeps waiting for that answer.
        let (cut, shown): (Vec<_>, Vec<_>) = std::mem::take(&mut self.approvals)
            .into_iter()
            .partition(|approval| approval.cut);
        self.approvals = cut;
        self.running.last_activity = Instant::now();
        for approval in shown {
            if approval.reply.send(ApprovalAnswer::Allow).is_ok() {
                self.emit(Event::ApprovalResolved {
                    id: Some(approval.id),
                    tool_call_id: approval.tool_call_id,
                    title: approval.title,
                    decision: ApprovalDecision::Allowed,
                    reason: Some(ApprovalReason::PermissionLevel),
                });
            }
        }
        self.sync();
    }

    fn shut_down(&mut self, done: oneshot::Sender<()>, patience: Duration) {
        self.closing = Some(done);
        self.next_after_stop = false;
        self.stop(patience);
        self.finish_closing();
    }

    /// Ends this task once no turn runs: the provider process closes with it.
    fn finish_closing(&mut self) {
        if self.turn_active {
            return;
        }
        if let Some(done) = self.closing.take() {
            self.close_session();
            self.finished = true;
            done.send(()).ok();
        }
    }

    /// The first message of a conversation gives it its title.
    fn give_title(&mut self, first_message: &str) {
        if std::mem::replace(&mut self.titled, true) {
            return;
        }
        match self
            .store
            .with(|store| store.set_title(self.id, first_message))
        {
            Ok(true) => self.daemon.publish_conversation(self.id),
            Ok(false) => {}
            Err(error) => self.report_store(Err::<(), _>(error)),
        }
    }

    fn subscribe(&mut self, connection: u64, sink: Sink) {
        match self.snapshot() {
            Ok(snapshot) => {
                sink.send(ServerMessage::ConversationOpened(snapshot)).ok();
                self.subscribers
                    .retain(|subscriber| subscriber.connection != connection);
                self.subscribers.push(Subscriber { connection, sink });
            }
            Err(message) => report(&sink, message),
        }
    }

    fn snapshot(&mut self) -> Result<ConversationSnapshot, String> {
        self.recorder.save().map_err(|error| error.to_string())?;
        let (conversation, page, draft, queue, usage) = self
            .store
            .with(|store| {
                Ok((
                    store.summary(self.id)?,
                    store.events_page(self.id, None, PAGE_EVENTS)?,
                    store.draft(self.id)?,
                    store.queue(self.id)?,
                    store.last_usage(self.id)?,
                ))
            })
            .map_err(|error: StoreError| error.to_string())?;
        let conversation =
            conversation.ok_or_else(|| format!("conversation {} does not exist", self.id))?;
        Ok(ConversationSnapshot {
            conversation,
            events: page.events,
            older: page.older,
            draft,
            turn_active: self.turn_active,
            pending_approvals: self
                .approvals
                .iter()
                .map(|approval| approval.id.clone())
                .collect(),
            queue,
            turn_model: self
                .session
                .as_ref()
                .filter(|_| self.turn_active)
                .map(|session| session.bot.model.clone()),
            usage,
        })
    }

    fn send_older(&self, page: Result<EventsPage, StoreError>, sink: &Sink) {
        let answer = match page {
            Ok(page) => ServerMessage::OlderEvents {
                conversation_id: self.id,
                events: page.events,
                older: page.older,
            },
            Err(error) => ServerMessage::OlderEventsFailed {
                conversation_id: self.id,
                message: error.to_string(),
            },
        };
        sink.send(answer).ok();
    }

    fn refuse_message(&self, sink: &Sink, message: impl Into<String>) {
        sink.send(ServerMessage::MessageRefused {
            conversation_id: self.id,
            message: message.into(),
        })
        .ok();
    }

    fn accept_message(&mut self, sink: &Sink) {
        sink.send(ServerMessage::MessageAccepted {
            conversation_id: self.id,
        })
        .ok();
        let cleared = self.store.with(|store| store.set_draft(self.id, ""));
        self.report_store(cleared);
    }

    fn prompt(&mut self, text: String, sink: &Sink) {
        if self.closing.is_some() {
            return self.refuse_message(sink, "This bot is being deleted.");
        }
        if text.trim().is_empty() {
            return self.refuse_message(sink, "A message needs some text.");
        }
        if text.len() > MESSAGE_LENGTH && text.chars().count() > MESSAGE_LENGTH {
            return self.refuse_message(sink, "A message can hold 100,000 characters at most.");
        }
        // The conversation a person writes in is the one the bot opens on.
        match self.store.with(|store| store.make_current(self.id)) {
            Ok(true) => self.daemon.publish_bot(self.bot_id),
            Ok(false) => {}
            Err(error) => return self.refuse_message(sink, error.to_string()),
        }
        let queue = match self.queue() {
            Ok(queue) => queue,
            Err(error) => return self.refuse_message(sink, error.to_string()),
        };
        // A message never overtakes the ones that already wait.
        if self.turn_active || !queue.messages.is_empty() {
            match self.store.with(|store| store.enqueue(self.id, &text)) {
                Ok(()) => {
                    // The person wrote again: the failed turn is not sent again.
                    self.retry = None;
                    self.accept_message(sink);
                    self.broadcast_queue();
                }
                Err(error) => self.refuse_message(sink, error.to_string()),
            }
            return;
        }
        self.accept_message(sink);
        self.start_turn(text, TurnOrigin::User, 0);
    }

    /// Starts a turn with a message that is not in the queue: a person's, or
    /// one the daemon sends on its own. `retries` counts the retries sent
    /// before it, set before delivery, which can end the turn at once.
    fn start_turn(&mut self, text: String, origin: TurnOrigin, retries: usize) {
        self.begin_turn(&text, origin);
        self.running.retries = retries;
        let delivery = self.prepare();
        let note = delivery.as_ref().ok().and_then(Delivery::note);
        self.emit(Event::UserText(
            UserMessage::new(text.clone())
                .with_note(note)
                .with_origin(origin),
        ));
        self.give_title(&text);
        self.sync();
        self.deliver(delivery, text);
    }

    /// A turn starts: the daemon's tools may run, with permits of this turn,
    /// and the guards watch it from its first event. A retry that waited is
    /// dropped: another turn took its place.
    fn begin_turn(&mut self, text: &str, origin: TurnOrigin) {
        self.turn_active = true;
        self.turn += 1;
        self.gate.open(self.turn);
        self.retry = None;
        self.running = RunningTurn {
            origin,
            text: text.to_owned(),
            ..RunningTurn::default()
        };
    }

    /// Sends again the message of a turn that failed before it said
    /// anything, unless a turn runs, the conversation closes, or the daemon
    /// is paused: a paused daemon starts no turn of its own.
    fn send_retry(&mut self) {
        let Some(retry) = self.retry.take() else {
            return;
        };
        if self.turn_active || self.closing.is_some() {
            return;
        }
        if self.daemon.paused() {
            eprintln!(
                "[crowbotd] conversation {}: the daemon is paused, so the failed turn is not sent again",
                self.id
            );
            return;
        }
        self.start_turn(retry.text, TurnOrigin::Retry, retry.retries);
    }

    /// Stops the running turn for a reason of the daemon's guards, which is
    /// recorded as the turn's end.
    fn guard_stop(&mut self, cause: Interruption) {
        if !self.turn_active || self.stop_due.is_some() || self.running.stop_cause.is_some() {
            return;
        }
        eprintln!(
            "[crowbotd] conversation {}: the turn is stopped ({cause:?})",
            self.id
        );
        self.running.stop_cause = Some(cause);
        self.stop(STOP_TIMEOUT);
    }

    /// What an event of the agent tells the guards: a turn that repeats
    /// itself, or one that asks for more tool calls than its bot allows,
    /// stops.
    fn watch(&mut self, event: &Event) {
        if !self.turn_active {
            return;
        }
        let seen = self.running.progress.observe(event);
        if seen.no_progress {
            return self.guard_stop(Interruption::NoProgress);
        }
        if seen.new_call
            && self
                .tool_call_limit()
                .is_some_and(|limit| self.running.progress.calls() > limit)
        {
            self.guard_stop(Interruption::BudgetExhausted);
        }
    }

    /// The bot's limit of tool calls per turn, read from its record at each
    /// call, so that a lower limit stops a turn that already runs.
    fn tool_call_limit(&self) -> Option<usize> {
        let limit = match self.store.with(|store| store.bot(self.bot_id)) {
            Ok(Some(row)) => row.bot.tool_call_limit,
            Ok(None) | Err(_) => None,
        };
        limit.and_then(|limit| usize::try_from(limit).ok())
    }

    /// Keeps what the agent reports the running turn consumed.
    fn record_usage(&mut self, report: impl FnOnce(&mut Reported)) {
        if !self.turn_active {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let source = (
            session.bot.provider.clone(),
            session.provider_session_id.clone(),
        );
        let usage = self
            .running
            .usage
            .get_or_insert_with(|| (source.0, source.1, Reported::default()));
        report(&mut usage.2);
    }

    /// The turn `turn` runs and no stop is asked: a daemon tool of that turn
    /// may still act.
    fn turn_open(&self, generation: u64, turn: u64) -> bool {
        self.is_current(generation)
            && self.turn_active
            && self.stop_due.is_none()
            && self.turn == turn
    }

    fn deliver(&mut self, delivery: Result<Delivery, TurnFailure>, text: String) {
        match delivery.and_then(|delivery| self.try_deliver(delivery, text)) {
            Ok(()) => {
                if let Some(model) = self
                    .session
                    .as_ref()
                    .map(|session| session.bot.model.clone())
                {
                    self.broadcast(ServerMessage::TurnModel {
                        conversation_id: self.id,
                        model,
                    });
                }
            }
            Err(failure) => self.end_turn(failure.into_event()),
        }
    }

    /// Reads how the next message reaches an agent, and what changed since
    /// that session saw the sections of its system prompt. Another provider,
    /// new instructions, a new model, a new reasoning level, or a new folder
    /// take a new provider session: the one that runs keeps the old ones.
    fn prepare(&mut self) -> Result<Delivery, TurnFailure> {
        let (conversation, bot) = self
            .store
            .with(|store| {
                Ok::<_, StoreError>((store.conversation(self.id)?, store.bot(self.bot_id)?))
            })
            .map_err(|error| TurnFailure::before_session(error.to_string()))?;
        conversation.ok_or_else(|| {
            TurnFailure::before_session(format!("conversation {} does not exist", self.id))
        })?;
        let bot = bot.ok_or_else(|| {
            TurnFailure::before_session(format!("bot {} does not exist", self.bot_id))
        })?;
        if !Path::new(&bot.bot.workspace).is_dir() {
            self.close_session();
            return Err(TurnFailure::before_session(format!(
                "The folder {} does not exist. Choose another one in the bot's settings.",
                bot.bot.workspace
            )));
        }
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.revision != bot.session_revision)
        {
            self.close_session();
        }
        if let Some(session) = &self.session {
            let (note, seen) = self.daemon.sections().note(self.bot_id, &session.sections);
            return Ok(Delivery::Live { note, seen });
        }
        let provider = bot.bot.provider.clone();
        self.plan_session(bot)
            .map(|plan| Delivery::Start(Box::new(plan)))
            .map_err(|message| TurnFailure::of_provider(&provider, message))
    }

    fn try_deliver(&mut self, delivery: Delivery, text: String) -> Result<(), TurnFailure> {
        let (note, seen) = match delivery {
            Delivery::Start(plan) => {
                let provider = plan.bot.provider.clone();
                return self.start_session(*plan, text, &provider);
            }
            Delivery::Live { note, seen } => (note, seen),
        };
        let Some(session) = &mut self.session else {
            return Err(TurnFailure::before_session(
                "the agent stopped before it received the message",
            ));
        };
        let provider = session.bot.provider.clone();
        let prompt = SessionCommand::Prompt {
            text,
            handoff: None,
            note: note.clone(),
        };
        match session.commands.send(prompt) {
            Ok(()) => {
                if note.is_some() {
                    // The versions the stored note told, and not the ones of
                    // now: a change since then reaches the next note.
                    session.sections = seen.clone();
                    let saved = self
                        .store
                        .with(|store| store.set_session_sections(self.id, &provider, &seen));
                    self.report_store(saved);
                }
                Ok(())
            }
            Err(mpsc::error::SendError(SessionCommand::Prompt { text, .. })) => {
                // The agent ended meanwhile: a new session runs the message,
                // with what it needs to read first.
                self.close_session();
                let failed = || {
                    TurnFailure::of_provider(
                        &provider,
                        "the agent stopped before it received the message",
                    )
                };
                let bot = self
                    .store
                    .with(|store| store.bot(self.bot_id))
                    .ok()
                    .flatten()
                    .ok_or_else(failed)?;
                let plan = self
                    .plan_session(bot)
                    .map_err(|message| TurnFailure::of_provider(&provider, message))?;
                self.start_session(plan, text, &provider)
            }
            Err(_) => Err(TurnFailure::of_provider(
                &provider,
                "the agent stopped before it received the message",
            )),
        }
    }

    /// Reads the provider session that runs the next message: the one the
    /// provider holds for this conversation when it was started with the
    /// settings the bot has now, and a new one otherwise. A resumed session
    /// gets in a note what changed in the sections since it last saw them; a
    /// new one gets them in its system prompt.
    fn plan_session(&mut self, bot: BotRow) -> Result<Plan, String> {
        let revision = bot.session_revision;
        let bot = bot.bot;
        let provider = bot.provider.clone();
        let fingerprint = session_fingerprint(&bot);
        let (held, last) = self
            .store
            .with(|store| {
                Ok::<_, StoreError>((
                    store.provider_session(self.id, &provider)?,
                    store.last_provider_session(self.id)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        let resumed = held.filter(|session| session.fingerprint == fingerprint);
        let moved_from = last
            .map(|session| session.provider)
            .filter(|last| *last != provider);
        let snapshot = self.daemon.sections().snapshot(self.bot_id);
        let system_prompt = Sections::system_prompt(&bot.instructions, &snapshot);
        let (note, sections) = match &resumed {
            Some(session) => self.daemon.sections().note(self.bot_id, &session.sections),
            None => (None, snapshot.versions),
        };
        Ok(Plan {
            revision,
            bot,
            resumed,
            moved_from,
            system_prompt,
            sections,
            note,
        })
    }

    /// Starts the provider session that runs the message. When another
    /// provider ran the conversation last, the message comes after what the
    /// session has not seen of it.
    fn start_session(
        &mut self,
        plan: Plan,
        text: String,
        provider: &str,
    ) -> Result<(), TurnFailure> {
        let session = self
            .launch(plan, text)
            .map_err(|message| TurnFailure::of_provider(provider, message))?;
        self.session = Some(session);
        Ok(())
    }

    fn launch(&mut self, plan: Plan, text: String) -> Result<Session, String> {
        let Plan {
            revision,
            bot,
            resumed,
            moved_from,
            system_prompt,
            sections,
            note,
        } = plan;
        self.recorder.save().map_err(|error| error.to_string())?;
        let handoff = match &moved_from {
            Some(from) => {
                let seen = resumed.as_ref().map_or(0, |session| session.last_event_id);
                let events = self
                    .store
                    .with(|store| store.handoff_events(self.id, seen, HANDOFF_CHARACTERS))
                    .map_err(|error| error.to_string())?;
                handoff::build(&events, &self.daemon.provider_name(from))
            }
            None => None,
        };
        let first = vec![SessionCommand::Prompt {
            text,
            handoff,
            note,
        }];
        let resume = resumed.map(|session| session.session_id);
        let settings = bot.clone();

        self.generation += 1;
        let generation = self.generation;
        let endpoint = self.open_endpoint(generation);
        let mcp = endpoint.as_ref().map(SessionEndpoint::command);
        let tools = self.daemon.tools().clone();
        let (commands, mut command_receiver) = mpsc::unbounded_channel();
        let (events, mut event_receiver) = mpsc::unbounded_channel();
        let inputs = self.inputs.clone();
        let daemon = self.daemon.clone();
        let task = tokio::spawn(async move {
            let forwarding = inputs.clone();
            let forward = async move {
                while let Some(event) = event_receiver.recv().await {
                    if forwarding.send(Input::Agent { generation, event }).is_err() {
                        break;
                    }
                }
            };
            let session = async move {
                // A runtime that is not installed downloads here: the turn
                // waits for it, and a stop ends the wait.
                let (agent, launch, signed_in) = tokio::select! {
                    launch = daemon.session_launch(&settings.provider) => launch?,
                    () = stopped(&mut command_receiver) => {
                        events
                            .send(AgentEvent::Event(Event::TurnEnded(TurnEnd::Cancelled)))
                            .ok();
                        return Ok(());
                    }
                };
                let config = SessionConfig {
                    agent,
                    launch,
                    cwd: PathBuf::from(settings.workspace),
                    resume,
                    system_prompt,
                    mcp,
                    tools,
                    model: settings.model,
                    effort: settings.effort,
                    signed_in,
                };
                acp::run_session(config, first, command_receiver, events).await
            };
            let (result, ()) = tokio::join!(session, forward);
            inputs
                .send(Input::SessionClosed { generation, result })
                .ok();
        });
        Ok(Session {
            generation,
            revision,
            bot,
            moved_from,
            commands,
            task: task.abort_handle(),
            sections,
            _endpoint: endpoint,
            provider_session_id: None,
        })
    }

    /// The endpoint the session's bridge reaches the daemon's tools through.
    /// A session whose endpoint cannot be made runs without them.
    fn open_endpoint(&self, generation: u64) -> Option<SessionEndpoint> {
        let (signals, mut received) = mpsc::unbounded_channel();
        let inputs = self.inputs.clone();
        tokio::spawn(async move {
            while let Some(signal) = received.recv().await {
                if inputs.send(Input::Tool { generation, signal }).is_err() {
                    break;
                }
            }
        });
        let context = ToolContext {
            bot_id: self.bot_id,
            conversation_id: self.id,
            kind: SessionKind::Conversation,
            outputs: Outputs::default(),
        };
        match SessionEndpoint::open(
            self.daemon.tools().clone(),
            context,
            self.gate.clone(),
            signals,
        ) {
            Ok(endpoint) => Some(endpoint),
            Err(error) => {
                eprintln!(
                    "[crowbotd] conversation {}: the session runs without the daemon's tools: {error}",
                    self.id
                );
                None
            }
        }
    }

    /// A call to a daemon tool: a refusal is recorded, an `External` tool
    /// asks as any other request does, and a change to the bot's state is
    /// applied. Each is answered as cancelled unless the turn its permit was
    /// given in still runs with no stop asked: the check and the write happen
    /// here, in one step of the conversation's loop and one transaction.
    fn tool_signal(&mut self, generation: u64, signal: ToolSignal) {
        match signal {
            ToolSignal::Refused { tool, message } => {
                if self.is_current(generation) {
                    self.emit(Event::ToolRefused { tool, message });
                }
            }
            ToolSignal::Approval {
                turn,
                tool_call_id,
                title,
                target,
                cut,
                reply,
            } => {
                if !self.turn_open(generation, turn) {
                    reply.send(ApprovalAnswer::Cancel).ok();
                    return;
                }
                let tool_call_id = tool_call_id.unwrap_or_else(|| self.new_approval_id());
                self.ask_approval(tool_call_id, Some(title), target, cut, reply);
            }
            ToolSignal::Apply { mutation, reply } => {
                if !self.turn_open(generation, mutation.turn()) {
                    reply
                        .send(Err(
                            "The turn was stopped before this change was applied, so nothing was changed."
                                .to_owned(),
                        ))
                        .ok();
                    return;
                }
                // Text still in memory reaches the database first, so that
                // the change's event keeps its place in the conversation.
                let closed = self.recorder.close();
                self.report_store(closed);
                match self
                    .store
                    .with(|store| store.apply_mutation(self.id, mutation))
                {
                    Ok(applied) => {
                        if let Some(event) = applied.event {
                            self.broadcast(ServerMessage::ConversationEvent {
                                conversation_id: self.id,
                                event,
                                at_ms: at_ms(),
                            });
                        }
                        reply.send(Ok(applied.output)).ok();
                    }
                    Err(error) => {
                        reply
                            .send(Err(format!(
                                "The change could not be stored, so nothing was changed: {error}"
                            )))
                            .ok();
                        self.report_store(Err::<(), _>(error));
                    }
                }
            }
        }
    }

    /// Closes the provider process at once. Events it already sent are ignored,
    /// and the next message starts a session that resumes the provider's own.
    fn close_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.task.abort();
        }
    }

    fn handle_agent(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Started {
                provider_session_id,
            } => {
                let Some(session) = self.session.as_mut() else {
                    return;
                };
                session.provider_session_id = Some(provider_session_id.clone());
                let provider = session.bot.provider.clone();
                let fingerprint = session_fingerprint(&session.bot);
                let sections = session.sections.clone();
                if let Some(from) = session.moved_from.take() {
                    self.emit(Event::ProviderChanged {
                        from,
                        to: provider.clone(),
                    });
                }
                let saved = self.recorder.save().and_then(|()| {
                    self.store.with(|store| {
                        store.set_provider_session(
                            self.id,
                            &provider,
                            &provider_session_id,
                            &fingerprint,
                        )?;
                        store.set_session_sections(self.id, &provider, &sections)
                    })
                });
                self.report_store(saved);
            }
            AgentEvent::Offer(offer) => {
                if let Some(session) = &self.session {
                    self.daemon.record_offer(&session.bot.provider, offer);
                }
            }
            AgentEvent::ModelUnavailable { model } => {
                let Some(session) = self.session.as_mut() else {
                    return;
                };
                // The session runs on the provider's default model, and so
                // does the bot from now on.
                session.bot.model = None;
                let provider = session.bot.provider.clone();
                self.emit(Event::ModelUnavailable {
                    model: model.clone(),
                    provider: Some(provider),
                });
                let cleared = self
                    .store
                    .with(|store| store.clear_bot_model(self.bot_id, &model));
                self.report_store(cleared);
                self.daemon.publish_bot(self.bot_id);
                self.broadcast(ServerMessage::TurnModel {
                    conversation_id: self.id,
                    model: None,
                });
            }
            AgentEvent::Usage { used, size, cost } => self.record_usage(|reported| {
                reported.context = Some((used, size));
                // Claude sends the cost on the last update of a turn only.
                if cost.is_some() {
                    reported.session_cost = cost;
                }
            }),
            AgentEvent::TurnTokens {
                input,
                output,
                total,
            } => self.record_usage(|reported| {
                reported.tokens = Some(Tokens {
                    input,
                    output,
                    total,
                });
            }),
            AgentEvent::Event(event) => {
                if event.ends_turn() {
                    // A turn ends once: an end that arrives after the turn was
                    // already closed is dropped.
                    if self.turn_active {
                        self.end_turn(event);
                    }
                } else {
                    self.emit(event.clone());
                    self.watch(&event);
                }
            }
            AgentEvent::ApprovalNeeded {
                tool_call_id,
                title,
                target,
                cut,
                reply,
            } => self.ask_approval(tool_call_id, title, target, cut, reply),
            AgentEvent::ApprovalRefused {
                tool_call_id,
                title,
            } => self.emit(Event::ApprovalResolved {
                id: None,
                tool_call_id,
                title,
                decision: ApprovalDecision::Declined,
                reason: Some(ApprovalReason::NoAllowOnce),
            }),
            AgentEvent::DaemonToolRequested {
                tool_call_id,
                title,
                reply,
            } => {
                let id = self.new_approval_id();
                // The daemon allows its own tool only within a turn that is
                // not being stopped: the tool's gate would refuse it anyway.
                if !self.turn_active || self.stop_due.is_some() {
                    reply.send(ApprovalAnswer::Cancel).ok();
                    self.emit(Event::ApprovalClosed {
                        id,
                        tool_call_id,
                        title,
                        reason: ApprovalClosure::Cancelled,
                    });
                    return;
                }
                if reply.send(ApprovalAnswer::Allow).is_ok() {
                    self.emit(Event::ApprovalResolved {
                        id: Some(id),
                        tool_call_id,
                        title,
                        decision: ApprovalDecision::Allowed,
                        reason: Some(ApprovalReason::DaemonTool),
                    });
                }
            }
        }
    }

    /// A request that the bot's level answers, or that waits for a person.
    /// A stop that is under way cancels it.
    fn ask_approval(
        &mut self,
        tool_call_id: String,
        title: Option<String>,
        target: Option<String>,
        cut: bool,
        reply: oneshot::Sender<ApprovalAnswer>,
    ) {
        {
            {
                let id = self.new_approval_id();
                if self.stop_due.is_some() {
                    reply.send(ApprovalAnswer::Cancel).ok();
                    self.emit(Event::ApprovalClosed {
                        id,
                        tool_call_id,
                        title,
                        reason: ApprovalClosure::Cancelled,
                    });
                    return;
                }
                if self.permission_level() == PermissionLevel::FullAccess {
                    if reply.send(ApprovalAnswer::Allow).is_ok() {
                        self.emit(Event::ApprovalResolved {
                            id: Some(id),
                            tool_call_id,
                            title,
                            decision: ApprovalDecision::Allowed,
                            reason: Some(ApprovalReason::PermissionLevel),
                        });
                    }
                    return;
                }
                self.approvals.push(PendingApproval {
                    id: id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    title: title.clone(),
                    cut,
                    reply,
                });
                let folder = self
                    .session
                    .as_ref()
                    .map(|session| session.bot.workspace.clone());
                self.emit(Event::ApprovalRequested {
                    id,
                    tool_call_id,
                    title,
                    target,
                    folder,
                    cut,
                });
                self.sync();
            }
        }
    }

    fn new_approval_id(&mut self) -> String {
        self.next_approval += 1;
        format!("approval-{}", self.next_approval)
    }

    fn resolve_approval(&mut self, approval_id: &str, decision: ApprovalDecision, sink: &Sink) {
        let conversation_id = self.id;
        let refuse = |message: &str| {
            sink.send(ServerMessage::ApprovalNotResolved {
                conversation_id,
                approval_id: approval_id.to_owned(),
                message: message.to_owned(),
            })
            .ok();
        };
        let Some(index) = self
            .approvals
            .iter()
            .position(|approval| approval.id == approval_id)
        else {
            return refuse("This approval is no longer waiting for an answer.");
        };
        if self.approvals[index].cut && decision == ApprovalDecision::Allowed {
            return refuse("This request is too long to show in full, so it can only be declined.");
        }
        let approval = self.approvals.remove(index);
        let answer = match decision {
            ApprovalDecision::Allowed => ApprovalAnswer::Allow,
            ApprovalDecision::Declined => ApprovalAnswer::Decline,
        };
        if approval.reply.send(answer).is_err() {
            self.sync();
            return refuse("The agent stopped waiting for this approval.");
        }
        self.emit(Event::ApprovalResolved {
            id: Some(approval.id),
            tool_call_id: approval.tool_call_id,
            title: approval.title,
            decision,
            reason: None,
        });
        // The time a person took to answer is not the agent's silence.
        self.running.last_activity = Instant::now();
        self.sync();
    }

    /// Records every waiting approval as cancelled, and returns the channels
    /// that carry the answer to the agent.
    fn close_approvals(&mut self) -> Vec<oneshot::Sender<ApprovalAnswer>> {
        let mut replies = Vec::new();
        for approval in std::mem::take(&mut self.approvals) {
            replies.push(approval.reply);
            self.emit(Event::ApprovalClosed {
                id: approval.id,
                tool_call_id: approval.tool_call_id,
                title: approval.title,
                reason: ApprovalClosure::Cancelled,
            });
        }
        replies
    }

    /// Stops the running turn. An agent that has not ended it after
    /// `patience` is closed, and the turn is recorded as interrupted.
    fn stop(&mut self, patience: Duration) {
        if !self.turn_active {
            return;
        }
        // From here no daemon tool starts, and no change of one is applied.
        self.gate.close();
        if let Some(due) = self.stop_due {
            // A shorter patience, the one of a daemon that stops, wins.
            self.stop_due = Some(due.min(Instant::now() + patience));
            return;
        }
        let approvals = self.close_approvals();
        self.sync();
        let asked = self.session.as_ref().is_some_and(|session| {
            session
                .commands
                .send(SessionCommand::Cancel { approvals })
                .is_ok()
        });
        if asked {
            self.stop_due = Some(Instant::now() + patience);
        } else {
            self.close_session();
            self.end_turn(Event::TurnInterrupted {
                cause: Interruption::AgentUnresponsive,
            });
        }
    }

    fn session_closed(&mut self, generation: u64, result: Result<(), SessionFailure>) {
        if !self.is_current(generation) {
            return;
        }
        let provider = self.session.take().map(|session| session.bot.provider);
        if self.turn_active {
            let failure = match result {
                Err(failure) => TurnFailure {
                    message: failure.message,
                    cause: failure.cause,
                    provider,
                },
                Ok(()) => TurnFailure {
                    message: "the agent stopped before the turn ended".to_owned(),
                    cause: None,
                    provider,
                },
            };
            self.end_turn(failure.into_event());
        }
    }

    /// Records the end of the running turn, then starts the next queued message
    /// or pauses the queue, and tells the daemon where the bot stands.
    fn end_turn(&mut self, end: Event) {
        self.finish_turn(end);
        self.sync();
        self.finish_closing();
    }

    fn finish_turn(&mut self, end: Event) {
        for reply in self.close_approvals() {
            reply.send(ApprovalAnswer::Cancel).ok();
        }
        // A turn the daemon's guards stopped ends for their reason, whatever
        // the agent answered to the stop.
        let end = match self.running.stop_cause.take() {
            Some(cause) => Event::TurnInterrupted { cause },
            None => end,
        };
        let completed = matches!(end, Event::TurnEnded(_));
        let retry = self.retry_after(&end);
        if let Some((provider, session_id, reported)) = self.running.usage.take() {
            let closed = self.recorder.close();
            self.report_store(closed);
            let stored = self.store.with(|store| {
                store.record_usage(self.id, &provider, session_id.as_deref(), &reported)
            });
            self.report_store(stored);
        }
        self.emit(end);
        if !self.followers.is_empty() {
            // A client has the end of the conversation on screen: the turn
            // that just ended is read before the bot's state is told.
            let marked = self.store.with(|store| store.mark_read(self.id));
            self.report_store(marked);
        }
        self.turn_active = false;
        self.gate.close();
        self.stop_due = None;
        let closed = self.recorder.close();
        self.report_store(closed);
        // The session that ran the turn knows the conversation up to here.
        if let Some(provider) = self
            .session
            .as_ref()
            .map(|session| session.bot.provider.clone())
        {
            let known = self
                .store
                .with(|store| store.touch_provider_session(self.id, &provider));
            self.report_store(known);
        }

        let send_now = std::mem::take(&mut self.next_after_stop);
        if self.closing.is_some() {
            return;
        }
        if let Some(wait) = retry {
            self.retry = Some(Retry {
                text: std::mem::take(&mut self.running.text),
                retries: self.running.retries + 1,
                due: Instant::now() + guard::scaled(wait),
            });
        }
        let queue = match self.queue() {
            Ok(queue) => queue,
            Err(error) => return self.report_store(Err::<(), _>(error)),
        };
        if queue.messages.is_empty() {
            return;
        }
        if send_now || (completed && !queue.paused) {
            self.start_next_queued();
        } else if !queue.paused {
            let paused = self
                .store
                .with(|store| store.set_queue_paused(self.id, true));
            self.report_store(paused);
            self.broadcast_queue();
        }
    }

    /// The wait before the turn that ends with `end` is sent again: a turn
    /// that failed for no cause the agent named, before it said anything or
    /// started a tool call, while retries are left. A sign-in or a usage
    /// limit does not pass with a wait, and a failure before any provider
    /// ran (a missing folder) would fail the same way.
    fn retry_after(&self, end: &Event) -> Option<std::time::Duration> {
        let Event::TurnFailed {
            cause: None | Some(FailureCause::Other),
            provider: Some(_),
            ..
        } = end
        else {
            return None;
        };
        if self.running.said {
            return None;
        }
        RETRY_DELAYS.get(self.running.retries).copied()
    }

    fn start_next_queued(&mut self) {
        if self.turn_active {
            return;
        }
        if self.queue().is_ok_and(|queue| queue.messages.is_empty()) {
            return;
        }
        let delivery = self.prepare();
        let note = delivery.as_ref().ok().and_then(Delivery::note);
        match self
            .store
            .with(|store| store.start_queued(self.id, note.clone()))
        {
            Ok(Some(text)) => {
                self.begin_turn(&text, TurnOrigin::User);
                self.broadcast(ServerMessage::ConversationEvent {
                    conversation_id: self.id,
                    event: Event::UserText(UserMessage::new(text.clone()).with_note(note)),
                    at_ms: at_ms(),
                });
                self.broadcast_queue();
                self.give_title(&text);
                self.sync();
                self.deliver(delivery, text);
            }
            Ok(None) => {}
            Err(error) => self.report_store(Err::<(), _>(error)),
        }
    }

    fn edit_queued(&mut self, queue_id: i64, text: &str, sink: &Sink) {
        if text.trim().is_empty() {
            return report(sink, "A message needs some text.");
        }
        let edited = self
            .store
            .with(|store| store.edit_queued(self.id, queue_id, text));
        self.queue_changed(edited, sink);
    }

    fn delete_queued(&mut self, queue_id: i64, sink: &Sink) {
        let deleted = self
            .store
            .with(|store| store.delete_queued(self.id, queue_id));
        self.queue_changed(deleted, sink);
    }

    fn send_queued_now(&mut self, queue_id: i64, sink: &Sink) {
        let moved = self.store.with(|store| {
            let moved = store.move_queued_first(self.id, queue_id)?;
            if moved {
                store.set_queue_paused(self.id, false)?;
            }
            Ok(moved)
        });
        if !self.queue_changed(moved, sink) {
            return;
        }
        if self.turn_active {
            self.next_after_stop = true;
            self.stop(STOP_TIMEOUT);
        } else {
            self.start_next_queued();
        }
    }

    fn resume_queue(&mut self, sink: &Sink) {
        match self
            .store
            .with(|store| store.set_queue_paused(self.id, false))
        {
            Ok(()) => {
                self.broadcast_queue();
                self.start_next_queued();
            }
            Err(error) => report(sink, error.to_string()),
        }
    }

    /// Tells every client about a queue change, or tells the one that asked why
    /// nothing changed.
    fn queue_changed(&mut self, changed: Result<bool, StoreError>, sink: &Sink) -> bool {
        match changed {
            Ok(true) => {
                self.broadcast_queue();
                true
            }
            Ok(false) => {
                report(sink, "This message is no longer in the queue.");
                false
            }
            Err(error) => {
                report(sink, error.to_string());
                false
            }
        }
    }

    fn queue(&self) -> Result<QueueState, StoreError> {
        self.store.with(|store| store.queue(self.id))
    }

    fn broadcast_queue(&mut self) {
        match self.queue() {
            Ok(queue) => self.broadcast(ServerMessage::QueueChanged {
                conversation_id: self.id,
                queue,
            }),
            Err(error) => self.report_store(Err::<(), _>(error)),
        }
    }

    fn emit(&mut self, event: Event) {
        // A turn that answered, ran a tool, or asked to run one is not sent
        // again: a retry could run twice what a person already allowed.
        if matches!(
            event,
            Event::MessageText(_)
                | Event::ToolCallStarted { .. }
                | Event::ApprovalRequested { .. }
                | Event::ApprovalResolved { .. }
        ) {
            self.running.said = true;
        }
        let recorded = self.recorder.record(event.clone());
        self.report_store(recorded);
        self.broadcast(ServerMessage::ConversationEvent {
            conversation_id: self.id,
            event,
            at_ms: at_ms(),
        });
    }

    fn report_store<T>(&mut self, result: Result<T, StoreError>) {
        if let Err(error) = result {
            self.broadcast(ServerMessage::Error {
                message: error.to_string(),
            });
        }
    }

    fn broadcast(&mut self, message: ServerMessage) {
        let attached = self.subscribers.len();
        self.subscribers
            .retain(|subscriber| subscriber.sink.send(message.clone()).is_ok());
        if self.subscribers.len() < attached {
            // A client that is gone no longer follows the conversation.
            let subscribers = &self.subscribers;
            self.followers.retain(|connection| {
                subscribers
                    .iter()
                    .any(|subscriber| subscriber.connection == *connection)
            });
        }
    }
}

fn report(sink: &Sink, message: impl Into<String>) {
    sink.send(ServerMessage::Error {
        message: message.into(),
    })
    .ok();
}

fn at_ms() -> i64 {
    i64::try_from(now_ms()).unwrap_or(i64::MAX)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}
