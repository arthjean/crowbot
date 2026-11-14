use std::path::PathBuf;
use std::str::FromStr;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    ResumeSessionRequest, SelectedPermissionOutcome, SessionId, SessionModeId, SessionModeState,
    SessionNotification, SessionUpdate, SetSessionModeRequest, StopReason, TextContent,
    ToolCallStatus,
};
use agent_client_protocol::{AcpAgent, Agent, Client, Error};
use serde_json::{Map, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

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

pub struct SessionConfig {
    pub agent: AgentProfile,
    pub cwd: PathBuf,
    pub approvals: ApprovalPolicy,
    pub resume: Option<String>,
}

pub async fn run_session(
    config: SessionConfig,
    mut prompts: UnboundedReceiver<String>,
    events: UnboundedSender<Event>,
) -> Result<(), Box<dyn std::error::Error>> {
    let SessionConfig {
        agent,
        cwd,
        approvals,
        resume,
    } = config;
    let process = AcpAgent::from_str(&agent.command)?;
    let update_events = events.clone();
    let approval_events = events.clone();

    Client
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
                .send(Event::SessionStarted {
                    provider_session_id: session_id.0.to_string(),
                })
                .ok();

            while let Some(prompt) = prompts.recv().await {
                events.send(Event::UserText(prompt.clone())).ok();
                let response = connection
                    .send_request(PromptRequest::new(
                        session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(prompt))],
                    ))
                    .block_task()
                    .await?;
                events
                    .send(Event::TurnEnded(turn_end(response.stop_reason)))
                    .ok();
            }
            Ok(())
        })
        .await?;

    Ok(())
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
    fn the_decline_policy_selects_reject_once() {
        let request = request(&[
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
        ]);
        let (outcome, decision) = resolve(&request, ApprovalPolicy::Decline);
        assert_eq!(decision, ApprovalDecision::Declined);
        assert_eq!(selected(&outcome).as_deref(), Some("option-2"));
    }

    #[test]
    fn the_allow_policy_selects_allow_once_and_never_allow_always() {
        let request = request(&[
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
        ]);
        let (outcome, decision) = resolve(&request, ApprovalPolicy::AllowOnce);
        assert_eq!(decision, ApprovalDecision::Allowed);
        assert_eq!(selected(&outcome).as_deref(), Some("option-1"));
    }

    #[test]
    fn the_allow_policy_declines_when_only_allow_always_is_offered() {
        let request = request(&[
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::RejectOnce,
        ]);
        let (outcome, decision) = resolve(&request, ApprovalPolicy::AllowOnce);
        assert_eq!(decision, ApprovalDecision::Declined);
        assert_eq!(selected(&outcome).as_deref(), Some("option-1"));
    }

    #[test]
    fn a_request_without_a_reject_option_is_cancelled() {
        let request = request(&[PermissionOptionKind::AllowAlways]);
        let (outcome, decision) = resolve(&request, ApprovalPolicy::Decline);
        assert_eq!(decision, ApprovalDecision::Declined);
        assert!(matches!(outcome, RequestPermissionOutcome::Cancelled));
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
