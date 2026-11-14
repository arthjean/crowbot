use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crowbot_protocol::transport::{Listener, MessageReader, MessageWriter};
use crowbot_protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};
use tokio::sync::mpsc;

use crate::conversation::{self, CUSTOM_PROVIDER, Command, ConversationHandle, Sink};
use crate::provider::claude;
use crate::store::SharedStore;

pub struct Daemon {
    store: SharedStore,
    cwd: PathBuf,
    command: Option<String>,
    conversations: Mutex<HashMap<i64, ConversationHandle>>,
    next_connection: AtomicU64,
}

impl Daemon {
    pub fn new(store: SharedStore, cwd: PathBuf, command: Option<String>) -> Self {
        Self {
            store,
            cwd,
            command,
            conversations: Mutex::new(HashMap::new()),
            next_connection: AtomicU64::new(1),
        }
    }

    fn conversation(&self, id: i64) -> Result<ConversationHandle, String> {
        let mut conversations = self
            .conversations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(handle) = conversations.get(&id) {
            return Ok(handle.clone());
        }
        self.store
            .with(|store| store.conversation(id))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("conversation {id} does not exist"))?;
        let handle = conversation::spawn(self.store.clone(), id);
        conversations.insert(id, handle.clone());
        Ok(handle)
    }

    fn create_conversation(&self) -> Result<ServerMessage, String> {
        let provider = if self.command.is_some() {
            CUSTOM_PROVIDER
        } else {
            claude::PROVIDER
        };
        self.store
            .with(|store| {
                let created =
                    store.create_conversation(provider, self.command.as_deref(), &self.cwd)?;
                store.summary(created.id)
            })
            .map_err(|error| error.to_string())?
            .map(ServerMessage::ConversationCreated)
            .ok_or_else(|| "the new conversation cannot be read back".to_owned())
    }

    fn list_conversations(&self) -> Result<ServerMessage, String> {
        self.store
            .with(|store| store.summaries())
            .map(ServerMessage::Conversations)
            .map_err(|error| error.to_string())
    }
}

pub async fn serve(daemon: Arc<Daemon>, mut listener: Listener) {
    loop {
        match listener.accept().await {
            Ok((reader, writer)) => {
                tokio::spawn(connection(daemon.clone(), reader, writer));
            }
            Err(error) => {
                eprintln!("[crowbotd] cannot accept a client: {error}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn connection(daemon: Arc<Daemon>, mut reader: MessageReader, mut writer: MessageWriter) {
    let id = daemon.next_connection.fetch_add(1, Ordering::Relaxed);
    let (sink, mut outgoing) = mpsc::unbounded_channel::<ServerMessage>();
    let mut writing = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if writer.write(&message).await.is_err() {
                break;
            }
        }
    });
    let mut client = ClientState {
        id,
        greeted: false,
        open: HashSet::new(),
    };

    loop {
        match reader.read::<ClientMessage>().await {
            Ok(Some(message)) => {
                if let Err(message) = client.handle(&daemon, message, &sink) {
                    sink.send(ServerMessage::Error { message }).ok();
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                sink.send(ServerMessage::Error {
                    message: error.to_string(),
                })
                .ok();
                break;
            }
        }
    }

    for conversation_id in client.open {
        if let Ok(handle) = daemon.conversation(conversation_id) {
            handle.send(Command::Unsubscribe { connection: id });
        }
    }
    drop(sink);
    if tokio::time::timeout(Duration::from_secs(1), &mut writing)
        .await
        .is_err()
    {
        writing.abort();
    }
}

struct ClientState {
    id: u64,
    greeted: bool,
    open: HashSet<i64>,
}

impl ClientState {
    fn handle(
        &mut self,
        daemon: &Daemon,
        message: ClientMessage,
        sink: &Sink,
    ) -> Result<(), String> {
        if let ClientMessage::Hello { protocol_version } = message {
            if protocol_version != PROTOCOL_VERSION {
                return Err(format!(
                    "this daemon speaks protocol version {PROTOCOL_VERSION}, the client speaks {protocol_version}"
                ));
            }
            self.greeted = true;
            sink.send(ServerMessage::Welcome {
                protocol_version: PROTOCOL_VERSION,
                daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            })
            .ok();
            return Ok(());
        }
        if !self.greeted {
            return Err("the first message must be a hello".to_owned());
        }
        let outcome = match message {
            ClientMessage::Hello { .. } => Ok(()),
            ClientMessage::ListConversations => daemon.list_conversations().map(|reply| {
                sink.send(reply).ok();
            }),
            ClientMessage::CreateConversation => daemon.create_conversation().map(|reply| {
                sink.send(reply).ok();
            }),
            ClientMessage::OpenConversation { conversation_id } => {
                daemon.conversation(conversation_id).map(|handle| {
                    self.open.insert(conversation_id);
                    handle.send(Command::Subscribe {
                        connection: self.id,
                        sink: sink.clone(),
                    });
                })
            }
            ClientMessage::CloseConversation { conversation_id } => {
                self.open.remove(&conversation_id);
                daemon.conversation(conversation_id).map(|handle| {
                    handle.send(Command::Unsubscribe {
                        connection: self.id,
                    });
                })
            }
            ClientMessage::SendMessage {
                conversation_id,
                text,
            } => daemon.conversation(conversation_id).map(|handle| {
                handle.send(Command::Prompt {
                    text,
                    sink: sink.clone(),
                });
            }),
            ClientMessage::ResolveApproval {
                conversation_id,
                approval_id,
                decision,
            } => daemon.conversation(conversation_id).map(|handle| {
                handle.send(Command::ResolveApproval {
                    approval_id,
                    decision,
                    sink: sink.clone(),
                });
            }),
            ClientMessage::StopTurn { conversation_id } => daemon
                .conversation(conversation_id)
                .map(|handle| handle.send(Command::Stop)),
            ClientMessage::EditQueuedMessage {
                conversation_id,
                queue_id,
                text,
            } => daemon.conversation(conversation_id).map(|handle| {
                handle.send(Command::EditQueued {
                    queue_id,
                    text,
                    sink: sink.clone(),
                });
            }),
            ClientMessage::DeleteQueuedMessage {
                conversation_id,
                queue_id,
            } => daemon.conversation(conversation_id).map(|handle| {
                handle.send(Command::DeleteQueued {
                    queue_id,
                    sink: sink.clone(),
                });
            }),
            ClientMessage::SendQueuedMessageNow {
                conversation_id,
                queue_id,
            } => daemon.conversation(conversation_id).map(|handle| {
                handle.send(Command::SendQueuedNow {
                    queue_id,
                    sink: sink.clone(),
                });
            }),
            ClientMessage::ResumeQueue { conversation_id } => {
                daemon.conversation(conversation_id).map(|handle| {
                    handle.send(Command::ResumeQueue { sink: sink.clone() });
                })
            }
        };
        if let Err(message) = outcome {
            sink.send(ServerMessage::Error { message }).ok();
        }
        Ok(())
    }
}
