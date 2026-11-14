use std::path::PathBuf;
use std::str::FromStr;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, StopReason, TextContent,
    ToolCallStatus,
};
use agent_client_protocol::{AcpAgent, Agent, Client};
use tokio::sync::mpsc::UnboundedSender;

use crate::event::{Event, ToolStatus, TurnEnd};

pub struct Turn {
    pub command: String,
    pub cwd: PathBuf,
    pub prompt: String,
}

pub async fn run_turn(
    turn: Turn,
    events: UnboundedSender<Event>,
) -> Result<TurnEnd, Box<dyn std::error::Error>> {
    let agent = AcpAgent::from_str(&turn.command)?;
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
                approval_events
                    .send(Event::ApprovalDeclined {
                        tool_call_id: request.tool_call.tool_call_id.0.to_string(),
                        title: request.tool_call.fields.title.clone(),
                    })
                    .ok();
                responder.respond(RequestPermissionResponse::new(decline(&request)))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(NewSessionRequest::new(turn.cwd))
                .block_task()
                .await?;
            let response = connection
                .send_request(PromptRequest::new(
                    session.session_id,
                    vec![ContentBlock::Text(TextContent::new(turn.prompt))],
                ))
                .block_task()
                .await?;
            Ok(response.stop_reason)
        })
        .await?;

    Ok(turn_end(stop_reason))
}

fn decline(request: &RequestPermissionRequest) -> RequestPermissionOutcome {
    request
        .options
        .iter()
        .find(|option| option.kind == PermissionOptionKind::RejectOnce)
        .map_or(RequestPermissionOutcome::Cancelled, |option| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        })
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
