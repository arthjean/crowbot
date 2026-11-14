use std::time::Duration;

use crowbot_protocol::transport::{self, MessageReader, MessageWriter};
use crowbot_protocol::{ClientMessage, Endpoint, PROTOCOL_VERSION, ServerMessage};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

const RETRY_DELAY: Duration = Duration::from_secs(1);

pub enum ConnectionEvent {
    Connected,
    Disconnected(String),
    Message(ServerMessage),
}

#[derive(Clone)]
pub struct Connection {
    outgoing: UnboundedSender<ClientMessage>,
}

impl Connection {
    pub fn send(&self, message: ClientMessage) {
        self.outgoing.send(message).ok();
    }
}

pub fn start(
    endpoint: Endpoint,
) -> std::io::Result<(Connection, UnboundedReceiver<ConnectionEvent>)> {
    let (outgoing, outgoing_receiver) = mpsc::unbounded_channel();
    let (events, event_receiver) = mpsc::unbounded_channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    std::thread::Builder::new()
        .name("daemon-connection".to_owned())
        .spawn(move || runtime.block_on(run(endpoint, outgoing_receiver, events)))?;
    Ok((Connection { outgoing }, event_receiver))
}

async fn run(
    endpoint: Endpoint,
    mut outgoing: UnboundedReceiver<ClientMessage>,
    events: UnboundedSender<ConnectionEvent>,
) {
    let mut reported: Option<String> = None;
    loop {
        let reason = match transport::connect(&endpoint).await {
            Ok((reader, writer)) => {
                while outgoing.try_recv().is_ok() {}
                reported = None;
                serve(reader, writer, &mut outgoing, &events).await
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                format!("No daemon is listening on {endpoint}.")
            }
            Err(error) => format!("Cannot reach the daemon on {endpoint}: {error}"),
        };
        if reported.as_ref() != Some(&reason) {
            if events
                .send(ConnectionEvent::Disconnected(reason.clone()))
                .is_err()
            {
                return;
            }
            reported = Some(reason);
        }
        if events.is_closed() {
            return;
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

async fn serve(
    mut reader: MessageReader,
    mut writer: MessageWriter,
    outgoing: &mut UnboundedReceiver<ClientMessage>,
    events: &UnboundedSender<ConnectionEvent>,
) -> String {
    let hello = ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
    };
    if let Err(error) = writer.write(&hello).await {
        return error.to_string();
    }
    match reader.read::<ServerMessage>().await {
        Ok(Some(ServerMessage::Welcome { .. })) => {}
        Ok(Some(ServerMessage::Error { message })) => {
            return format!("The daemon refused the connection: {message}");
        }
        Ok(Some(_)) => return "The daemon did not answer the greeting.".to_owned(),
        Ok(None) => return "The daemon closed the connection.".to_owned(),
        Err(error) => return error.to_string(),
    }
    if events.send(ConnectionEvent::Connected).is_err() {
        return String::new();
    }
    loop {
        tokio::select! {
            incoming = reader.read::<ServerMessage>() => match incoming {
                Ok(Some(message)) => {
                    if events.send(ConnectionEvent::Message(message)).is_err() {
                        return String::new();
                    }
                }
                Ok(None) => return "The daemon closed the connection.".to_owned(),
                Err(error) => return error.to_string(),
            },
            message = outgoing.recv() => match message {
                Some(message) => {
                    if let Err(error) = writer.write(&message).await {
                        return error.to_string();
                    }
                }
                None => return String::new(),
            },
        }
    }
}
