use std::path::PathBuf;
use std::str::FromStr;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    ContentBlock, InitializeRequest, NewSessionRequest, NewSessionResponse, PermissionOptionKind,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionModeId, SessionNotification, SessionUpdate,
    SetSessionModeRequest, StopReason, TextContent, ToolCallStatus,
};
use agent_client_protocol::{AcpAgent, Agent, Client, Error};
use serde_json::{Map, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::event::{ApprovalDecision, ApprovalPolicy, Event, ToolStatus, TurnEnd};

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

pub struct Turn {
    pub agent: AgentProfile,
    pub cwd: PathBuf,
    pub prompt: String,
    pub approvals: ApprovalPolicy,
}

pub async fn run_turn(
    turn: Turn,
    events: UnboundedSender<Event>,
) -> Result<TurnEnd, Box<dyn std::error::Error>> {
    let Turn {
        agent,
        cwd,
        prompt,
        approvals,
    } = turn;
    let process = AcpAgent::from_str(&agent.command)?;
    let update_events = events.clone();
    let approval_events = events;

    let stop_reason = Client
        .v1()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                if let Some(event) = translate(notification.update) {
                    update_events.send(event).ok();
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let (outcome, decision) = resolve(&request, approvals);
                approval_events
                    .send(Event::ApprovalResolved {
                        tool_call_id: request.tool_call.tool_call_id.0.to_string(),
                        title: request.tool_call.fields.title.clone(),
                        decision,
                    })
                    .ok();
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(process, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let mut new_session = NewSessionRequest::new(cwd);
            new_session.meta = agent.session_meta;
            let session = connection.send_request(new_session).block_task().await?;

            if let Some(mode) = agent.session_mode.as_deref()
                && needs_mode_change(&session, mode)?
            {
                connection
                    .send_request(SetSessionModeRequest::new(
                        session.session_id.clone(),
                        SessionModeId::new(mode),
                    ))
                    .block_task()
                    .await?;
            }

            let response = connection
                .send_request(PromptRequest::new(
                    session.session_id,
                    vec![ContentBlock::Text(TextContent::new(prompt))],
                ))
                .block_task()
                .await?;
            Ok(response.stop_reason)
        })
        .await?;

    Ok(turn_end(stop_reason))
}

fn needs_mode_change(session: &NewSessionResponse, mode: &str) -> Result<bool, Error> {
    let offered = session.modes.as_ref().is_some_and(|modes| {
        modes
            .available_modes
            .iter()
            .any(|available| available.id.0.as_ref() == mode)
    });
    if !offered {
        return Err(Error::internal_error().data(Value::String(format!(
            "the agent does not offer the session mode `{mode}`"
        ))));
    }
    Ok(session
        .modes
        .as_ref()
        .is_some_and(|modes| modes.current_mode_id.0.as_ref() != mode))
}

fn resolve(
    request: &RequestPermissionRequest,
    policy: ApprovalPolicy,
) -> (RequestPermissionOutcome, ApprovalDecision) {
    let select = |kind: PermissionOptionKind| {
        request
            .options
            .iter()
            .find(|option| option.kind == kind)
            .map(|option| {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    option.option_id.clone(),
                ))
            })
    };
    if policy == ApprovalPolicy::AllowOnce
        && let Some(outcome) = select(PermissionOptionKind::AllowOnce)
    {
        return (outcome, ApprovalDecision::Allowed);
    }
    (
        select(PermissionOptionKind::RejectOnce).unwrap_or(RequestPermissionOutcome::Cancelled),
        ApprovalDecision::Declined,
    )
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
        SessionUpdate::ToolCallUpdate(update) => Some(Event::ToolCallUpdated {
            id: update.tool_call_id.0.to_string(),
            title: update.fields.title,
            status: update.fields.status.and_then(tool_status),
        }),
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
