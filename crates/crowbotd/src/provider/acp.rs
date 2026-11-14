use std::path::PathBuf;
use std::str::FromStr;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    ResumeSessionRequest, SelectedPermissionOutcome, SessionConfigId, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue, SessionId,
    SessionModeId, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, SystemPrompt, TextContent,
    ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdateFields,
    ToolKind as AgentToolKind,
};
use agent_client_protocol::{AcpAgent, Agent, Client, Error, ErrorCode};
use crowbot_protocol::{
    APPROVAL_TARGET_BYTES, Event, FailureCause, TOOL_INPUT_BYTES, TOOL_OUTPUT_BYTES, ToolKind,
    ToolStatus, TurnEnd, cut_text,
};
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
    /// The bot's instructions. They reach the agent as the system prompt of
    /// the session, or at the start of its first prompt when the agent takes
    /// no system prompt.
    pub instructions: String,
    /// The model the session runs on. `None` keeps the agent's default.
    pub model: Option<String>,
}

pub enum AgentEvent {
    Started {
        provider_session_id: String,
    },
    Event(Event),
    ApprovalNeeded {
        tool_call_id: String,
        title: Option<String>,
        target: Option<String>,
        cut: bool,
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

pub struct SessionFailure {
    pub message: String,
    pub cause: Option<FailureCause>,
}

impl From<Error> for SessionFailure {
    fn from(error: Error) -> Self {
        Self {
            cause: failure_cause(&error),
            message: failure_message(&error),
        }
    }
}

pub async fn run_session(
    config: SessionConfig,
    mut commands: UnboundedReceiver<SessionCommand>,
    events: UnboundedSender<AgentEvent>,
) -> Result<(), SessionFailure> {
    let SessionConfig {
        agent,
        cwd,
        resume,
        instructions,
        model,
    } = config;
    let process = AcpAgent::from_str(&agent.command).map_err(|error| SessionFailure {
        message: error.to_string(),
        cause: None,
    })?;
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
                let (target, cut) = approval_target(&request.tool_call.fields);
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
                        target,
                        cut,
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

            let capabilities = initialized.agent_capabilities.session_capabilities;
            let system_prompt = (capabilities.system_prompt.is_some() && !instructions.is_empty())
                .then(|| {
                    SystemPrompt::new(vec![ContentBlock::Text(TextContent::new(
                        instructions.clone(),
                    ))])
                });
            // An agent that takes no system prompt reads the instructions once,
            // at the start of the first prompt of a new session.
            let mut preface =
                (system_prompt.is_none() && resume.is_none() && !instructions.is_empty())
                    .then_some(instructions);

            let (session_id, modes, config_options) = match resume {
                Some(provider_session_id) => {
                    if capabilities.resume.is_none() {
                        return Err(failure("the agent cannot resume a session"));
                    }
                    let session_id = SessionId::new(provider_session_id);
                    let mut request = ResumeSessionRequest::new(session_id.clone(), cwd);
                    request.meta = agent.session_meta;
                    request.system_prompt = system_prompt;
                    let resumed = connection.send_request(request).block_task().await?;
                    (session_id, resumed.modes, resumed.config_options)
                }
                None => {
                    let mut request = NewSessionRequest::new(cwd);
                    request.meta = agent.session_meta;
                    request.system_prompt = system_prompt;
                    let created = connection.send_request(request).block_task().await?;
                    (created.session_id, created.modes, created.config_options)
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

            if let Some(model) = model.as_deref()
                && let Some(option) = model_option(config_options.as_deref(), model)?
            {
                connection
                    .send_request(SetSessionConfigOptionRequest::new(
                        session_id.clone(),
                        option,
                        SessionConfigOptionValue::value_id(model.to_owned()),
                    ))
                    .block_task()
                    .await
                    .map_err(|error| {
                        failure(&format!("the agent refused the model `{model}`: {error}"))
                    })?;
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
                let content = preface
                    .take()
                    .into_iter()
                    .chain([prompt])
                    .map(|text| ContentBlock::Text(TextContent::new(text)))
                    .collect();
                let mut answer = std::pin::pin!(
                    connection
                        .send_request(PromptRequest::new(session_id.clone(), content))
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
        .map_err(SessionFailure::from)
}

const SIGN_IN_KINDS: [&str; 2] = ["authentication_failed", "oauth_org_not_allowed"];
const USAGE_LIMIT_KINDS: [&str; 3] = ["rate_limit", "billing_error", "account_on_hold"];

fn error_kind(error: &Error) -> Option<&str> {
    error.data.as_ref()?.get("errorKind")?.as_str()
}

fn failure_cause(error: &Error) -> Option<FailureCause> {
    if error.code == ErrorCode::AUTH_REQUIRED {
        return Some(FailureCause::SignInRequired);
    }
    let kind = error_kind(error)?;
    if SIGN_IN_KINDS.contains(&kind) {
        Some(FailureCause::SignInRequired)
    } else if USAGE_LIMIT_KINDS.contains(&kind) {
        Some(FailureCause::UsageLimit)
    } else {
        None
    }
}

fn failure_message(error: &Error) -> String {
    if error_kind(error).is_some() && !error.message.is_empty() {
        error.message.clone()
    } else {
        error.to_string()
    }
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

/// The config option that sets the model, when the session does not run on
/// `model` yet. An agent that offers no model choice cannot run the bot's model.
fn model_option(
    options: Option<&[SessionConfigOption]>,
    model: &str,
) -> Result<Option<SessionConfigId>, Error> {
    let option = options
        .unwrap_or_default()
        .iter()
        .find(|option| option.category == Some(SessionConfigOptionCategory::Model))
        .ok_or_else(|| {
            failure(&format!(
                "the agent offers no model choice, so it cannot run on the model `{model}`"
            ))
        })?;
    let current = match &option.kind {
        SessionConfigKind::Select(select) => Some(select.current_value.0.as_ref()),
        _ => None,
    };
    Ok((current != Some(model)).then(|| option.id.clone()))
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
            status: tool_status(call.status),
            kind: Some(tool_kind(call.kind)),
            input: tool_input(call.raw_input.as_ref(), &call.locations),
            title: call.title,
        }),
        SessionUpdate::ToolCallUpdate(update) => {
            let fields = update.fields;
            let input = tool_input(
                fields.raw_input.as_ref(),
                fields.locations.as_deref().unwrap_or_default(),
            );
            let output = tool_output(
                fields.content.as_deref().unwrap_or_default(),
                fields.raw_output.as_ref(),
            );
            let title = fields.title;
            let status = fields.status.and_then(tool_status);
            let kind = fields.kind.map(tool_kind);
            let changed = title.is_some()
                || status.is_some()
                || kind.is_some()
                || input.is_some()
                || output.is_some();
            changed.then(|| Event::ToolCallUpdated {
                id: update.tool_call_id.0.to_string(),
                title,
                status,
                kind,
                input,
                output,
            })
        }
        _ => None,
    }
}

fn tool_kind(kind: AgentToolKind) -> ToolKind {
    match kind {
        AgentToolKind::Read => ToolKind::Read,
        AgentToolKind::Edit => ToolKind::Edit,
        AgentToolKind::Delete => ToolKind::Delete,
        AgentToolKind::Move => ToolKind::Move,
        AgentToolKind::Search => ToolKind::Search,
        AgentToolKind::Execute => ToolKind::Execute,
        AgentToolKind::Think => ToolKind::Think,
        AgentToolKind::Fetch => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

fn string_field(fields: Option<&Map<String, Value>>, names: &[&str]) -> Option<String> {
    let fields = fields?;
    names
        .iter()
        .filter_map(|name| fields.get(*name)?.as_str())
        .find(|text| !text.is_empty())
        .map(str::to_owned)
}

fn first_location(locations: &[ToolCallLocation]) -> Option<String> {
    locations
        .first()
        .map(|location| location.path.to_string_lossy().into_owned())
        .filter(|path| !path.is_empty())
}

fn tool_input(raw_input: Option<&Value>, locations: &[ToolCallLocation]) -> Option<String> {
    let fields = raw_input.and_then(Value::as_object);
    let mut input = string_field(fields, &["command"])
        .or_else(|| string_field(fields, &["file_path", "path", "abs_path"]))
        .or_else(|| first_location(locations))
        .or_else(|| string_field(fields, &["pattern", "query", "url"]))
        .or_else(|| {
            fields
                .filter(|fields| !fields.is_empty())
                .and_then(|fields| serde_json::to_string(fields).ok())
        })?;
    cut_text(&mut input, TOOL_INPUT_BYTES);
    Some(input)
}

fn content_text(content: &ToolCallContent) -> Option<String> {
    match content {
        ToolCallContent::Content(content) => match &content.content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        },
        ToolCallContent::Diff(diff) => Some(format!("{}\n{}", diff.path.display(), diff.new_text)),
        _ => None,
    }
}

fn tool_output(content: &[ToolCallContent], raw_output: Option<&Value>) -> Option<String> {
    let shown = content
        .iter()
        .filter_map(content_text)
        .collect::<Vec<_>>()
        .join("\n");
    let mut output = if shown.is_empty() {
        match raw_output? {
            Value::Null => return None,
            Value::String(text) => text.clone(),
            other => serde_json::to_string(other).ok()?,
        }
    } else {
        shown
    };
    if output.is_empty() {
        return None;
    }
    cut_text(&mut output, TOOL_OUTPUT_BYTES);
    Some(output)
}

fn approval_target(fields: &ToolCallUpdateFields) -> (Option<String>, bool) {
    let input = fields.raw_input.as_ref().and_then(Value::as_object);
    let target = string_field(input, &["command"])
        .or_else(|| first_location(fields.locations.as_deref().unwrap_or_default()))
        .or_else(|| string_field(input, &["file_path", "path"]));
    match target {
        Some(mut target) => {
            let cut = cut_text(&mut target, APPROVAL_TARGET_BYTES);
            (Some(target), cut)
        }
        None => (None, false),
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
        Content, ContentChunk, Diff, PermissionOption, SessionConfigSelectOption, SessionMode,
        Terminal, ToolCall, ToolCallUpdate,
    };
    use serde_json::json;

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

    fn no_choice() -> Vec<SessionConfigSelectOption> {
        Vec::new()
    }

    fn model_options(current: &str) -> Vec<SessionConfigOption> {
        let models = vec![
            SessionConfigSelectOption::new("fast", "Fast"),
            SessionConfigSelectOption::new("smart", "Smart"),
        ];
        vec![
            SessionConfigOption::select("mode", "Mode", "default", no_choice()),
            SessionConfigOption::select("model", "Model", current.to_owned(), models)
                .category(SessionConfigOptionCategory::Model),
        ]
    }

    #[test]
    fn the_model_is_set_through_the_option_of_the_model_category() {
        let options = model_options("fast");
        let option = model_option(Some(&options), "smart").unwrap();
        assert_eq!(option, Some(SessionConfigId::new("model")));
    }

    #[test]
    fn a_session_already_on_the_model_needs_no_change() {
        let options = model_options("smart");
        assert_eq!(model_option(Some(&options), "smart").unwrap(), None);
    }

    #[test]
    fn an_agent_with_no_model_choice_cannot_run_a_chosen_model() {
        let no_model = vec![SessionConfigOption::select(
            "mode",
            "Mode",
            "default",
            no_choice(),
        )];
        assert!(model_option(Some(&no_model), "smart").is_err());
        assert!(model_option(None, "smart").is_err());
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

    fn text_item(text: &str) -> ToolCallContent {
        ToolCallContent::Content(Content::new(ContentBlock::Text(TextContent::new(text))))
    }

    fn updated(fields: ToolCallUpdateFields) -> Option<Event> {
        translate(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "t1", fields,
        )))
    }

    #[test]
    fn a_tool_call_starts_with_its_kind_and_a_summary_of_its_input() {
        let call = ToolCall::new("t1", "Run tests")
            .kind(AgentToolKind::Execute)
            .status(ToolCallStatus::InProgress)
            .raw_input(json!({ "command": "cargo test", "timeout": 60 }));
        assert_eq!(
            translate(SessionUpdate::ToolCall(call)),
            Some(Event::ToolCallStarted {
                id: "t1".to_owned(),
                title: "Run tests".to_owned(),
                status: Some(ToolStatus::InProgress),
                kind: Some(ToolKind::Execute),
                input: Some("cargo test".to_owned()),
            })
        );
        assert_eq!(
            translate(SessionUpdate::ToolCall(ToolCall::new("t2", "Think"))),
            Some(Event::ToolCallStarted {
                id: "t2".to_owned(),
                title: "Think".to_owned(),
                status: Some(ToolStatus::Pending),
                kind: Some(ToolKind::Other),
                input: None,
            })
        );
    }

    #[test]
    fn every_agent_tool_kind_has_a_kind_and_the_ones_unknown_here_are_other() {
        for (agent, kind) in [
            (AgentToolKind::Read, ToolKind::Read),
            (AgentToolKind::Edit, ToolKind::Edit),
            (AgentToolKind::Delete, ToolKind::Delete),
            (AgentToolKind::Move, ToolKind::Move),
            (AgentToolKind::Search, ToolKind::Search),
            (AgentToolKind::Execute, ToolKind::Execute),
            (AgentToolKind::Think, ToolKind::Think),
            (AgentToolKind::Fetch, ToolKind::Fetch),
            (AgentToolKind::SwitchMode, ToolKind::Other),
            (AgentToolKind::Other, ToolKind::Other),
        ] {
            assert_eq!(tool_kind(agent), kind);
        }
    }

    #[test]
    fn the_input_summary_is_the_command_then_a_path_then_a_search() {
        let locations = [
            ToolCallLocation::new("src/lib.rs"),
            ToolCallLocation::new("src/main.rs"),
        ];
        let summary =
            |input: Value, locations: &[ToolCallLocation]| tool_input(Some(&input), locations);
        assert_eq!(
            summary(
                json!({ "command": "ls", "file_path": "a.txt", "pattern": "x" }),
                &locations
            )
            .as_deref(),
            Some("ls")
        );
        assert_eq!(
            summary(json!({ "file_path": "a.txt", "path": "b.txt" }), &locations).as_deref(),
            Some("a.txt")
        );
        assert_eq!(
            summary(json!({ "path": "b.txt", "abs_path": "/c.txt" }), &[]).as_deref(),
            Some("b.txt")
        );
        assert_eq!(
            summary(
                json!({ "abs_path": "/c.txt", "url": "https://example.com" }),
                &[]
            )
            .as_deref(),
            Some("/c.txt")
        );
        assert_eq!(
            summary(json!({ "pattern": "TODO", "limit": 5 }), &locations).as_deref(),
            Some("src/lib.rs")
        );
        assert_eq!(
            summary(json!({ "pattern": "TODO", "query": "q" }), &[]).as_deref(),
            Some("TODO")
        );
        assert_eq!(
            summary(
                json!({ "query": "crows", "url": "https://example.com" }),
                &[]
            )
            .as_deref(),
            Some("crows")
        );
        assert_eq!(
            summary(json!({ "url": "https://example.com" }), &[]).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            summary(json!({ "command": 7, "limit": 5 }), &[]).as_deref(),
            Some(r#"{"command":7,"limit":5}"#)
        );
        assert_eq!(summary(json!({}), &[]), None);
        assert_eq!(summary(json!("ls"), &[]), None);
        assert_eq!(
            summary(json!(["ls"]), &locations).as_deref(),
            Some("src/lib.rs")
        );
        assert_eq!(tool_input(None, &[]), None);
        assert_eq!(tool_input(None, &locations).as_deref(), Some("src/lib.rs"));
    }

    #[test]
    fn a_long_input_summary_is_cut() {
        let command = "é".repeat(TOOL_INPUT_BYTES);
        let input = tool_input(Some(&json!({ "command": command })), &[]).unwrap();
        assert_eq!(input.len(), TOOL_INPUT_BYTES);
        assert!(command.starts_with(&input));
        let exact = "x".repeat(TOOL_INPUT_BYTES);
        assert_eq!(
            tool_input(Some(&json!({ "command": exact })), &[]),
            Some(exact)
        );
    }

    #[test]
    fn the_output_is_the_text_of_the_content_then_the_raw_output() {
        let content = [
            text_item("line one"),
            ToolCallContent::Terminal(Terminal::new("terminal-1")),
            ToolCallContent::Diff(Diff::new("src/main.rs", "fn main() {}")),
            text_item("line two"),
        ];
        assert_eq!(
            tool_output(&content, Some(&json!("ignored"))).as_deref(),
            Some("line one\nsrc/main.rs\nfn main() {}\nline two")
        );
        let nothing = [ToolCallContent::Terminal(Terminal::new("terminal-1"))];
        assert_eq!(
            tool_output(&nothing, Some(&json!("raw text"))).as_deref(),
            Some("raw text")
        );
        assert_eq!(
            tool_output(&[], Some(&json!({ "code": 1, "stderr": "boom" }))).as_deref(),
            Some(r#"{"code":1,"stderr":"boom"}"#)
        );
        assert_eq!(tool_output(&[], Some(&json!(3))).as_deref(), Some("3"));
        assert_eq!(tool_output(&[], Some(&Value::Null)), None);
        assert_eq!(tool_output(&[], Some(&json!(""))), None);
        assert_eq!(tool_output(&nothing, None), None);
        assert_eq!(tool_output(&[], None), None);
    }

    #[test]
    fn a_long_output_is_cut() {
        let long = "x".repeat(TOOL_OUTPUT_BYTES + 1);
        let cut = tool_output(&[text_item(&long)], None).unwrap();
        assert_eq!(cut.len(), TOOL_OUTPUT_BYTES);
        let raw = tool_output(&[], Some(&json!(long))).unwrap();
        assert_eq!(raw.len(), TOOL_OUTPUT_BYTES);
        let exact = "x".repeat(TOOL_OUTPUT_BYTES);
        assert_eq!(tool_output(&[text_item(&exact)], None), Some(exact));
    }

    #[test]
    fn a_tool_call_update_carries_what_changed_and_nothing_else() {
        let failed = ToolCallUpdateFields::new()
            .status(ToolCallStatus::Failed)
            .content(vec![text_item("error: 1 test failed")]);
        assert_eq!(
            updated(failed),
            Some(Event::ToolCallUpdated {
                id: "t1".to_owned(),
                title: None,
                status: Some(ToolStatus::Failed),
                kind: None,
                input: None,
                output: Some("error: 1 test failed".to_owned()),
            })
        );
        let detailed = ToolCallUpdateFields::new()
            .title("Read notes.txt".to_owned())
            .kind(AgentToolKind::Read)
            .raw_input(json!({ "file_path": "notes.txt" }));
        assert_eq!(
            updated(detailed),
            Some(Event::ToolCallUpdated {
                id: "t1".to_owned(),
                title: Some("Read notes.txt".to_owned()),
                status: None,
                kind: Some(ToolKind::Read),
                input: Some("notes.txt".to_owned()),
                output: None,
            })
        );
        let output_only = ToolCallUpdateFields::new().raw_output(json!("done"));
        assert_eq!(
            updated(output_only),
            Some(Event::ToolCallUpdated {
                id: "t1".to_owned(),
                title: None,
                status: None,
                kind: None,
                input: None,
                output: Some("done".to_owned()),
            })
        );
        let located = ToolCallUpdateFields::new().locations(vec![ToolCallLocation::new("a.rs")]);
        assert_eq!(
            updated(located),
            Some(Event::ToolCallUpdated {
                id: "t1".to_owned(),
                title: None,
                status: None,
                kind: None,
                input: Some("a.rs".to_owned()),
                output: None,
            })
        );
        assert_eq!(updated(ToolCallUpdateFields::new()), None);
        assert_eq!(
            updated(ToolCallUpdateFields::new().content(Vec::<ToolCallContent>::new())),
            None
        );
    }

    #[test]
    fn an_approval_targets_the_command_then_the_location_then_the_path() {
        let location = || vec![ToolCallLocation::new("src/main.rs")];
        let command = ToolCallUpdateFields::new()
            .raw_input(json!({ "command": "rm -rf build", "file_path": "a.txt" }))
            .locations(location());
        assert_eq!(
            approval_target(&command),
            (Some("rm -rf build".to_owned()), false)
        );
        let located = ToolCallUpdateFields::new()
            .raw_input(json!({ "file_path": "a.txt" }))
            .locations(location());
        assert_eq!(
            approval_target(&located),
            (Some("src/main.rs".to_owned()), false)
        );
        let file =
            ToolCallUpdateFields::new().raw_input(json!({ "file_path": "a.txt", "path": "b" }));
        assert_eq!(approval_target(&file), (Some("a.txt".to_owned()), false));
        let path = ToolCallUpdateFields::new().raw_input(json!({ "path": "b.txt" }));
        assert_eq!(approval_target(&path), (Some("b.txt".to_owned()), false));
        let search = ToolCallUpdateFields::new().raw_input(json!({ "pattern": "TODO" }));
        assert_eq!(approval_target(&search), (None, false));
        assert_eq!(approval_target(&ToolCallUpdateFields::new()), (None, false));
    }

    #[test]
    fn an_approval_target_over_the_cap_is_cut_and_says_so() {
        let exact = "x".repeat(APPROVAL_TARGET_BYTES);
        let whole = ToolCallUpdateFields::new().raw_input(json!({ "command": exact }));
        assert_eq!(approval_target(&whole), (Some(exact), false));
        let long = "x".repeat(APPROVAL_TARGET_BYTES + 1);
        let over = ToolCallUpdateFields::new().raw_input(json!({ "command": long }));
        let (target, cut) = approval_target(&over);
        assert!(cut);
        assert_eq!(
            target.map(|target| target.len()),
            Some(APPROVAL_TARGET_BYTES)
        );
    }

    fn kind_error(kind: &str) -> Error {
        Error::internal_error().data(json!({ "errorKind": kind }))
    }

    #[test]
    fn a_failure_is_a_sign_in_or_a_usage_limit_only_when_the_agent_says_so() {
        assert_eq!(
            failure_cause(&Error::auth_required()),
            Some(FailureCause::SignInRequired)
        );
        assert_eq!(
            failure_cause(&Error::auth_required().data(json!({ "errorKind": "rate_limit" }))),
            Some(FailureCause::SignInRequired)
        );
        for kind in ["authentication_failed", "oauth_org_not_allowed"] {
            assert_eq!(
                failure_cause(&kind_error(kind)),
                Some(FailureCause::SignInRequired),
                "{kind}"
            );
        }
        for kind in ["rate_limit", "billing_error", "account_on_hold"] {
            assert_eq!(
                failure_cause(&kind_error(kind)),
                Some(FailureCause::UsageLimit),
                "{kind}"
            );
        }
        for kind in ["worker_shutdown", "server_error", "", "RATE_LIMIT"] {
            assert_eq!(failure_cause(&kind_error(kind)), None, "{kind}");
        }
        assert_eq!(failure_cause(&Error::internal_error()), None);
        assert_eq!(failure_cause(&failure("rate_limit")), None);
        assert_eq!(
            failure_cause(&Error::internal_error().data(json!({ "errorKind": 7 }))),
            None
        );
        assert_eq!(
            failure_cause(&Error::internal_error().data(json!({ "kind": "rate_limit" }))),
            None
        );
    }

    #[test]
    fn a_session_failure_carries_the_message_and_the_cause() {
        let limit = SessionFailure::from(
            Error::new(-32603, "You have reached your usage limit.")
                .data(json!({ "errorKind": "rate_limit", "rateLimit": { "resetsAt": 1 } })),
        );
        assert_eq!(limit.message, "You have reached your usage limit.");
        assert_eq!(limit.cause, Some(FailureCause::UsageLimit));

        let sign_in = SessionFailure::from(Error::auth_required());
        assert_eq!(sign_in.message, "Authentication required");
        assert_eq!(sign_in.cause, Some(FailureCause::SignInRequired));

        let other = SessionFailure::from(failure("the agent cannot resume a session"));
        assert!(other.message.contains("the agent cannot resume a session"));
        assert_eq!(other.cause, None);
    }
}
