use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crowbot_protocol::{
    ApprovalClosure, ApprovalDecision, ConversationSnapshot, Event, Interruption, QueueState,
    ServerMessage,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use crate::provider::acp::{
    self, AgentEvent, AgentProfile, ApprovalAnswer, SessionCommand, SessionConfig,
};
use crate::provider::claude;
use crate::store::{Conversation, Recorder, SharedStore, StoreError};

pub const CUSTOM_PROVIDER: &str = "custom";

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

pub fn spawn(store: SharedStore, conversation_id: i64) -> ConversationHandle {
    let (inputs, receiver) = mpsc::unbounded_channel();
    let actor = Actor {
        id: conversation_id,
        recorder: Recorder::new(store.clone(), conversation_id),
        store,
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
            }
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
        })
    }

    fn prompt(&mut self, text: String, sink: &Sink) {
        if text.trim().is_empty() {
            return report(sink, "A message needs some text.");
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
        self.deliver(text);
    }

    fn deliver(&mut self, text: String) {
        if let Err(message) = self.try_deliver(text) {
            self.end_turn(Event::TurnFailed { message });
        }
    }

    fn try_deliver(&mut self, text: String) -> Result<(), String> {
        let text = match &self.session {
            Some(session) => match session.commands.send(SessionCommand::Prompt(text)) {
                Ok(()) => return Ok(()),
                Err(mpsc::error::SendError(SessionCommand::Prompt(unsent))) => unsent,
                Err(_) => return Err("the agent stopped before it received the message".to_owned()),
            },
            None => text,
        };
        let session = self.start_session()?;
        session
            .commands
            .send(SessionCommand::Prompt(text))
            .map_err(|_| "the agent stopped before it received the message".to_owned())?;
        self.session = Some(session);
        Ok(())
    }

    fn start_session(&mut self) -> Result<Session, String> {
        let conversation = self
            .store
            .with(|store| store.conversation(self.id))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("conversation {} does not exist", self.id))?;
        let config = SessionConfig {
            agent: agent_profile(&conversation)?,
            cwd: conversation.cwd,
            resume: conversation.provider_session_id,
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
                let saved = self
                    .store
                    .with(|store| store.set_provider_session(self.id, &provider_session_id));
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
            }
            AgentEvent::ApprovalRefused {
                tool_call_id,
                title,
            } => self.emit(Event::ApprovalResolved {
                id: None,
                tool_call_id,
                title,
                decision: ApprovalDecision::Declined,
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
        });
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
    /// or pauses the queue.
    fn end_turn(&mut self, end: Event) {
        for reply in self.close_approvals() {
            reply.send(ApprovalAnswer::Cancel).ok();
        }
        let completed = matches!(end, Event::TurnEnded(_));
        self.emit(end);
        self.turn_active = false;
        self.stop_due = None;
        let closed = self.recorder.close();
        self.report_store(closed);

        let send_now = std::mem::take(&mut self.next_after_stop);
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
        self.subscribers
            .retain(|subscriber| subscriber.sink.send(message.clone()).is_ok());
    }
}

fn report(sink: &Sink, message: impl Into<String>) {
    sink.send(ServerMessage::Error {
        message: message.into(),
    })
    .ok();
}

fn agent_profile(conversation: &Conversation) -> Result<AgentProfile, String> {
    match (conversation.provider.as_str(), &conversation.command) {
        (claude::PROVIDER, _) => Ok(claude::profile()),
        (CUSTOM_PROVIDER, Some(command)) => Ok(AgentProfile::generic(command.clone())),
        (provider, _) => Err(format!("unknown provider: {provider}")),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}
