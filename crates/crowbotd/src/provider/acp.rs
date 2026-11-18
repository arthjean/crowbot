use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AuthCapabilities, AuthMethod, AuthState, AuthStatusRequest, AuthenticateRequest,
    CancelNotification, ClientCapabilities, ContentBlock, Implementation, InitializeRequest,
    NewSessionRequest, PermissionOptionKind, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest,
    SelectedPermissionOutcome, SessionConfigId, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelectOption,
    SessionConfigSelectOptions, SessionId, SessionModeId, SessionModeState, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, SystemPrompt,
    TextContent, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdateFields,
    ToolKind as AgentToolKind,
};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, Error, ErrorCode, UntypedMessage,
};
use crowbot_protocol::{
    APPROVAL_TARGET_BYTES, Event, FailureCause, ModelChoice, ProviderState, TOOL_INPUT_BYTES,
    TOOL_OUTPUT_BYTES, ToolKind, ToolStatus, TurnEnd, cut_text,
};
use serde_json::{Map, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

/// What the daemon knows about a provider before it starts its agent.
pub struct AgentProfile {
    /// The name a person reads.
    pub name: &'static str,
    /// The arguments and the environment the agent gets after the ones of
    /// its program.
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub session_meta: Option<Map<String, Value>>,
    /// The session mode in which the agent asks before each gated tool call.
    /// A session that cannot be put in it does not start.
    pub session_mode: Option<&'static str>,
    /// The mode an agent that reports no session mode starts in, from its
    /// arguments. A session that reports moving out of it is stopped.
    pub started_mode: Option<&'static str>,
    /// The sign-in method the agent wants named before it opens a session.
    pub session_auth: Option<&'static str>,
    /// The sign-in methods that take a key: the daemon never offers them.
    pub hidden_sign_ins: &'static [&'static str],
    /// The request that names the page where a person finishes a sign-in
    /// the agent runs. Such an agent opens no browser itself.
    pub sign_in_page: Option<&'static str>,
    /// The folders the agent needs, which the daemon makes before each start.
    pub folders: Vec<PathBuf>,
    /// The files the daemon writes before each start, with their content.
    pub files: Vec<(PathBuf, String)>,
    /// The files and folders the daemon removes before each start.
    pub removed: Vec<PathBuf>,
}

impl AgentProfile {
    pub fn generic() -> Self {
        Self {
            name: "Custom agent",
            args: Vec::new(),
            env: BTreeMap::new(),
            session_meta: None,
            session_mode: None,
            started_mode: None,
            session_auth: None,
            hidden_sign_ins: &[],
            sign_in_page: None,
            folders: Vec::new(),
            files: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// Writes what the agent reads at its start.
    pub fn prepare(&self) -> std::io::Result<()> {
        for folder in &self.folders {
            crate::private::create_folder(folder)?;
        }
        for (path, content) in &self.files {
            if let Some(folder) = path.parent() {
                crate::private::create_folder(folder)?;
            }
            crate::private::write(path, content)?;
        }
        for path in &self.removed {
            let removed = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            match removed {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }
}

/// The command that starts an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl Launch {
    pub fn of(program: PathBuf) -> Self {
        Self {
            program,
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    /// Reads a command line, or the JSON form with `command`, `args`, and `env`.
    pub fn parse(command: &str) -> Result<Self, String> {
        let config = AcpAgent::from_str(command)
            .map_err(|error| error.to_string())?
            .into_config();
        Ok(Self {
            program: config.command().to_owned(),
            args: config.arguments().to_vec(),
            env: config.environment().clone(),
        })
    }

    #[must_use]
    pub fn with(mut self, profile: &AgentProfile) -> Self {
        self.args.extend(profile.args.iter().cloned());
        self.env.extend(profile.env.clone());
        self
    }

    /// The agent ends with the daemon: on Unix it starts under the daemon's
    /// guard (`lifetime.rs`).
    fn agent(&self) -> AcpAgent {
        let (program, args) = crate::lifetime::guarded(&self.program, &self.args);
        AcpAgent::new(
            AcpAgentConfig::new(program)
                .args(args)
                .envs(self.env.clone()),
        )
    }

    /// Whether the system would find the program: a path as it is written, a
    /// bare name in the folders of `PATH` and next to the daemon.
    pub fn program_exists(&self) -> bool {
        let program = self.program.as_path();
        let named = |folder: &Path| {
            let path = folder.join(program);
            path.is_file()
                || (cfg!(windows)
                    && path.extension().is_none()
                    && path.with_extension("exe").is_file())
        };
        if program.components().count() > 1 {
            return named(Path::new(""));
        }
        let path = self
            .env
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
            .map(|(_, value)| value.into())
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let beside = std::env::current_exe()
            .ok()
            .and_then(|daemon| daemon.parent().map(Path::to_owned));
        std::env::split_paths(&path)
            .chain(beside)
            .any(|folder| named(&folder))
    }
}

fn client_info() -> Implementation {
    Implementation::new("crowbot", env!("CARGO_PKG_VERSION"))
}

/// How long an agent gets to start and tell its sign-in state.
const PROBE_PATIENCE: Duration = Duration::from_secs(60);
/// How long an agent that pushes its sign-in state gets to push it.
const STATUS_PATIENCE: Duration = Duration::from_secs(8);
/// How long an agent gets to accept the sign-in it already holds.
const SESSION_AUTH_PATIENCE: Duration = Duration::from_secs(30);
const PROBE_DETAIL_BYTES: usize = 300;

/// The longest failure text a turn keeps from an agent's error.
const FAILURE_BYTES: usize = 4 << 10;
const STATUS_UPDATES: [&str; 2] = ["_auth/status_update", "auth/status_update"];

/// The agent's own sign-in, as a command to run: the agent command followed by
/// the arguments of a sign-in method of the `terminal` kind.
#[derive(Clone)]
pub struct SignInCommand {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

#[derive(Clone)]
pub enum SignInStart {
    /// The daemon runs the command and reads its exit status.
    Command(SignInCommand),
    /// The agent runs the sign-in when it is asked to authenticate.
    Agent,
}

#[derive(Clone)]
pub struct SignInOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub start: SignInStart,
}

/// What an agent tells about itself with no session.
pub struct Probe {
    pub state: ProviderState,
    /// The label of the signed-in account, such as the name of a plan. The
    /// rest of what the agent tells about the account is dropped here.
    pub account: Option<String>,
    pub detail: Option<String>,
    pub sign_ins: Vec<SignInOption>,
}

impl Probe {
    pub fn without_answer(state: ProviderState, detail: impl Into<String>) -> Self {
        Self {
            state,
            account: None,
            detail: Some(short(detail.into())),
            sign_ins: Vec::new(),
        }
    }
}

fn short(mut detail: String) -> String {
    cut_text(&mut detail, PROBE_DETAIL_BYTES);
    detail
}

type Status = (ProviderState, Option<String>, Option<String>);

/// What an agent pushes about its sign-in, with no request: the state, and the
/// label of the account.
fn pushed_status(message: &UntypedMessage) -> Option<Status> {
    if !STATUS_UPDATES.contains(&message.method.as_str()) {
        return None;
    }
    let status = message.params.get("authStatus")?;
    let label = status
        .get("label")
        .and_then(Value::as_str)
        .map(|label| short(label.to_owned()));
    match status.get("kind")?.as_str()? {
        "none" => Some((ProviderState::SignedOut, None, None)),
        _ => Some((ProviderState::Ready, label, None)),
    }
}

fn sign_in_options(
    profile: &AgentProfile,
    launch: &Launch,
    methods: Vec<AuthMethod>,
) -> Vec<SignInOption> {
    methods
        .into_iter()
        .filter_map(|method| match method {
            AuthMethod::Terminal(terminal) => Some(SignInOption {
                id: terminal.id.0.to_string(),
                name: terminal.name,
                description: terminal.description,
                start: SignInStart::Command(SignInCommand {
                    program: launch.program.clone(),
                    arguments: launch.args.iter().cloned().chain(terminal.args).collect(),
                    // What the profile sets, such as the home the daemon
                    // owns, wins over what the agent asks for its sign-in.
                    environment: terminal.env.into_iter().chain(launch.env.clone()).collect(),
                }),
            }),
            AuthMethod::Agent(agent) => Some(SignInOption {
                id: agent.id.0.to_string(),
                name: agent.name,
                description: agent.description,
                start: SignInStart::Agent,
            }),
            _ => None,
        })
        .filter(|option| !profile.hidden_sign_ins.contains(&option.id.as_str()))
        .collect()
}

async fn asked_status(connection: &ConnectionTo<Agent>) -> Status {
    let status = connection
        .send_request(AuthStatusRequest::new())
        .block_task()
        .await;
    match status {
        Ok(status) => {
            let account = status
                .accounts
                .iter()
                .find(|account| account.active == Some(true))
                .or(status.accounts.first())
                .map(|account| account.label.clone());
            match status.state {
                AuthState::Authenticated | AuthState::NotRequired => {
                    (ProviderState::Ready, account, None)
                }
                AuthState::Unauthenticated => (ProviderState::SignedOut, None, None),
                _ => (
                    ProviderState::Unknown,
                    None,
                    Some("The agent could not read its own sign-in.".to_owned()),
                ),
            }
        }
        Err(error) => (
            ProviderState::Unknown,
            None,
            Some(short(failure_message(&error))),
        ),
    }
}

/// Starts the agent for one short connection and reads whether it can run a
/// turn: its program exists, it answers, and it reports a sign-in. `signed_in`
/// tells that a sign-in started from the daemon ended well: an agent that
/// reports nothing is then asked to accept the sign-in it holds.
pub async fn probe(profile: &AgentProfile, launch: &Launch, signed_in: bool) -> Probe {
    if !launch.program_exists() {
        return Probe::without_answer(
            ProviderState::RuntimeMissing,
            format!(
                "The program `{}` was not found on this computer.",
                launch.program.display()
            ),
        );
    }
    let (pushed, mut pushes) = tokio::sync::mpsc::unbounded_channel();
    let session_auth = profile.session_auth;
    let asked = Client
        .v1()
        .on_receive_notification(
            async move |message: UntypedMessage, _connection| {
                if let Some(status) = pushed_status(&message) {
                    pushed.send(status).ok();
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            launch.agent(),
            |connection: ConnectionTo<Agent>| async move {
                let capabilities =
                    ClientCapabilities::new().auth(AuthCapabilities::new().terminal(true));
                let initialized = connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(capabilities)
                            .client_info(client_info()),
                    )
                    .block_task()
                    .await?;
                let agent = &initialized.agent_capabilities;
                let pushes_status = agent
                    .meta
                    .as_ref()
                    .is_some_and(|meta| meta.contains_key("authStatus"));
                let status = if agent.auth.status.is_some() {
                    Some(asked_status(&connection).await)
                } else if pushes_status {
                    tokio::time::timeout(STATUS_PATIENCE, pushes.recv())
                        .await
                        .ok()
                        .flatten()
                } else if let Some(method) = session_auth {
                    let accepted = signed_in
                        && matches!(
                            tokio::time::timeout(
                                SESSION_AUTH_PATIENCE,
                                connection
                                    .send_request(AuthenticateRequest::new(method))
                                    .block_task(),
                            )
                            .await,
                            Ok(Ok(_))
                        );
                    let state = if accepted {
                        ProviderState::Ready
                    } else {
                        ProviderState::SignedOut
                    };
                    Some((state, None, None))
                } else {
                    None
                };
                Ok((initialized.auth_methods, status))
            },
        );
    let (methods, status) = match tokio::time::timeout(PROBE_PATIENCE, asked).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(error)) => {
            return Probe::without_answer(ProviderState::RuntimeMissing, failure_message(&error));
        }
        Err(_) => {
            return Probe::without_answer(
                ProviderState::Unknown,
                "The agent did not answer in time.",
            );
        }
    };
    let (state, account, detail) = status.unwrap_or((
        ProviderState::Unknown,
        None,
        Some("The agent does not tell whether it is signed in.".to_owned()),
    ));
    Probe {
        state,
        account,
        detail,
        sign_ins: sign_in_options(profile, launch, methods),
    }
}

/// How often, and how many times, an agent is asked for its sign-in page.
const SIGN_IN_PAGE_INTERVAL: Duration = Duration::from_millis(300);
const SIGN_IN_PAGE_TRIES: u32 = 40;

/// Asks the agent to run one of its own sign-ins, and waits until it ends.
/// When the agent names the page where the person signs in, through the
/// request `page_request`, `on_page` gets its address. Dropping the call
/// closes the agent, and the sign-in with it.
pub async fn authenticate(
    launch: &Launch,
    method: &str,
    page_request: Option<&'static str>,
    on_page: impl Fn(String) + Send + 'static,
) -> Result<(), String> {
    let method = method.to_owned();
    Client
        .v1()
        .connect_with(
            launch.agent(),
            |connection: ConnectionTo<Agent>| async move {
                let capabilities =
                    ClientCapabilities::new().auth(AuthCapabilities::new().terminal(true));
                connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(capabilities)
                            .client_info(client_info()),
                    )
                    .block_task()
                    .await?;
                let signing_in = connection
                    .send_request(AuthenticateRequest::new(method))
                    .block_task();
                let naming = async {
                    if let Some(request) = page_request {
                        for _ in 0..SIGN_IN_PAGE_TRIES {
                            tokio::time::sleep(SIGN_IN_PAGE_INTERVAL).await;
                            let asked = UntypedMessage::new(request, Map::new())?;
                            let answer = connection.send_request(asked).block_task().await;
                            if let Some(page) = answer.ok().as_ref().and_then(sign_in_page) {
                                on_page(page);
                                break;
                            }
                        }
                    }
                    std::future::pending::<Result<(), Error>>().await
                };
                tokio::select! {
                    signed_in = signing_in => signed_in.map(|_| ()),
                    failed = naming => failed,
                }
            },
        )
        .await
        .map_err(|error| short(failure_message(&error)))
}

/// The address of a sign-in page, as an agent names it. Only a secure web
/// address is kept: a client opens it in the browser.
fn sign_in_page(answer: &Value) -> Option<String> {
    let page = answer.get("auth_url")?.as_str()?;
    page.starts_with("https://").then(|| page.to_owned())
}

/// The models and the reasoning levels a session offers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    pub models: Vec<ModelChoice>,
    pub model: Option<String>,
    pub efforts: Vec<ModelChoice>,
    pub effort: Option<String>,
    /// The session is new: what it stands on is what the provider chose.
    pub new_session: bool,
}

pub struct SessionConfig {
    pub agent: AgentProfile,
    pub launch: Launch,
    pub cwd: PathBuf,
    pub resume: Option<String>,
    /// The bot's instructions. They reach the agent as the system prompt of
    /// the session, or at the start of its first prompt when the agent takes
    /// no system prompt.
    pub instructions: String,
    /// The model the session runs on. `None` keeps the agent's default.
    pub model: Option<String>,
    /// The reasoning level. `None` keeps the agent's default.
    pub effort: Option<String>,
    /// Whether a sign-in started from the daemon ended well.
    pub signed_in: bool,
}

pub enum AgentEvent {
    Started {
        provider_session_id: String,
    },
    Event(Event),
    Offer(Offer),
    /// The bot's model is not offered: the session runs on the agent's default.
    ModelUnavailable {
        model: String,
    },
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
    /// A message, after what the session must read first: what was said in
    /// the conversation with another provider.
    Prompt {
        text: String,
        handoff: Option<String>,
    },
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

impl SessionFailure {
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
        }
    }
}

impl From<Error> for SessionFailure {
    fn from(error: Error) -> Self {
        Self {
            cause: failure_cause(&error),
            message: failure_message(&error),
        }
    }
}

fn choice(option: &SessionConfigSelectOption) -> ModelChoice {
    ModelChoice {
        id: option.value.0.to_string(),
        name: option.name.clone(),
        description: option.description.clone(),
        ..ModelChoice::default()
    }
}

fn choices(options: &SessionConfigSelectOptions) -> Vec<ModelChoice> {
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => options.iter().map(choice).collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter().map(choice))
            .collect(),
        _ => Vec::new(),
    }
}

/// The option of a category, with its choices and the one it stands on.
fn select_of(
    options: &[SessionConfigOption],
    category: &SessionConfigOptionCategory,
) -> Option<(SessionConfigId, Vec<ModelChoice>, String)> {
    let option = options
        .iter()
        .find(|option| option.category.as_ref() == Some(category))?;
    match &option.kind {
        SessionConfigKind::Select(select) => Some((
            option.id.clone(),
            choices(&select.options),
            select.current_value.0.to_string(),
        )),
        _ => None,
    }
}

fn offer_of(options: &[SessionConfigOption], new_session: bool) -> Offer {
    let (models, model) = select_of(options, &SessionConfigOptionCategory::Model)
        .map_or((Vec::new(), None), |(_, choices, current)| {
            (choices, Some(current))
        });
    let (efforts, effort) = select_of(options, &SessionConfigOptionCategory::ThoughtLevel)
        .map_or((Vec::new(), None), |(_, choices, current)| {
            (choices, Some(current))
        });
    Offer {
        models,
        model,
        efforts,
        effort,
        new_session,
    }
}

/// The option to set so that the session stands on `wanted`, when it does
/// not yet. `Err` means the agent does not offer that choice.
fn choice_to_set(
    options: &[SessionConfigOption],
    category: &SessionConfigOptionCategory,
    wanted: &str,
) -> Result<Option<SessionConfigId>, ()> {
    let (id, choices, current) = select_of(options, category).ok_or(())?;
    if !choices.iter().any(|choice| choice.id == wanted) {
        return Err(());
    }
    Ok((current != wanted).then_some(id))
}

/// Names the sign-in method to an agent that wants it before each session.
/// Without a sign-in started from the daemon, the agent is not asked: it
/// would start a sign-in of its own in the middle of a turn.
async fn accept_sign_in(
    connection: &ConnectionTo<Agent>,
    profile: &AgentProfile,
    signed_in: bool,
) -> Result<(), Error> {
    let Some(method) = profile.session_auth else {
        return Ok(());
    };
    if !signed_in {
        return Err(Error::auth_required());
    }
    let accepted = tokio::time::timeout(
        SESSION_AUTH_PATIENCE,
        connection
            .send_request(AuthenticateRequest::new(method))
            .block_task(),
    )
    .await;
    match accepted {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(Error::auth_required()),
    }
}

/// Reads the models and the reasoning levels an agent offers: one session is
/// opened in `cwd`, with no prompt, and closed. The levels of a model are
/// often its own, so the session is moved to each model in turn and the
/// levels it then offers are kept on that model.
pub async fn read_offer(
    profile: AgentProfile,
    launch: &Launch,
    cwd: PathBuf,
    signed_in: bool,
) -> Result<Offer, SessionFailure> {
    Client
        .v1()
        .connect_with(
            launch.agent(),
            |connection: ConnectionTo<Agent>| async move {
                connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1).client_info(client_info()),
                    )
                    .block_task()
                    .await?;
                accept_sign_in(&connection, &profile, signed_in).await?;
                let mut request = NewSessionRequest::new(cwd);
                request.meta = profile.session_meta;
                let created = connection.send_request(request).block_task().await?;
                let options = created.config_options.unwrap_or_default();
                let mut offer = offer_of(&options, true);
                let Some((model_option, _, current)) =
                    select_of(&options, &SessionConfigOptionCategory::Model)
                else {
                    return Ok(offer);
                };
                let mut levels = vec![(
                    current,
                    select_of(&options, &SessionConfigOptionCategory::ThoughtLevel),
                )];
                let others: Vec<String> = offer
                    .models
                    .iter()
                    .map(|model| model.id.clone())
                    .filter(|id| *id != levels[0].0)
                    .collect();
                for id in others {
                    // A model the agent will not move to keeps the levels of
                    // the provider.
                    let set = connection
                        .send_request(SetSessionConfigOptionRequest::new(
                            created.session_id.clone(),
                            model_option.clone(),
                            SessionConfigOptionValue::value_id(id.clone()),
                        ))
                        .block_task()
                        .await;
                    if let Ok(set) = set {
                        let found = select_of(
                            &set.config_options,
                            &SessionConfigOptionCategory::ThoughtLevel,
                        );
                        levels.push((id, found));
                    }
                }
                for (id, found) in levels {
                    if let (Some(model), Some((_, efforts, current))) =
                        (offer.models.iter_mut().find(|model| model.id == id), found)
                    {
                        model.efforts = efforts;
                        model.default_effort = Some(current);
                    }
                }
                Ok(offer)
            },
        )
        .await
        .map_err(SessionFailure::from)
}

pub async fn run_session(
    config: SessionConfig,
    first: Vec<SessionCommand>,
    mut commands: UnboundedReceiver<SessionCommand>,
    events: UnboundedSender<AgentEvent>,
) -> Result<(), SessionFailure> {
    let SessionConfig {
        agent,
        launch,
        cwd,
        resume,
        instructions,
        model,
        effort,
        signed_in,
    } = config;
    let update_events = events.clone();
    let approval_events = events.clone();
    let mut first = first.into_iter();
    // The modes the agent reports it moved to on its own, out of the one in
    // which it asks first.
    let guarded_mode = agent.session_mode.or(agent.started_mode);
    let (left_mode, mut mode_left) = tokio::sync::mpsc::unbounded_channel::<String>();

    Client
        .v1()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                if let SessionUpdate::CurrentModeUpdate(update) = &notification.update
                    && guarded_mode.is_some_and(|mode| update.current_mode_id.0.as_ref() != mode)
                {
                    left_mode.send(update.current_mode_id.0.to_string()).ok();
                }
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
        .connect_with(launch.agent(), |connection: ConnectionTo<Agent>| async move {
            let initialized = connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1).client_info(client_info()),
                )
                .block_task()
                .await?;
            accept_sign_in(&connection, &agent, signed_in).await?;

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

            let new_session = resume.is_none();
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

            if let Some(mode) = agent.session_mode
                && needs_mode_change(agent.name, modes.as_ref(), mode)?
            {
                connection
                    .send_request(SetSessionModeRequest::new(
                        session_id.clone(),
                        SessionModeId::new(mode),
                    ))
                    .block_task()
                    .await
                    .map_err(|error| missing_mode(agent.name, mode, &error.to_string()))?;
            }
            // What the agent reported before it stood in the wanted mode is
            // past: no prompt has been sent yet.
            while mode_left.try_recv().is_ok() {}

            let mut config_options = config_options.unwrap_or_default();
            events
                .send(AgentEvent::Offer(offer_of(&config_options, new_session)))
                .ok();
            let wanted = [
                (model, SessionConfigOptionCategory::Model, true),
                (effort, SessionConfigOptionCategory::ThoughtLevel, false),
            ];
            for (value, category, is_model) in wanted {
                let Some(value) = value else {
                    continue;
                };
                match choice_to_set(&config_options, &category, &value) {
                    Ok(None) => {}
                    Ok(Some(option)) => {
                        let set = connection
                            .send_request(SetSessionConfigOptionRequest::new(
                                session_id.clone(),
                                option,
                                SessionConfigOptionValue::value_id(value.clone()),
                            ))
                            .block_task()
                            .await
                            .map_err(|error| {
                                failure(&format!("the agent refused the choice `{value}`: {error}"))
                            })?;
                        config_options = set.config_options;
                    }
                    Err(()) if is_model => {
                        events
                            .send(AgentEvent::ModelUnavailable { model: value })
                            .ok();
                    }
                    Err(()) => {}
                }
            }

            events
                .send(AgentEvent::Started {
                    provider_session_id: session_id.0.to_string(),
                })
                .ok();

            loop {
                let command = match first.next() {
                    Some(command) => command,
                    None => match commands.recv().await {
                        Some(command) => command,
                        None => break,
                    },
                };
                let SessionCommand::Prompt { text, handoff } = command else {
                    continue;
                };
                if let (Some(mode), Ok(left)) = (guarded_mode, mode_left.try_recv()) {
                    return Err(left_mode_error(agent.name, mode, &left));
                }
                let content = preface
                    .take()
                    .into_iter()
                    .chain(handoff)
                    .chain([text])
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
                        Some(left) = mode_left.recv() => {
                            // The agent would now run tools without asking:
                            // the turn stops, and the session closes with it.
                            connection
                                .send_notification(CancelNotification::new(session_id.clone()))
                                .ok();
                            let mode = guarded_mode.unwrap_or_default();
                            return Err(left_mode_error(agent.name, mode, &left));
                        }
                        command = commands.recv() => match command {
                            Some(SessionCommand::Cancel { approvals }) => {
                                connection
                                    .send_notification(CancelNotification::new(session_id.clone()))?;
                                for approval in approvals {
                                    approval.send(ApprovalAnswer::Cancel).ok();
                                }
                            }
                            // The conversation sends one prompt at a time.
                            Some(SessionCommand::Prompt { .. }) => {}
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

/// The text of an agent's error that a person reads and the database keeps:
/// its message and the detail it gave as text. The rest of what an agent
/// sends with an error (a request, its headers, a stack trace) is left out.
fn failure_message(error: &Error) -> String {
    let detail = match &error.data {
        Some(Value::String(text)) => Some(text.as_str()),
        Some(Value::Object(data)) => data.get("message").and_then(Value::as_str),
        _ => None,
    }
    .map(str::trim)
    .filter(|detail| !detail.is_empty());
    let message = error.message.trim();
    let code = error.code.to_string();
    // A message that only names its code says nothing a detail does not.
    let generic = message.is_empty() || message == code;
    let mut text = match detail {
        Some(detail) if generic => detail.to_owned(),
        Some(detail) if !message.contains(detail) => format!("{message}: {detail}"),
        _ if message.is_empty() => code,
        _ => message.to_owned(),
    };
    cut_text(&mut text, FAILURE_BYTES);
    text
}

fn failure(message: &str) -> Error {
    Error::internal_error().data(Value::String(message.to_owned()))
}

/// The error of a session that cannot be put in the mode where the agent asks
/// before each gated tool call.
fn missing_mode(provider: &str, mode: &str, reason: &str) -> Error {
    failure(&format!(
        "{provider} cannot run in the session mode `{mode}` ({reason}), so Crow Bot would not receive its approval requests. No tool was run."
    ))
}

/// The error of a session whose agent left, on its own, the mode in which it
/// asks before each gated tool call.
fn left_mode_error(provider: &str, mode: &str, left: &str) -> Error {
    failure(&format!(
        "{provider} left the session mode `{mode}` for `{left}`, so Crow Bot would not receive its approval requests. The turn was stopped."
    ))
}

fn needs_mode_change(
    provider: &str,
    modes: Option<&SessionModeState>,
    mode: &str,
) -> Result<bool, Error> {
    let offered = modes.is_some_and(|modes| {
        modes
            .available_modes
            .iter()
            .any(|available| available.id.0.as_ref() == mode)
    });
    if !offered {
        return Err(missing_mode(provider, mode, "it does not offer that mode"));
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
        AuthMethodAgent, AuthMethodTerminal, Content, ContentChunk, Diff, PermissionOption,
        SessionMode, Terminal, ToolCall, ToolCallUpdate,
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
        assert!(!needs_mode_change("Agent", Some(&state), "default").unwrap());
    }

    #[test]
    fn a_session_in_another_mode_needs_a_change() {
        let state = modes("bypassPermissions", &["default", "bypassPermissions"]);
        assert!(needs_mode_change("Agent", Some(&state), "default").unwrap());
    }

    #[test]
    fn an_agent_that_does_not_offer_the_mode_is_an_error() {
        let state = modes("plan", &["plan"]);
        assert!(needs_mode_change("Agent", Some(&state), "default").is_err());
        assert!(needs_mode_change("Agent", None, "default").is_err());
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
    fn a_missing_mode_names_the_provider_and_the_mode() {
        let state = modes("plan", &["plan"]);
        let error = needs_mode_change("Codex", Some(&state), "read-only").unwrap_err();
        let message = SessionFailure::from(error).message;
        assert!(message.contains("Codex"), "{message}");
        assert!(message.contains("`read-only`"), "{message}");
        assert!(message.contains("No tool was run"), "{message}");
    }

    const MODEL: SessionConfigOptionCategory = SessionConfigOptionCategory::Model;
    const EFFORT: SessionConfigOptionCategory = SessionConfigOptionCategory::ThoughtLevel;

    #[test]
    fn the_model_is_set_through_the_option_of_the_model_category() {
        let options = model_options("fast");
        assert_eq!(
            choice_to_set(&options, &MODEL, "smart"),
            Ok(Some(SessionConfigId::new("model")))
        );
    }

    #[test]
    fn a_session_already_on_the_model_needs_no_change() {
        let options = model_options("smart");
        assert_eq!(choice_to_set(&options, &MODEL, "smart"), Ok(None));
    }

    #[test]
    fn a_model_the_agent_does_not_offer_is_told_apart_from_one_to_set() {
        let no_model = vec![SessionConfigOption::select(
            "mode",
            "Mode",
            "default",
            no_choice(),
        )];
        assert_eq!(choice_to_set(&no_model, &MODEL, "smart"), Err(()));
        assert_eq!(choice_to_set(&[], &MODEL, "smart"), Err(()));
        assert_eq!(
            choice_to_set(&model_options("fast"), &MODEL, "bogus"),
            Err(())
        );
        assert_eq!(
            choice_to_set(&model_options("fast"), &EFFORT, "high"),
            Err(())
        );
    }

    #[test]
    fn an_offer_lists_the_models_and_the_reasoning_levels_with_what_the_session_stands_on() {
        let mut options = model_options("fast");
        options.push(
            SessionConfigOption::select(
                "reasoning_effort",
                "Reasoning effort",
                "high",
                vec![
                    SessionConfigSelectOption::new("low", "Low"),
                    SessionConfigSelectOption::new("high", "High").description("Deeper".to_owned()),
                ],
            )
            .category(EFFORT),
        );
        let offer = offer_of(&options, true);
        let ids = |choices: &[ModelChoice]| {
            choices
                .iter()
                .map(|choice| choice.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&offer.models), ["fast", "smart"]);
        assert_eq!(offer.models[1].name, "Smart");
        assert_eq!(offer.model.as_deref(), Some("fast"));
        assert_eq!(ids(&offer.efforts), ["low", "high"]);
        assert_eq!(offer.efforts[1].description.as_deref(), Some("Deeper"));
        assert_eq!(offer.effort.as_deref(), Some("high"));
        assert!(offer.new_session);
        assert_eq!(offer_of(&[], false), Offer::default());
    }

    #[test]
    fn an_agent_pushes_its_sign_in_as_a_state_and_a_label_only() {
        let pushed = |method: &str, params: Value| {
            pushed_status(&UntypedMessage::new(method, params).unwrap())
        };
        assert_eq!(
            pushed(
                "_auth/status_update",
                json!({ "authStatus": { "kind": "account", "label": "ChatGPT Plus",
                    "account": { "email": "someone@example.com", "plan": "plus" } } })
            ),
            Some((ProviderState::Ready, Some("ChatGPT Plus".to_owned()), None))
        );
        assert_eq!(
            pushed(
                "auth/status_update",
                json!({ "authStatus": { "kind": "none", "label": "Not logged in" } })
            ),
            Some((ProviderState::SignedOut, None, None))
        );
        assert_eq!(pushed("_auth/status_update", json!({})), None);
        assert_eq!(
            pushed(
                "session/update",
                json!({ "authStatus": { "kind": "none" } })
            ),
            None
        );
    }

    #[test]
    fn a_launch_reads_a_command_line_and_adds_what_the_profile_gives() {
        let launch = Launch::parse(r#"{"command":"agent","args":["stdio"]}"#).unwrap();
        assert_eq!(launch.program, Path::new("agent"));
        assert_eq!(launch.args, ["stdio"]);
        let mut profile = AgentProfile::generic();
        profile.args = vec!["--ask".to_owned()];
        profile
            .env
            .insert("AGENT_HOME".to_owned(), "/data/home".to_owned());
        let launch = launch.with(&profile);
        assert_eq!(launch.args, ["stdio", "--ask"]);
        assert_eq!(
            launch.env.get("AGENT_HOME").map(String::as_str),
            Some("/data/home")
        );
    }

    #[test]
    fn the_sign_ins_keep_the_order_of_the_agent_and_leave_out_the_hidden_ones() {
        let mut profile = AgentProfile::generic();
        profile.hidden_sign_ins = &["api-key"];
        let launch = Launch::of(PathBuf::from("agent"));
        let methods = vec![
            AuthMethod::Agent(AuthMethodAgent::new("api-key", "API key")),
            AuthMethod::Agent(AuthMethodAgent::new("chat", "Chat plan")),
            AuthMethod::Terminal(
                AuthMethodTerminal::new("console", "Console").args(vec!["login".to_owned()]),
            ),
        ];
        let options = sign_in_options(&profile, &launch, methods);
        let names: Vec<&str> = options.iter().map(|option| option.name.as_str()).collect();
        assert_eq!(names, ["Chat plan", "Console"]);
        assert!(matches!(options[0].start, SignInStart::Agent));
        match &options[1].start {
            SignInStart::Command(command) => assert_eq!(command.arguments, ["login"]),
            SignInStart::Agent => panic!("a terminal method is a command"),
        }
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

    #[test]
    fn a_failure_keeps_the_message_and_the_text_detail_and_nothing_else() {
        // The daemon's own failures read as their text alone.
        assert_eq!(
            failure_message(&failure("Codex cannot start")),
            "Codex cannot start"
        );
        // A detail in text follows a message that says more than its code.
        let detailed = Error::new(-32603, "Request failed").data(json!("model not found"));
        assert_eq!(
            failure_message(&detailed),
            "Request failed: model not found"
        );
        // What else an agent sends with an error is not kept: request bodies,
        // headers, stack traces.
        let leaky = Error::new(-32603, "Internal error").data(json!({
            "message": "upstream returned 500",
            "request": { "headers": { "authorization": "Bearer secret-token" } },
            "stack": "at fetch (/home/user/agent.js:1:1)"
        }));
        assert_eq!(failure_message(&leaky), "upstream returned 500");
        let bare = Error::new(-32603, "Internal error").data(json!({ "errorKind": "x" }));
        assert_eq!(failure_message(&bare), "Internal error");
        assert_eq!(
            failure_message(&Error::new(-32000, "")),
            "Authentication required"
        );
        // A long detail is cut.
        let long = failure(&"a".repeat(64 << 10));
        assert_eq!(failure_message(&long).len(), FAILURE_BYTES);
    }
}
