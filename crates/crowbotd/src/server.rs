use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crowbot_protocol::transport::{Listener, MessageReader, MessageWriter};
use crowbot_protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};
use tokio::sync::mpsc;

use crate::conversation::{Command, Sink};
use crate::daemon::{Daemon, Refusal};

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
    let id = daemon.next_connection();
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
            Ok(Some(message)) => match client.handle(&daemon, message, &sink) {
                Ok(()) => {}
                Err(Closing::Silent) => break,
                Err(Closing::Error(message)) => {
                    sink.send(ServerMessage::Error { message }).ok();
                    break;
                }
            },
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

    daemon.detach(id);
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

const INCOMPATIBLE_SINCE: u32 = 4;

enum Closing {
    Error(String),
    Silent,
}

struct ClientState {
    id: u64,
    greeted: bool,
    open: HashSet<i64>,
}

impl ClientState {
    fn handle(
        &mut self,
        daemon: &Arc<Daemon>,
        message: ClientMessage,
        sink: &Sink,
    ) -> Result<(), Closing> {
        if let ClientMessage::Hello { protocol_version } = message {
            if protocol_version != PROTOCOL_VERSION {
                if protocol_version >= INCOMPATIBLE_SINCE {
                    sink.send(ServerMessage::Incompatible {
                        protocol_version: PROTOCOL_VERSION,
                        daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
                    })
                    .ok();
                    return Err(Closing::Silent);
                }
                return Err(Closing::Error(format!(
                    "this daemon speaks protocol version {PROTOCOL_VERSION}, the client speaks {protocol_version}"
                )));
            }
            self.greeted = true;
            sink.send(ServerMessage::Welcome {
                protocol_version: PROTOCOL_VERSION,
                daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            })
            .ok();
            daemon.attach(self.id, sink.clone());
            return Ok(());
        }
        if !self.greeted {
            return Err(Closing::Error(
                "the first message must be a hello".to_owned(),
            ));
        }
        let reply = |message: ServerMessage| {
            sink.send(message).ok();
        };
        // The change itself reaches every client; the one that asked also
        // learns which bot it made.
        let created = |made: Result<i64, Refusal>| {
            made.map(|bot_id| reply(ServerMessage::BotCreated { bot_id }))
        };
        let failed = Refusal::Other;
        let outcome = match message {
            ClientMessage::Hello { .. } => Ok(()),
            ClientMessage::ListBots => daemon.list_bots().map(reply).map_err(failed),
            ClientMessage::CreateBot { draft } => created(daemon.create_bot(draft)),
            ClientMessage::DuplicateBot { bot_id } => created(daemon.duplicate_bot(bot_id)),
            ClientMessage::UpdateBot { bot_id, patch } => daemon
                .update_bot(bot_id, patch)
                .map(|()| reply(ServerMessage::BotUpdated { bot_id })),
            ClientMessage::DeleteBot { bot_id } => {
                let (daemon, sink) = (daemon.clone(), sink.clone());
                tokio::spawn(async move {
                    if let Err(message) = daemon.delete_bot(bot_id).await {
                        sink.send(ServerMessage::Error { message }).ok();
                    }
                });
                Ok(())
            }
            ClientMessage::DeleteHistory { bot_id } => {
                let (daemon, sink) = (daemon.clone(), sink.clone());
                tokio::spawn(async move {
                    if let Err(message) = daemon.delete_history(bot_id).await {
                        sink.send(ServerMessage::Error { message }).ok();
                    }
                });
                Ok(())
            }
            ClientMessage::ListProviders => {
                reply(daemon.list_providers());
                Ok(())
            }
            ClientMessage::CheckProvider { provider } => {
                daemon.check_provider(&provider).map_err(failed)
            }
            ClientMessage::SignIn { provider, method } => {
                daemon.sign_in(&provider, method.as_deref()).map_err(failed)
            }
            ClientMessage::InstallRuntime { provider } => {
                daemon.install_runtime(&provider).map_err(failed)
            }
            ClientMessage::SetProviderExecutable { provider, path } => {
                daemon.set_provider_executable(&provider, path)
            }
            ClientMessage::ListModels { provider } => {
                daemon.list_models(&provider).map(reply).map_err(failed)
            }
            ClientMessage::CancelSignIn { provider } => {
                daemon.cancel_sign_in(&provider).map_err(failed)
            }
            ClientMessage::GetDaemonInfo => {
                reply(daemon.info());
                Ok(())
            }
            ClientMessage::Search { request, query } => {
                daemon.search(request, &query).map(reply).map_err(failed)
            }
            ClientMessage::ListConversations { bot_id } => {
                daemon.list_conversations(bot_id).map(reply).map_err(failed)
            }
            ClientMessage::NewConversation { bot_id } => daemon
                .new_conversation(bot_id)
                .map(|summary| reply(ServerMessage::ConversationCreated(summary)))
                .map_err(failed),
            ClientMessage::MarkRead {
                conversation_id,
                following,
            } => daemon
                .conversation(conversation_id)
                .map(|handle| {
                    handle.send(Command::MarkRead {
                        connection: self.id,
                        following,
                        sink: sink.clone(),
                    });
                })
                .map_err(failed),
            ClientMessage::MarkBotRead { bot_id } => daemon.mark_bot_read(bot_id).map_err(failed),
            ClientMessage::MarkBotUnread { bot_id } => {
                daemon.mark_bot_unread(bot_id).map_err(failed)
            }
            ClientMessage::GetPreferences => {
                reply(daemon.preferences());
                Ok(())
            }
            ClientMessage::SetPreferences { patch } => daemon.set_preferences(&patch),
            ClientMessage::OpenConversation { conversation_id } => daemon
                .conversation(conversation_id)
                .map(|handle| {
                    self.open.insert(conversation_id);
                    handle.send(Command::Subscribe {
                        connection: self.id,
                        sink: sink.clone(),
                    });
                })
                .map_err(failed),
            ClientMessage::CloseConversation { conversation_id } => {
                self.open.remove(&conversation_id);
                daemon
                    .conversation(conversation_id)
                    .map(|handle| {
                        handle.send(Command::Unsubscribe {
                            connection: self.id,
                        });
                    })
                    .map_err(failed)
            }
            ClientMessage::SendMessage {
                conversation_id,
                text,
            } => {
                match daemon.conversation(conversation_id) {
                    Ok(handle) => handle.send(Command::Prompt {
                        text,
                        sink: sink.clone(),
                    }),
                    Err(message) => reply(ServerMessage::MessageRefused {
                        conversation_id,
                        message,
                    }),
                }
                Ok(())
            }
            ClientMessage::LoadOlderEvents {
                conversation_id,
                before,
            } => {
                match daemon.conversation(conversation_id) {
                    Ok(handle) => handle.send(Command::LoadOlder {
                        before,
                        sink: sink.clone(),
                    }),
                    Err(message) => reply(ServerMessage::OlderEventsFailed {
                        conversation_id,
                        message,
                    }),
                }
                Ok(())
            }
            ClientMessage::RevealEvent {
                conversation_id,
                before,
                event_id,
            } => {
                match daemon.conversation(conversation_id) {
                    Ok(handle) => handle.send(Command::Reveal {
                        event_id,
                        before,
                        sink: sink.clone(),
                    }),
                    Err(message) => reply(ServerMessage::OlderEventsFailed {
                        conversation_id,
                        message,
                    }),
                }
                Ok(())
            }
            ClientMessage::LoadNewerEvents {
                conversation_id,
                after,
                until,
            } => {
                match daemon.conversation(conversation_id) {
                    Ok(handle) => handle.send(Command::LoadNewer {
                        after,
                        until,
                        sink: sink.clone(),
                    }),
                    Err(message) => reply(ServerMessage::NewerEventsFailed {
                        conversation_id,
                        message,
                    }),
                }
                Ok(())
            }
            ClientMessage::SetDraft {
                conversation_id,
                text,
            } => daemon
                .conversation(conversation_id)
                .map(|handle| {
                    handle.send(Command::SetDraft {
                        text,
                        sink: sink.clone(),
                    });
                })
                .map_err(failed),
            ClientMessage::ResolveApproval {
                conversation_id,
                approval_id,
                decision,
            } => {
                match daemon.conversation(conversation_id) {
                    Ok(handle) => handle.send(Command::ResolveApproval {
                        approval_id,
                        decision,
                        sink: sink.clone(),
                    }),
                    Err(message) => reply(ServerMessage::ApprovalNotResolved {
                        conversation_id,
                        approval_id,
                        message,
                    }),
                }
                Ok(())
            }
            ClientMessage::StopTurn { conversation_id } => daemon
                .conversation(conversation_id)
                .map(|handle| handle.send(Command::Stop))
                .map_err(failed),
            ClientMessage::EditQueuedMessage {
                conversation_id,
                queue_id,
                text,
            } => daemon
                .conversation(conversation_id)
                .map(|handle| {
                    handle.send(Command::EditQueued {
                        queue_id,
                        text,
                        sink: sink.clone(),
                    });
                })
                .map_err(failed),
            ClientMessage::DeleteQueuedMessage {
                conversation_id,
                queue_id,
            } => daemon
                .conversation(conversation_id)
                .map(|handle| {
                    handle.send(Command::DeleteQueued {
                        queue_id,
                        sink: sink.clone(),
                    });
                })
                .map_err(failed),
            ClientMessage::SendQueuedMessageNow {
                conversation_id,
                queue_id,
            } => daemon
                .conversation(conversation_id)
                .map(|handle| {
                    handle.send(Command::SendQueuedNow {
                        queue_id,
                        sink: sink.clone(),
                    });
                })
                .map_err(failed),
            ClientMessage::ResumeQueue { conversation_id } => daemon
                .conversation(conversation_id)
                .map(|handle| {
                    handle.send(Command::ResumeQueue { sink: sink.clone() });
                })
                .map_err(failed),
        };
        if let Err(refusal) = outcome {
            reply(refusal.into_message());
        }
        Ok(())
    }
}
