use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crowbot_protocol::{
    ApprovalClosure, ApprovalDecision, ApprovalReason, ConversationSnapshot, Event, Interruption,
    PermissionLevel, QueueState, ServerMessage,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use crate::daemon::Daemon;
use crate::provider::acp::{self, AgentEvent, ApprovalAnswer, SessionCommand, SessionConfig};
use crate::store::{BotRow, Conversation, Recorder, SharedStore, StoreError};

/// How long an agent gets to end its turn after a stop, before its process is closed.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// The bot is being deleted: the running turn stops, then the task ends
    /// and tells so through `done`.
    ShutDown {
        done: oneshot::Sender<()>,
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
        result: Result<(), String>,
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
    model: Option<String>,
    commands: UnboundedSender<SessionCommand>,
    task: AbortHandle,
}

struct PendingApproval {
    id: String,
    tool_call_id: String,
    title: Option<String>,
    reply: oneshot::Sender<ApprovalAnswer>,
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
            Command::ResolveApproval {
                approval_id,
                decision,
                sink,
            } => self.resolve_approval(&approval_id, decision, &sink),
            Command::Stop => self.stop(),
            Command::EditQueued {
                queue_id,
                text,
                sink,
            } => self.edit_queued(queue_id, &text, &sink),
            Command::DeleteQueued { queue_id, sink } => self.delete_queued(queue_id, &sink),
            Command::SendQueuedNow { queue_id, sink } => self.send_queued_now(queue_id, &sink),
            Command::ResumeQueue { sink } => self.resume_queue(&sink),
            Command::PermissionLevelChanged => self.permission_level_changed(),
            Command::ShutDown { done } => self.shut_down(done),
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
        for approval in std::mem::take(&mut self.approvals) {
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

    fn shut_down(&mut self, done: oneshot::Sender<()>) {
        self.closing = Some(done);
        self.next_after_stop = false;
        self.stop();
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
        let (conversation, events, queue) = self
            .store
            .with(|store| {
                Ok((
                    store.summary(self.id)?,
                    store.events(self.id)?,
                    store.queue(self.id)?,
                ))
            })
            .map_err(|error: StoreError| error.to_string())?;
        let conversation =
            conversation.ok_or_else(|| format!("conversation {} does not exist", self.id))?;
        Ok(ConversationSnapshot {
            conversation,
            events,
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
                .map(|session| session.model.clone()),
        })
    }

    fn prompt(&mut self, text: String, sink: &Sink) {
        if self.closing.is_some() {
            return report(sink, "This bot is being deleted.");
        }
        if text.trim().is_empty() {
            return report(sink, "A message needs some text.");
        }
        // The conversation a person writes in is the one the bot opens on.
        match self.store.with(|store| store.make_current(self.id)) {
            Ok(true) => self.daemon.publish_bot(self.bot_id),
            Ok(false) => {}
            Err(error) => return report(sink, error.to_string()),
        }
        let queue = match self.queue() {
            Ok(queue) => queue,
            Err(error) => return report(sink, error.to_string()),
        };
        // A message never overtakes the ones that already wait.
        if self.turn_active || !queue.messages.is_empty() {
            match self.store.with(|store| store.enqueue(self.id, &text)) {
                Ok(()) => self.broadcast_queue(),
                Err(error) => report(sink, error.to_string()),
            }
            return;
        }
        self.turn_active = true;
        self.emit(Event::UserText(text.clone()));
        self.give_title(&text);
        self.sync();
        self.deliver(text);
    }

    fn deliver(&mut self, text: String) {
        match self.try_deliver(text) {
            Ok(()) => {
                if let Some(model) = self.session.as_ref().map(|session| session.model.clone()) {
                    self.broadcast(ServerMessage::TurnModel {
                        conversation_id: self.id,
                        model,
                    });
                }
            }
            Err(message) => self.end_turn(Event::TurnFailed { message }),
        }
    }

    fn try_deliver(&mut self, text: String) -> Result<(), String> {
        let (conversation, bot) = self
            .store
            .with(|store| {
                Ok::<_, StoreError>((store.conversation(self.id)?, store.bot(self.bot_id)?))
            })
            .map_err(|error| error.to_string())?;
        let conversation =
            conversation.ok_or_else(|| format!("conversation {} does not exist", self.id))?;
        let bot = bot.ok_or_else(|| format!("bot {} does not exist", self.bot_id))?;
        if !Path::new(&bot.bot.workspace).is_dir() {
            self.close_session();
            return Err(format!(
                "The folder {} does not exist. Choose another one in the bot's settings.",
                bot.bot.workspace
            ));
        }
        // New instructions, a new model, or a new folder take a new provider
        // session: the one that runs keeps the old ones.
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.revision != bot.session_revision)
        {
            self.close_session();
        }
        let text = match &self.session {
            Some(session) => match session.commands.send(SessionCommand::Prompt(text)) {
                Ok(()) => return Ok(()),
                Err(mpsc::error::SendError(SessionCommand::Prompt(unsent))) => unsent,
                Err(_) => return Err("the agent stopped before it received the message".to_owned()),
            },
            None => text,
        };
        let session = self.start_session(conversation, bot)?;
        session
            .commands
            .send(SessionCommand::Prompt(text))
            .map_err(|_| "the agent stopped before it received the message".to_owned())?;
        self.session = Some(session);
        Ok(())
    }

    fn start_session(
        &mut self,
        conversation: Conversation,
        bot: BotRow,
    ) -> Result<Session, String> {
        let revision = bot.session_revision;
        let model = bot.bot.model.clone();
        let config = SessionConfig {
            agent: self.daemon.agent_profile(&bot.bot.provider)?,
            cwd: PathBuf::from(bot.bot.workspace),
            // The provider session is resumed only when it was started with
            // the settings the bot has now.
            resume: conversation
                .provider_session_id
                .filter(|_| conversation.session_revision == Some(revision)),
            instructions: bot.bot.instructions,
            model: bot.bot.model,
        };
        self.generation += 1;
        let generation = self.generation;
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let (events, mut event_receiver) = mpsc::unbounded_channel();
        let inputs = self.inputs.clone();
        let task = tokio::spawn(async move {
            let forwarding = inputs.clone();
            let forward = async move {
                while let Some(event) = event_receiver.recv().await {
                    if forwarding.send(Input::Agent { generation, event }).is_err() {
                        break;
                    }
                }
            };
            let (result, ()) =
                tokio::join!(acp::run_session(config, command_receiver, events), forward);
            inputs
                .send(Input::SessionClosed { generation, result })
                .ok();
        });
        Ok(Session {
            generation,
            revision,
            model,
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
                let Some(revision) = self.session.as_ref().map(|session| session.revision) else {
                    return;
                };
                let saved = self.store.with(|store| {
                    store.set_provider_session(self.id, &provider_session_id, revision)
                });
                self.report_store(saved);
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
                    reply,
                });
                self.emit(Event::ApprovalRequested {
                    id,
                    tool_call_id,
                    title,
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
        let Some(index) = self
            .approvals
            .iter()
            .position(|approval| approval.id == approval_id)
        else {
            return report(sink, "This approval is no longer waiting for an answer.");
        };
        let approval = self.approvals.remove(index);
        let answer = match decision {
            ApprovalDecision::Allowed => ApprovalAnswer::Allow,
            ApprovalDecision::Declined => ApprovalAnswer::Decline,
        };
        if approval.reply.send(answer).is_err() {
            return report(sink, "The agent stopped waiting for this approval.");
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

    fn stop(&mut self) {
        if !self.turn_active || self.stop_due.is_some() {
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
            self.stop_due = Some(Instant::now() + STOP_TIMEOUT);
        } else {
            self.close_session();
            self.end_turn(Event::TurnInterrupted {
                cause: Interruption::AgentUnresponsive,
            });
        }
    }

    fn session_closed(&mut self, generation: u64, result: Result<(), String>) {
        if !self.is_current(generation) {
            return;
        }
        self.session = None;
        if self.turn_active {
            self.end_turn(Event::TurnFailed {
                message: result
                    .err()
                    .unwrap_or_else(|| "the agent stopped before the turn ended".to_owned()),
            });
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
            self.stop();
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}
