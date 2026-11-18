use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crowbot_protocol::{
    ApprovalClosure, ApprovalDecision, ApprovalReason, Bot, ConversationSnapshot, Event,
    FailureCause, Interruption, MESSAGE_LENGTH, PermissionLevel, QueueState, ServerMessage,
    TurnEnd,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use crate::daemon::Daemon;
use crate::handoff::{self, HANDOFF_CHARACTERS};
use crate::provider::acp::{
    self, AgentEvent, ApprovalAnswer, SessionCommand, SessionConfig, SessionFailure,
};
use crate::store::{
    BotRow, Conversation, EventsPage, Recorder, SharedStore, StoreError, session_fingerprint,
};

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
        stop_due: None,
        next_after_stop: false,
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
    /// Set while a stop waits for the agent to end the turn.
    stop_due: Option<Instant>,
    /// Set by "send now": the next queued message starts however the turn ends.
    next_after_stop: bool,
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
                        self.handle_agent(event);
                    }
                }
                Some(Input::SessionClosed { generation, result }) => {
                    self.session_closed(generation, result);
                }
                None => break,
            }
            if self.finished {
                break;
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        match (self.recorder.save_due(), self.stop_due) {
            (Some(save), Some(stop)) => Some(save.min(stop)),
            (save, stop) => save.or(stop),
        }
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
            Command::Stop => self.stop(STOP_TIMEOUT),
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
        let (conversation, page, draft, queue) = self
            .store
            .with(|store| {
                Ok((
                    store.summary(self.id)?,
                    store.events_page(self.id, None, PAGE_EVENTS)?,
                    store.draft(self.id)?,
                    store.queue(self.id)?,
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
                    self.accept_message(sink);
                    self.broadcast_queue();
                }
                Err(error) => self.refuse_message(sink, error.to_string()),
            }
            return;
        }
        self.turn_active = true;
        self.accept_message(sink);
        self.emit(Event::UserText(text.clone()));
        self.give_title(&text);
        self.sync();
        self.deliver(text);
    }

    fn deliver(&mut self, text: String) {
        match self.try_deliver(text) {
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

    fn try_deliver(&mut self, text: String) -> Result<(), TurnFailure> {
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
        let provider = bot.bot.provider.clone();
        // Another provider, new instructions, a new model, a new reasoning
        // level, or a new folder take a new provider session: the one that
        // runs keeps the old ones.
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.revision != bot.session_revision)
        {
            self.close_session();
        }
        let text = match &self.session {
            Some(session) => {
                let prompt = SessionCommand::Prompt {
                    text,
                    handoff: None,
                };
                match session.commands.send(prompt) {
                    Ok(()) => return Ok(()),
                    Err(mpsc::error::SendError(SessionCommand::Prompt { text, .. })) => text,
                    Err(_) => {
                        return Err(TurnFailure::of_provider(
                            &provider,
                            "the agent stopped before it received the message",
                        ));
                    }
                }
            }
            None => text,
        };
        let session = self
            .start_session(bot, text)
            .map_err(|message| TurnFailure::of_provider(&provider, message))?;
        self.session = Some(session);
        Ok(())
    }

    /// Starts the provider session that runs the message. The session is the
    /// one the provider holds for this conversation when it was started with
    /// the settings the bot has now, and a new one otherwise. When another
    /// provider ran the conversation last, the message comes after what the
    /// session has not seen of it.
    fn start_session(&mut self, bot: BotRow, text: String) -> Result<Session, String> {
        let revision = bot.session_revision;
        let bot = bot.bot;
        let provider = bot.provider.clone();
        let fingerprint = session_fingerprint(&bot);
        self.recorder.save().map_err(|error| error.to_string())?;
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
        let first = vec![SessionCommand::Prompt { text, handoff }];
        let resume = resumed.map(|session| session.session_id);
        let settings = bot.clone();

        self.generation += 1;
        let generation = self.generation;
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
                    instructions: settings.instructions,
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
        })
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
                let provider = session.bot.provider.clone();
                let fingerprint = session_fingerprint(&session.bot);
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
                        )
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
            AgentEvent::Event(event) => {
                if event.ends_turn() {
                    // A turn ends once: an end that arrives after the turn was
                    // already closed is dropped.
                    if self.turn_active {
                        self.end_turn(event);
                    }
                } else {
                    self.emit(event);
                }
            }
            AgentEvent::ApprovalNeeded {
                tool_call_id,
                title,
                target,
                cut,
                reply,
            } => {
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
        let completed = matches!(end, Event::TurnEnded(_));
        self.emit(end);
        if !self.followers.is_empty() {
            // A client has the end of the conversation on screen: the turn
            // that just ended is read before the bot's state is told.
            let marked = self.store.with(|store| store.mark_read(self.id));
            self.report_store(marked);
        }
        self.turn_active = false;
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

    fn start_next_queued(&mut self) {
        if self.turn_active {
            return;
        }
        match self.store.with(|store| store.start_queued(self.id)) {
            Ok(Some(text)) => {
                self.turn_active = true;
                self.broadcast(ServerMessage::ConversationEvent {
                    conversation_id: self.id,
                    event: Event::UserText(text.clone()),
                    at_ms: at_ms(),
                });
                self.broadcast_queue();
                self.give_title(&text);
                self.sync();
                self.deliver(text);
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
