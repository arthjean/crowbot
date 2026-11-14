use std::path::PathBuf;
use std::str::FromStr;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    ResumeSessionRequest, SelectedPermissionOutcome, SessionId, SessionModeId, SessionModeState,
    SessionNotification, SessionUpdate, SetSessionModeRequest, StopReason, TextContent,
    ToolCallStatus,
};
use agent_client_protocol::{AcpAgent, Agent, Client, Error};
use crowbot_protocol::{Event, ToolStatus, TurnEnd};
use serde_json::{Map, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

pub struct AgentProfile {
    pub command: String,
    pub session_meta: Option<Map<String, Value>>,
    pub session_mode: Option<String>,
}

impl AgentProfile {
    pub fn generic(command: String) -> Self {
        Self {
            command,
            session_meta: None,
            session_mode: None,
        }
    }
}

pub struct SessionConfig {
    pub agent: AgentProfile,
    pub cwd: PathBuf,
    pub resume: Option<String>,
}

pub enum AgentEvent {
    Started {
        provider_session_id: String,
    },
    Event(Event),
    ApprovalNeeded {
        tool_call_id: String,
        title: Option<String>,
        reply: oneshot::Sender<ApprovalAnswer>,
    },
    /// The agent asked for an approval that the daemon cannot grant, because the
    /// request offers no allow-once option. It was declined at once.
    ApprovalRefused {
        tool_call_id: String,
        title: Option<String>,
    },
}

pub enum SessionCommand {
    Prompt(String),
    /// Asks the agent to end the running turn, then answers the approvals that
    /// waited as cancelled, in that order: an agent that reads its cancelled
    /// requests already knows that the turn is stopping. The turn still ends
    /// through the answer to its prompt, with the stop reason the agent gives.
    Cancel {
        approvals: Vec<oneshot::Sender<ApprovalAnswer>>,
    },
}

/// What the daemon answers to a permission request.
pub enum ApprovalAnswer {
    Allow,
    Decline,
    /// The turn was stopped while the request waited.
    Cancel,
}

pub async fn run_session(
    config: SessionConfig,
    mut commands: UnboundedReceiver<SessionCommand>,
    events: UnboundedSender<AgentEvent>,
) -> Result<(), String> {
    let SessionConfig { agent, cwd, resume } = config;
    let process = AcpAgent::from_str(&agent.command).map_err(|error| error.to_string())?;
    let update_events = events.clone();
    let approval_events = events.clone();

    Client
        .v1()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                if let Some(event) = translate(notification.update) {
                    update_events.send(AgentEvent::Event(event)).ok();
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest,
                        responder,
                        connection: ConnectionTo<Agent>| {
                let tool_call_id = request.tool_call.tool_call_id.0.to_string();
                let title = request.tool_call.fields.title.clone();
                let decline = decline_outcome(&request);
                let Some(allow) = allow_once_outcome(&request) else {
                    approval_events
                        .send(AgentEvent::ApprovalRefused {
                            tool_call_id,
                            title,
                        })
                        .ok();
                    return responder.respond(RequestPermissionResponse::new(decline));
                };
                let (reply, decision) = oneshot::channel();
                approval_events
                    .send(AgentEvent::ApprovalNeeded {
                        tool_call_id,
                        title,
                        reply,
                    })
                    .ok();
                connection.spawn(async move {
                    let outcome = match decision.await {
                        Ok(ApprovalAnswer::Allow) => allow,
                        Ok(ApprovalAnswer::Decline) | Err(_) => decline,
                        Ok(ApprovalAnswer::Cancel) => RequestPermissionOutcome::Cancelled,
                    };
                    responder.respond(RequestPermissionResponse::new(outcome))
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(process, |connection: ConnectionTo<Agent>| async move {
            let initialized = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let (session_id, modes) = match resume {
                Some(provider_session_id) => {
                    if initialized
                        .agent_capabilities
                        .session_capabilities
                        .resume
                        .is_none()
                    {
                        return Err(failure("the agent cannot resume a session"));
                    }
                    let session_id = SessionId::new(provider_session_id);
                    let mut request = ResumeSessionRequest::new(session_id.clone(), cwd);
                    request.meta = agent.session_meta;
                    let resumed = connection.send_request(request).block_task().await?;
                    (session_id, resumed.modes)
                }
                None => {
                    let mut request = NewSessionRequest::new(cwd);
                    request.meta = agent.session_meta;
                    let created = connection.send_request(request).block_task().await?;
                    (created.session_id, created.modes)
                }
            };

            if let Some(mode) = agent.session_mode.as_deref()
                && needs_mode_change(modes.as_ref(), mode)?
            {
                connection
                    .send_request(SetSessionModeRequest::new(
                        session_id.clone(),
                        SessionModeId::new(mode),
                    ))
                    .block_task()
                    .await?;
            }

            events
                .send(AgentEvent::Started {
                    provider_session_id: session_id.0.to_string(),
                })
                .ok();

            while let Some(command) = commands.recv().await {
                let SessionCommand::Prompt(prompt) = command else {
                    continue;
                };
                let mut answer = std::pin::pin!(
                    connection
                        .send_request(PromptRequest::new(
                            session_id.clone(),
                            vec![ContentBlock::Text(TextContent::new(prompt))],
                        ))
                        .block_task()
                );
                let response = loop {
                    tokio::select! {
                        response = &mut answer => break response?,
                        command = commands.recv() => match command {
                            Some(SessionCommand::Cancel { approvals }) => {
                                connection
                                    .send_notification(CancelNotification::new(session_id.clone()))?;
                                for approval in approvals {
                                    approval.send(ApprovalAnswer::Cancel).ok();
                                }
                            }
                            // The conversation sends one prompt at a time.
                            Some(SessionCommand::Prompt(_)) => {}
                            None => return Ok(()),
                        },
                    }
                };
                events
                    .send(AgentEvent::Event(Event::TurnEnded(turn_end(
                        response.stop_reason,
                    ))))
                    .ok();
            }
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())
}

fn failure(message: &str) -> Error {
    Error::internal_error().data(Value::String(message.to_owned()))
}

fn needs_mode_change(modes: Option<&SessionModeState>, mode: &str) -> Result<bool, Error> {
    let offered = modes.is_some_and(|modes| {
        modes
            .available_modes
            .iter()
            .any(|available| available.id.0.as_ref() == mode)
    });
    if !offered {
        return Err(failure(&format!(
            "the agent does not offer the session mode `{mode}`"
        )));
    }
    Ok(modes.is_some_and(|modes| modes.current_mode_id.0.as_ref() != mode))
}

fn select(
    request: &RequestPermissionRequest,
    kind: PermissionOptionKind,
) -> Option<RequestPermissionOutcome> {
    request
        .options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        })
}

fn allow_once_outcome(request: &RequestPermissionRequest) -> Option<RequestPermissionOutcome> {
    select(request, PermissionOptionKind::AllowOnce)
}

fn decline_outcome(request: &RequestPermissionRequest) -> RequestPermissionOutcome {
    select(request, PermissionOptionKind::RejectOnce).unwrap_or(RequestPermissionOutcome::Cancelled)
}

fn translate(update: SessionUpdate) -> Option<Event> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => text(chunk.content).map(Event::MessageText),
        SessionUpdate::AgentThoughtChunk(chunk) => text(chunk.content).map(Event::ThoughtText),
        SessionUpdate::ToolCall(call) => Some(Event::ToolCallStarted {
            id: call.tool_call_id.0.to_string(),
            title: call.title,
            status: tool_status(call.status),
        }),
        SessionUpdate::ToolCallUpdate(update) => {
            let title = update.fields.title;
            let status = update.fields.status.and_then(tool_status);
            (title.is_some() || status.is_some()).then(|| Event::ToolCallUpdated {
                id: update.tool_call_id.0.to_string(),
                title,
                status,
            })
        }
        _ => None,
    }
}

fn text(content: ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text),
        _ => None,
    }
}

fn tool_status(status: ToolCallStatus) -> Option<ToolStatus> {
    match status {
        ToolCallStatus::Pending => Some(ToolStatus::Pending),
        ToolCallStatus::InProgress => Some(ToolStatus::InProgress),
        ToolCallStatus::Completed => Some(ToolStatus::Completed),
        ToolCallStatus::Failed => Some(ToolStatus::Failed),
        _ => None,
    }
}

fn turn_end(reason: StopReason) -> TurnEnd {
    match reason {
        StopReason::EndTurn => TurnEnd::Completed,
        StopReason::MaxTokens => TurnEnd::MaxTokens,
        StopReason::MaxTurnRequests => TurnEnd::MaxRequests,
        StopReason::Refusal => TurnEnd::Refused,
        StopReason::Cancelled => TurnEnd::Cancelled,
        _ => TurnEnd::Other,
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::v1::schema::{
        ContentChunk, PermissionOption, SessionMode, ToolCallUpdate, ToolCallUpdateFields,
    };

    use super::*;

    fn request(kinds: &[PermissionOptionKind]) -> RequestPermissionRequest {
        let options = kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| PermissionOption::new(format!("option-{index}"), "label", *kind))
            .collect();
        RequestPermissionRequest::new(
            SessionId::new("session"),
            ToolCallUpdate::new("tool", ToolCallUpdateFields::new()),
            options,
        )
    }

    fn selected(outcome: &RequestPermissionOutcome) -> Option<String> {
        match outcome {
            RequestPermissionOutcome::Selected(selected) => Some(selected.option_id.0.to_string()),
            _ => None,
        }
    }

    fn modes(current: &str, available: &[&str]) -> SessionModeState {
        SessionModeState::new(
            SessionModeId::new(current),
            available
                .iter()
                .map(|id| SessionMode::new(SessionModeId::new(*id), *id))
                .collect(),
        )
    }

    #[test]
    fn declining_selects_reject_once() {
        let request = request(&[
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
        ]);
        assert_eq!(
            selected(&decline_outcome(&request)).as_deref(),
            Some("option-2")
        );
    }

    #[test]
    fn allowing_selects_allow_once_and_never_allow_always() {
        let request = request(&[
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
        ]);
        let outcome = allow_once_outcome(&request).unwrap();
        assert_eq!(selected(&outcome).as_deref(), Some("option-1"));
    }

    #[test]
    fn a_request_that_only_offers_allow_always_cannot_be_allowed() {
        let request = request(&[
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::RejectOnce,
        ]);
        assert!(allow_once_outcome(&request).is_none());
    }

    #[test]
    fn a_request_without_a_reject_option_is_cancelled() {
        let request = request(&[PermissionOptionKind::AllowAlways]);
        assert!(matches!(
            decline_outcome(&request),
            RequestPermissionOutcome::Cancelled
        ));
    }

    #[test]
    fn a_session_already_in_the_wanted_mode_needs_no_change() {
        let state = modes("default", &["default", "plan"]);
        assert!(!needs_mode_change(Some(&state), "default").unwrap());
    }

    #[test]
    fn a_session_in_another_mode_needs_a_change() {
        let state = modes("bypassPermissions", &["default", "bypassPermissions"]);
        assert!(needs_mode_change(Some(&state), "default").unwrap());
    }

    #[test]
    fn an_agent_that_does_not_offer_the_mode_is_an_error() {
        let state = modes("plan", &["plan"]);
        assert!(needs_mode_change(Some(&state), "default").is_err());
        assert!(needs_mode_change(None, "default").is_err());
    }

    #[test]
    fn message_and_thought_chunks_become_text_events() {
        let chunk = || ContentChunk::new(ContentBlock::Text(TextContent::new("caw")));
        assert_eq!(
            translate(SessionUpdate::AgentMessageChunk(chunk())),
            Some(Event::MessageText("caw".to_owned()))
        );
        assert_eq!(
            translate(SessionUpdate::AgentThoughtChunk(chunk())),
            Some(Event::ThoughtText("caw".to_owned()))
        );
    }

    #[test]
    fn echoed_user_chunks_are_ignored() {
        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("caw")));
        assert_eq!(translate(SessionUpdate::UserMessageChunk(chunk)), None);
    }

    #[test]
    fn stop_reasons_map_to_turn_ends() {
        assert_eq!(turn_end(StopReason::EndTurn), TurnEnd::Completed);
        assert_eq!(turn_end(StopReason::Cancelled), TurnEnd::Cancelled);
        assert_eq!(turn_end(StopReason::Refusal), TurnEnd::Refused);
    }
}
