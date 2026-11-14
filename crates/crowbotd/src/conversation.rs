use std::time::{SystemTime, UNIX_EPOCH};

use crowbot_protocol::{ApprovalDecision, ConversationSnapshot, Event, ServerMessage};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::provider::acp::{self, AgentEvent, AgentProfile, SessionConfig};
use crate::provider::claude;
use crate::store::{Conversation, Recorder, SharedStore};

pub const CUSTOM_PROVIDER: &str = "custom";

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
}

enum Input {
    Command(Command),
    Agent(AgentEvent),
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
    prompts: UnboundedSender<String>,
}

struct PendingApproval {
    id: String,
    tool_call_id: String,
    title: Option<String>,
    reply: oneshot::Sender<ApprovalDecision>,
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
}

impl Actor {
    async fn run(mut self, mut inputs: UnboundedReceiver<Input>) {
        while let Some(input) = inputs.recv().await {
            match input {
                Input::Command(command) => self.handle_command(command),
                Input::Agent(event) => self.handle_agent(event),
                Input::SessionClosed { generation, result } => {
                    self.session_closed(generation, result);
                }
            }
        }
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
        self.recorder.flush().map_err(|error| error.to_string())?;
        let (conversation, events) = self
            .store
            .with(|store| Ok((store.summary(self.id)?, store.events(self.id)?)))
            .map_err(|error: crate::store::StoreError| error.to_string())?;
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
        })
    }

    fn prompt(&mut self, text: String, sink: &Sink) {
        if text.trim().is_empty() {
            return report(sink, "A message needs some text.");
        }
        if self.turn_active {
            return report(sink, "The bot is still working on the previous message.");
        }
        self.turn_active = true;
        self.emit(Event::UserText(text.clone()));
        if let Err(message) = self.deliver(text) {
            self.fail_turn(message);
        }
    }

    fn deliver(&mut self, text: String) -> Result<(), String> {
        let text = match &self.session {
            Some(session) => match session.prompts.send(text) {
                Ok(()) => return Ok(()),
                Err(unsent) => unsent.0,
            },
            None => text,
        };
        let session = self.start_session()?;
        session
            .prompts
            .send(text)
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
        let (prompts, prompt_receiver) = mpsc::unbounded_channel();
        let (events, mut event_receiver) = mpsc::unbounded_channel();
        let inputs = self.inputs.clone();
        tokio::spawn(async move {
            let forwarding = inputs.clone();
            let forward = async move {
                while let Some(event) = event_receiver.recv().await {
                    if forwarding.send(Input::Agent(event)).is_err() {
                        break;
                    }
                }
            };
            let (result, ()) =
                tokio::join!(acp::run_session(config, prompt_receiver, events), forward);
            inputs
                .send(Input::SessionClosed { generation, result })
                .ok();
        });
        Ok(Session {
            generation,
            prompts,
        })
    }

    fn handle_agent(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Started {
                provider_session_id,
            } => {
                let saved = self
                    .store
                    .with(|store| store.set_provider_session(self.id, &provider_session_id));
                if let Err(error) = saved {
                    self.broadcast(ServerMessage::Error {
                        message: error.to_string(),
                    });
                }
            }
            AgentEvent::Event(event) => {
                let ends_turn = event.ends_turn();
                self.emit(event);
                if ends_turn {
                    self.finish_turn();
                }
            }
            AgentEvent::ApprovalNeeded {
                tool_call_id,
                title,
                reply,
            } => {
                self.next_approval += 1;
                let id = format!("approval-{}", self.next_approval);
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
        }
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
        if approval.reply.send(decision).is_err() {
            return report(sink, "The agent stopped waiting for this approval.");
        }
        self.emit(Event::ApprovalResolved {
            id: Some(approval.id),
            tool_call_id: approval.tool_call_id,
            title: approval.title,
            decision,
        });
    }

    fn session_closed(&mut self, generation: u64, result: Result<(), String>) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.generation != generation)
        {
            return;
        }
        self.session = None;
        if self.turn_active {
            self.fail_turn(
                result
                    .err()
                    .unwrap_or_else(|| "the agent stopped before the turn ended".to_owned()),
            );
        }
    }

    fn fail_turn(&mut self, message: String) {
        self.emit(Event::TurnFailed { message });
        self.finish_turn();
    }

    fn finish_turn(&mut self) {
        self.turn_active = false;
        self.approvals.clear();
        if let Err(error) = self.recorder.flush() {
            self.broadcast(ServerMessage::Error {
                message: error.to_string(),
            });
        }
    }

    fn emit(&mut self, event: Event) {
        if let Err(error) = self.recorder.record(event.clone()) {
            self.broadcast(ServerMessage::Error {
                message: error.to_string(),
            });
        }
        self.broadcast(ServerMessage::ConversationEvent {
            conversation_id: self.id,
            event,
        });
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
