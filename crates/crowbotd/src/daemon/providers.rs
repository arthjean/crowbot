//! What the daemon knows about each provider: the program that starts its
//! agent, whether it can run a turn, the sign-ins it offers, and the models
//! it lists. A sign-in is the provider's own: the daemon starts it and reads
//! whether it ended well, and nothing else.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crowbot_protocol::{
    FailureCause, ModelChoice, ModelsState, ProviderModels, ProviderState, ProviderStatus,
    RuntimeInstall, ServerMessage, SignInMethod,
};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;

use super::{CUSTOM_PROVIDER, Daemon, Refusal, State, invalid};
use crate::provider::acp::{
    self, AgentProfile, Launch, Offer, Probe, SessionFailure, SignInCommand, SignInOption,
    SignInStart,
};
use crate::provider::{claude, codex, grok};
use crate::runtime::{Installed, Progress, RuntimeError, RuntimeSpec};
use crate::store::ProviderSetting;

/// A sign-in that nobody finished ends after this long.
const SIGN_IN_PATIENCE: Duration = Duration::from_secs(600);
/// How long a provider gets to open the session that lists its models.
const MODELS_PATIENCE: Duration = Duration::from_secs(90);
/// The least time between two reports of a download.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

const PROVIDERS: [&str; 3] = [claude::PROVIDER, codex::PROVIDER, grok::PROVIDER];

#[derive(Default)]
pub(super) struct ProviderEntry {
    status: ProviderStatus,
    checked: bool,
    /// A check was asked for while another one ran: one more follows it.
    stale: bool,
    sign_ins: Vec<SignInOption>,
    cancel: Option<oneshot::Sender<()>>,
    /// Why the last sign-in or the last download did not finish, shown until
    /// the provider is ready.
    note: Option<String>,
    models: Option<ProviderModels>,
    reading_models: bool,
    progress_sent: Option<Instant>,
}

enum SignInEnd {
    Finished(bool),
    Cancelled,
    TimedOut,
    NotStarted(String),
}

/// Where the program of a provider's agent comes from.
enum Source {
    Ready(Launch),
    /// The runtime the daemon installed, whose files are checked before each
    /// start (`ProviderHub::verified`).
    Installed(Launch),
    /// The daemon installs the runtime, and has not done so yet.
    NotInstalled(RuntimeSpec),
    /// No runtime is pinned for this computer, and the user set no executable.
    Unsupported,
    /// The folder of the runtimes is on a file system mounted `noexec`, so no
    /// runtime can start, and none is downloaded.
    Refused(PathBuf),
}

fn launch_of(installed: Installed) -> Launch {
    Launch {
        program: installed.program,
        args: installed.args,
        env: installed.env,
    }
}

fn unsupported(name: &str) -> String {
    format!(
        "Crow Bot installs no {name} runtime on this computer. Set the path of its executable in the settings."
    )
}

/// Why no runtime can start: its folder is on a file system mounted `noexec`.
fn refused_programs(name: &str, folder: &Path) -> String {
    format!(
        "The folder of the runtimes does not allow running programs (noexec): {}. Start the daemon with --data-dir elsewhere, or set the path of the {name} executable in the settings.",
        folder.display()
    )
}

fn runtime_failure(name: &str, error: &RuntimeError) -> String {
    match error {
        RuntimeError::Unsupported => unsupported(name),
        RuntimeError::Offline { .. } => {
            format!("The {name} runtime is not installed and cannot be downloaded offline.")
        }
        RuntimeError::HashMismatch { .. } => format!(
            "The {name} runtime was not installed: {error}. The file was removed and nothing of it was run."
        ),
        RuntimeError::Download { .. } => {
            format!("The {name} runtime could not be downloaded: {error}.")
        }
        RuntimeError::Unpack { .. } => {
            format!("The {name} runtime could not be installed: {error}.")
        }
    }
}

fn entry<'a>(state: &'a mut State, id: &'static str, name: &str) -> &'a mut ProviderEntry {
    state.providers.entry(id).or_insert_with(|| ProviderEntry {
        status: ProviderStatus {
            id: id.to_owned(),
            name: name.to_owned(),
            ..ProviderStatus::default()
        },
        ..ProviderEntry::default()
    })
}

impl Daemon {
    /// The providers a new bot of this daemon can run on.
    pub(super) fn provider_ids(&self) -> Vec<&'static str> {
        if self.command.is_some() {
            vec![CUSTOM_PROVIDER]
        } else {
            PROVIDERS.to_vec()
        }
    }

    pub(super) fn known_provider(&self, provider: &str) -> Result<&'static str, String> {
        if provider == CUSTOM_PROVIDER {
            return match self.command {
                Some(_) => Ok(CUSTOM_PROVIDER),
                None => Err(
                    "this bot runs a custom agent, and the daemon was started without --agent"
                        .to_owned(),
                ),
            };
        }
        PROVIDERS
            .into_iter()
            .find(|id| *id == provider)
            .ok_or_else(|| format!("unknown provider: {provider}"))
    }

    /// The folder a provider keeps its own files in, when the daemon gives
    /// it one in place of the user's.
    fn provider_home(&self, id: &str) -> PathBuf {
        self.data_dir.join("providers").join(id).join("home")
    }

    pub fn agent_profile(&self, provider: &str) -> Result<AgentProfile, String> {
        Ok(match self.known_provider(provider)? {
            claude::PROVIDER => claude::profile(),
            codex::PROVIDER => codex::profile(&self.provider_home(codex::PROVIDER)),
            grok::PROVIDER => grok::profile(&self.provider_home(grok::PROVIDER)),
            _ => AgentProfile::generic(),
        })
    }

    /// The name a person reads for a provider.
    pub fn provider_name(&self, provider: &str) -> String {
        self.agent_profile(provider)
            .map_or_else(|_| provider.to_owned(), |profile| profile.name.to_owned())
    }

    fn provider_setting(&self, id: &str) -> Result<ProviderSetting, String> {
        self.store
            .with(|store| store.provider_setting(id))
            .map_err(|error| error.to_string())
    }

    /// The program of the provider's agent: the command given to the daemon,
    /// then the executable the user set, then the runtime the daemon installs.
    fn launch_source(
        &self,
        id: &str,
        profile: &AgentProfile,
        setting: &ProviderSetting,
    ) -> Result<Source, String> {
        if id == CUSTOM_PROVIDER {
            let command = self.command.as_deref().unwrap_or_default();
            return Ok(Source::Ready(Launch::parse(command)?.with(profile)));
        }
        if let Some(executable) = &setting.executable {
            return Ok(Source::Ready(
                Launch::of(PathBuf::from(executable)).with(profile),
            ));
        }
        if let Some(folder) = self.runtimes.refusing_programs() {
            return Ok(Source::Refused(folder.to_owned()));
        }
        if let Some(installed) = self.runtimes.installed(id) {
            return Ok(Source::Installed(launch_of(installed).with(profile)));
        }
        Ok(match self.runtimes.spec(id) {
            Some(spec) => Source::NotInstalled(spec),
            None => Source::Unsupported,
        })
    }

    /// What starts a session of the provider. A runtime that is not installed
    /// yet is downloaded first, and every client follows the download.
    pub async fn session_launch(
        self: &Arc<Self>,
        provider: &str,
    ) -> Result<(AgentProfile, Launch, bool), SessionFailure> {
        let id = self
            .known_provider(provider)
            .map_err(SessionFailure::plain)?;
        let profile = self.agent_profile(id).map_err(SessionFailure::plain)?;
        let setting = self.provider_setting(id).map_err(SessionFailure::plain)?;
        let source = self
            .launch_source(id, &profile, &setting)
            .map_err(SessionFailure::plain)?;
        let ready = match source {
            Source::Ready(launch) => Some(launch),
            Source::Installed(launch) => self.verified(id).await.ok().map(|()| launch),
            Source::NotInstalled(_) => None,
            Source::Unsupported => return Err(SessionFailure::plain(unsupported(profile.name))),
            Source::Refused(folder) => {
                return Err(SessionFailure::plain(refused_programs(
                    profile.name,
                    &folder,
                )));
            }
        };
        let launch = match ready {
            Some(launch) => launch,
            // A runtime that failed its check is downloaded again.
            None => {
                let installed = self
                    .install(id, profile.name)
                    .await
                    .map_err(SessionFailure::plain)?;
                launch_of(installed).with(&profile)
            }
        };
        profile.prepare().map_err(|error| {
            SessionFailure::plain(format!(
                "Cannot prepare the folder of {}: {error}",
                profile.name
            ))
        })?;
        #[cfg(target_os = "linux")]
        if id == codex::PROVIDER {
            codex::check_sandbox(&launch)
                .await
                .map_err(SessionFailure::plain)?;
        }
        Ok((profile, launch, setting.signed_in))
    }

    /// Checks the files of the installed runtime of a provider before it
    /// runs. A runtime that fails the check is no longer installed, and a
    /// turn downloads it again.
    async fn verified(self: &Arc<Self>, id: &'static str) -> Result<(), String> {
        let daemon = self.clone();
        let checked = tokio::task::spawn_blocking(move || daemon.runtimes.verify(id))
            .await
            .map_err(|error| error.to_string())?;
        checked.map_err(|detail| {
            let name = self.provider_name(id);
            eprintln!("[crowbotd] the {name} runtime failed its check: {detail}");
            format!(
                "The {name} runtime changed after its install ({detail}), so it did not run. Crow Bot downloads it again when a bot needs it."
            )
        })
    }

    /// Downloads the runtime of a provider, and tells every client how far
    /// the download is.
    async fn install(self: &Arc<Self>, id: &'static str, name: &str) -> Result<Installed, String> {
        let total = self.runtimes.spec(id).map_or(0, |spec| spec.bytes());
        self.report_install(id, name, Some((0, total)), true);
        let progress: Progress = {
            let (daemon, name) = (self.clone(), name.to_owned());
            Arc::new(move |received, total| {
                daemon.report_install(id, &name, Some((received, total)), received >= total);
            })
        };
        let installed = self.runtimes.ensure(id, progress).await;
        self.report_install(id, name, None, true);
        match installed {
            Ok(installed) => {
                self.check_provider(id).ok();
                Ok(installed)
            }
            Err(error) => Err(runtime_failure(name, &error)),
        }
    }

    fn report_install(&self, id: &'static str, name: &str, install: Option<(u64, u64)>, now: bool) {
        let mut state = self.state();
        let entry = entry(&mut state, id, name);
        let recent = entry
            .progress_sent
            .is_some_and(|sent| sent.elapsed() < PROGRESS_INTERVAL);
        if recent && !now {
            return;
        }
        entry.progress_sent = Some(Instant::now());
        entry.status.install = install.map(|(received, total)| RuntimeInstall { received, total });
        let status = entry.status.clone();
        state.broadcast(&ServerMessage::ProviderChanged(status));
    }

    /// Downloads the runtime of a provider now, instead of at its first turn.
    pub fn install_runtime(self: &Arc<Self>, provider: &str) -> Result<(), String> {
        let id = self.known_provider(provider)?;
        let profile = self.agent_profile(id)?;
        let setting = self.provider_setting(id)?;
        match self.launch_source(id, &profile, &setting)? {
            // A check reads the files of an installed runtime; one that fails
            // it can then be installed again.
            Source::Ready(_) | Source::Installed(_) => return self.check_provider(id),
            Source::Unsupported => return Err(unsupported(profile.name)),
            Source::Refused(folder) => return Err(refused_programs(profile.name, &folder)),
            Source::NotInstalled(_) => {}
        }
        let daemon = self.clone();
        tokio::spawn(async move {
            if let Err(message) = daemon.install(id, profile.name).await {
                {
                    let mut state = daemon.state();
                    entry(&mut state, id, profile.name).note = Some(message);
                }
                daemon.check_provider(id).ok();
            }
        });
        Ok(())
    }

    /// Sets the executable the daemon starts for a provider, in place of the
    /// runtime it installs. The executable runs as it is, with what the
    /// provider's profile adds to every start.
    pub fn set_provider_executable(
        self: &Arc<Self>,
        provider: &str,
        path: Option<String>,
    ) -> Result<(), Refusal> {
        let id = self.known_provider(provider).map_err(Refusal::Other)?;
        if id == CUSTOM_PROVIDER {
            return Err(invalid(
                "executable",
                "The custom agent is the command given to the daemon with --agent.",
            ));
        }
        let path = path
            .map(|path| path.trim().to_owned())
            .filter(|path| !path.is_empty());
        if let Some(path) = &path {
            let file = std::path::Path::new(path);
            if !file.is_absolute() {
                return Err(invalid(
                    "executable",
                    format!("The path {path} is not a full path."),
                ));
            }
            if !file.is_file() {
                return Err(invalid(
                    "executable",
                    format!("The file {path} does not exist."),
                ));
            }
        }
        self.store
            .with(|store| store.set_provider_executable(id, path.as_deref()))?;
        if let Some(entry) = self.state().providers.get_mut(id) {
            entry.models = None;
            entry.note = None;
        }
        self.check_provider(id).map_err(Refusal::Other)
    }

    /// The state of every provider. A provider that was never checked is
    /// checked now, and its state follows as a change.
    pub fn list_providers(self: &Arc<Self>) -> ServerMessage {
        let ids = self.provider_ids();
        let unchecked: Vec<&str> = {
            let state = self.state();
            ids.iter()
                .copied()
                .filter(|id| !state.providers.get(*id).is_some_and(|entry| entry.checked))
                .collect()
        };
        for id in unchecked {
            self.check_provider(id).ok();
        }
        let state = self.state();
        ServerMessage::Providers(
            ids.into_iter()
                .filter_map(|id| state.providers.get(id))
                .map(|entry| entry.status.clone())
                .collect(),
        )
    }

    /// Asks the provider for its state, and tells every client when it
    /// answers. A runtime that is not installed is not downloaded for it.
    pub fn check_provider(self: &Arc<Self>, provider: &str) -> Result<(), String> {
        let id = self.known_provider(provider)?;
        let profile = self.agent_profile(id)?;
        let setting = self.provider_setting(id)?;
        let source = self.launch_source(id, &profile, &setting)?;
        let runtime = (id != CUSTOM_PROVIDER)
            .then(|| self.runtimes.spec(id))
            .flatten()
            .map(|spec| spec.label());
        let launch = {
            let mut state = self.state();
            let entry = entry(&mut state, id, profile.name);
            if entry.status.checking {
                entry.stale = true;
                return Ok(());
            }
            entry.status.executable.clone_from(&setting.executable);
            entry.status.runtime = runtime;
            let launch = match source {
                Source::Refused(folder) => {
                    entry.rest(
                        ProviderState::RuntimeMissing,
                        refused_programs(profile.name, &folder),
                    );
                    None
                }
                Source::Ready(launch) => {
                    entry.status.checking = true;
                    Some((launch, false))
                }
                Source::Installed(launch) => {
                    entry.status.checking = true;
                    Some((launch, true))
                }
                Source::NotInstalled(spec) => {
                    entry.rest(
                        ProviderState::NotInstalled,
                        format!(
                            "Crow Bot downloads {} ({} MB) when a bot first needs it.",
                            spec.label(),
                            spec.bytes().div_ceil(1 << 20)
                        ),
                    );
                    None
                }
                Source::Unsupported => {
                    entry.rest(ProviderState::RuntimeMissing, unsupported(profile.name));
                    None
                }
            };
            let status = entry.status.clone();
            state.broadcast(&ServerMessage::ProviderChanged(status));
            launch
        };
        let Some((launch, installed)) = launch else {
            return Ok(());
        };
        let daemon = self.clone();
        tokio::spawn(async move {
            if installed && let Err(message) = daemon.verified(id).await {
                let probe = Probe::without_answer(ProviderState::NotInstalled, message);
                daemon.record_probe(id, &profile, setting.signed_in, probe);
                return;
            }
            let probe = match profile.prepare() {
                Ok(()) => acp::probe(&profile, &launch, setting.signed_in).await,
                Err(error) => Probe::without_answer(
                    ProviderState::Unknown,
                    format!("Cannot prepare the folder of {}: {error}", profile.name),
                ),
            };
            daemon.record_probe(id, &profile, setting.signed_in, probe);
        });
        Ok(())
    }

    fn record_probe(
        self: &Arc<Self>,
        id: &'static str,
        profile: &AgentProfile,
        signed_in: bool,
        probe: Probe,
    ) {
        // An agent that wants its sign-in named, and refuses the one it
        // holds, is not asked again before a person signs in.
        if signed_in && profile.session_auth.is_some() && probe.state == ProviderState::SignedOut {
            let forgotten = self
                .store
                .with(|store| store.set_provider_signed_in(id, false));
            if let Err(error) = forgotten {
                eprintln!("[crowbotd] cannot store the sign-in of {id}: {error}");
            }
        }
        let stale = {
            let mut state = self.state();
            let Some(entry) = state.providers.get_mut(id) else {
                return;
            };
            let ready = probe.state == ProviderState::Ready;
            let note = entry.note.take().filter(|_| !ready);
            entry.checked = true;
            entry.status.checking = false;
            entry.status.state = probe.state;
            entry.status.account = probe.account;
            entry.status.detail = probe.detail.or(note);
            entry.status.can_sign_in = !probe.sign_ins.is_empty();
            entry.status.sign_in_methods = probe
                .sign_ins
                .iter()
                .map(|option| SignInMethod {
                    id: option.id.clone(),
                    name: option.name.clone(),
                    description: option.description.clone(),
                })
                .collect();
            entry.status.help_url = None;
            entry.sign_ins = probe.sign_ins;
            let status = entry.status.clone();
            let stale = std::mem::take(&mut entry.stale);
            state.broadcast(&ServerMessage::ProviderChanged(status));
            stale
        };
        if stale {
            self.check_provider(id).ok();
        }
    }

    /// Starts a sign-in of the provider: the one named, or the first it
    /// offers. Its state is read again when the sign-in ends, however it ends.
    pub fn sign_in(self: &Arc<Self>, provider: &str, method: Option<&str>) -> Result<(), String> {
        let id = self.known_provider(provider)?;
        let (option, cancelled) = {
            let mut state = self.state();
            let entry = state
                .providers
                .get_mut(id)
                .ok_or_else(|| format!("the provider {provider} was not checked yet"))?;
            if entry.status.signing_in {
                return Ok(());
            }
            let name = entry.status.name.clone();
            let option = match method {
                Some(method) => entry
                    .sign_ins
                    .iter()
                    .find(|option| option.id == method)
                    .ok_or_else(|| format!("{name} offers no sign-in named {method}."))?,
                None => entry.sign_ins.first().ok_or_else(|| {
                    format!(
                        "{name} offers no sign-in that Crow Bot can start. Sign in with its own program, then check again."
                    )
                })?,
            }
            .clone();
            let (cancel, cancelled) = oneshot::channel();
            entry.cancel = Some(cancel);
            entry.note = None;
            entry.status.signing_in = true;
            entry.status.detail = None;
            let status = entry.status.clone();
            state.broadcast(&ServerMessage::ProviderChanged(status));
            (option, cancelled)
        };
        if let Err(message) = self.start_sign_in(id, option, cancelled) {
            self.sign_in_ended(id, &SignInEnd::NotStarted(message.clone()));
            return Err(message);
        }
        Ok(())
    }

    fn start_sign_in(
        self: &Arc<Self>,
        id: &'static str,
        option: SignInOption,
        cancelled: oneshot::Receiver<()>,
    ) -> Result<(), String> {
        let name = self.provider_name(id);
        let daemon = self.clone();
        match option.start {
            SignInStart::Command(command) => {
                let child = spawn_hidden(&command)
                    .map_err(|error| format!("Cannot start the sign-in of {name}: {error}"))?;
                tokio::spawn(async move {
                    let end = wait_for_sign_in(child, cancelled).await;
                    daemon.sign_in_ended(id, &end);
                });
            }
            SignInStart::Agent => {
                let profile = self.agent_profile(id)?;
                let setting = self.provider_setting(id)?;
                let (launch, installed) = match self.launch_source(id, &profile, &setting)? {
                    Source::Ready(launch) => (launch, false),
                    Source::Installed(launch) => (launch, true),
                    Source::NotInstalled(_) | Source::Unsupported => {
                        return Err(format!("The {name} runtime is not installed."));
                    }
                    Source::Refused(folder) => return Err(refused_programs(&name, &folder)),
                };
                let named = self.clone();
                let on_page = move |page: String| named.sign_in_page(id, page);
                tokio::spawn(async move {
                    if installed && daemon.verified(id).await.is_err() {
                        daemon.sign_in_ended(id, &SignInEnd::Finished(false));
                        return;
                    }
                    let signing_in =
                        acp::authenticate(&launch, &option.id, profile.sign_in_page, on_page);
                    // Dropping the call closes the agent, and its sign-in.
                    let end = tokio::select! {
                        signed_in = signing_in => {
                            SignInEnd::Finished(signed_in.is_ok())
                        }
                        _ = cancelled => SignInEnd::Cancelled,
                        () = tokio::time::sleep(SIGN_IN_PATIENCE) => SignInEnd::TimedOut,
                    };
                    daemon.sign_in_ended(id, &end);
                });
            }
        }
        Ok(())
    }

    /// The provider named the page where the running sign-in finishes. It
    /// reaches the clients, and is never stored.
    fn sign_in_page(&self, id: &'static str, page: String) {
        let mut state = self.state();
        let Some(entry) = state.providers.get_mut(id) else {
            return;
        };
        if !entry.status.signing_in {
            return;
        }
        entry.status.sign_in_url = Some(page);
        let status = entry.status.clone();
        state.broadcast(&ServerMessage::ProviderChanged(status));
    }

    fn sign_in_ended(self: &Arc<Self>, id: &'static str, end: &SignInEnd) {
        if matches!(end, SignInEnd::Finished(true)) {
            let remembered = self
                .store
                .with(|store| store.set_provider_signed_in(id, true));
            if let Err(error) = remembered {
                eprintln!("[crowbotd] cannot store the sign-in of {id}: {error}");
            }
        }
        {
            let mut state = self.state();
            let Some(entry) = state.providers.get_mut(id) else {
                return;
            };
            entry.cancel = None;
            entry.status.signing_in = false;
            entry.status.sign_in_url = None;
            entry.note = match end {
                SignInEnd::Finished(true) | SignInEnd::Cancelled => None,
                SignInEnd::Finished(false) => Some("The sign-in did not finish.".to_owned()),
                SignInEnd::TimedOut => {
                    Some("The sign-in took too long and was stopped.".to_owned())
                }
                SignInEnd::NotStarted(message) => Some(message.clone()),
            };
            // The reason shows at once, and stays when the provider is read
            // again and gives none of its own.
            entry.status.detail.clone_from(&entry.note);
            // What the provider lists can depend on who is signed in.
            entry.models = None;
            let status = entry.status.clone();
            state.broadcast(&ServerMessage::ProviderChanged(status));
        }
        self.check_provider(id).ok();
    }

    pub fn cancel_sign_in(&self, provider: &str) -> Result<(), String> {
        let id = self.known_provider(provider)?;
        let cancel = self
            .state()
            .providers
            .get_mut(id)
            .and_then(|entry| entry.cancel.take());
        if let Some(cancel) = cancel {
            cancel.send(()).ok();
        }
        Ok(())
    }

    /// The models and the reasoning levels of a provider. What a session
    /// reported is answered at once; otherwise the daemon opens a session to
    /// ask, and the list follows as a change.
    pub fn list_models(self: &Arc<Self>, provider: &str) -> Result<ServerMessage, String> {
        let id = self.known_provider(provider)?;
        let name = self.provider_name(id);
        {
            let mut state = self.state();
            let entry = entry(&mut state, id, &name);
            if let Some(models) = &entry.models
                && models.state == ModelsState::Listed
            {
                return Ok(ServerMessage::ProviderModels(models.clone()));
            }
            let loading = ServerMessage::ProviderModels(ProviderModels {
                provider: id.to_owned(),
                state: ModelsState::Loading,
                ..ProviderModels::default()
            });
            if std::mem::replace(&mut entry.reading_models, true) {
                return Ok(loading);
            }
        }
        let daemon = self.clone();
        tokio::spawn(async move {
            let read = daemon.read_models(id).await;
            if let Some(entry) = daemon.state().providers.get_mut(id) {
                entry.reading_models = false;
            }
            match read {
                Ok(offer) => daemon.record_offer(id, offer),
                Err(detail) => daemon.publish_models(
                    id,
                    ProviderModels {
                        provider: id.to_owned(),
                        state: ModelsState::Unavailable,
                        detail: Some(detail),
                        ..ProviderModels::default()
                    },
                ),
            }
        });
        Ok(ServerMessage::ProviderModels(ProviderModels {
            provider: id.to_owned(),
            state: ModelsState::Loading,
            ..ProviderModels::default()
        }))
    }

    /// Opens one session of the provider, with no prompt, to read what it
    /// offers. A runtime that is not installed is not downloaded for it.
    async fn read_models(self: &Arc<Self>, id: &'static str) -> Result<Offer, String> {
        let profile = self.agent_profile(id)?;
        let name = profile.name;
        let setting = self.provider_setting(id)?;
        let launch = match self.launch_source(id, &profile, &setting)? {
            Source::Ready(launch) => launch,
            Source::Installed(launch) => {
                self.verified(id).await?;
                launch
            }
            Source::NotInstalled(_) => {
                return Err(format!("The {name} runtime is not installed yet."));
            }
            Source::Unsupported => return Err(unsupported(name)),
            Source::Refused(folder) => return Err(refused_programs(name, &folder)),
        };
        let folder = self.data_dir.join("providers").join(id).join("models");
        profile
            .prepare()
            .and_then(|()| crate::private::create_folder(&folder))
            .map_err(|error| format!("Cannot prepare the folder of {name}: {error}"))?;
        let asked = acp::read_offer(profile, &launch, folder, setting.signed_in);
        match tokio::time::timeout(MODELS_PATIENCE, asked).await {
            Ok(Ok(offer)) => Ok(offer),
            Ok(Err(failure)) if failure.cause == Some(FailureCause::SignInRequired) => {
                Err(format!("Sign in to {name} to see its models."))
            }
            Ok(Err(failure)) => Err(format!(
                "{name} did not list its models: {}.",
                failure.message.trim_end_matches('.')
            )),
            Err(_) => Err(format!("{name} did not answer in time.")),
        }
    }

    /// Keeps what a session of the provider offers, and tells every client
    /// when it changed. A new session stands on the provider's defaults.
    pub fn record_offer(&self, provider: &str, offer: Offer) {
        let Ok(id) = self.known_provider(provider) else {
            return;
        };
        let name = self.provider_name(id);
        let (default_model, default_effort) = if offer.new_session {
            (offer.model, offer.effort)
        } else {
            self.state()
                .providers
                .get(id)
                .and_then(|entry| entry.models.as_ref())
                .map_or((None, None), |models| {
                    (models.default_model.clone(), models.default_effort.clone())
                })
        };
        let listed = !offer.models.is_empty();
        let mut models = latest_models(offer.models, default_model.as_deref());
        // A session of a turn reports the levels of its own model only: the
        // levels read earlier for the other models stay.
        if let Some(known) = self
            .state()
            .providers
            .get(id)
            .and_then(|entry| entry.models.as_ref())
        {
            for model in models.iter_mut().filter(|model| model.efforts.is_empty()) {
                if let Some(old) = known.models.iter().find(|old| old.id == model.id) {
                    model.efforts = old.efforts.clone();
                    model.default_effort = old.default_effort.clone();
                }
            }
        }
        self.publish_models(
            id,
            ProviderModels {
                provider: id.to_owned(),
                state: if listed {
                    ModelsState::Listed
                } else {
                    ModelsState::Unavailable
                },
                models,
                default_model,
                efforts: offer.efforts,
                default_effort,
                detail: (!listed).then(|| format!("{name} does not list its models.")),
            },
        );
    }

    fn publish_models(&self, id: &'static str, models: ProviderModels) {
        let name = self.provider_name(id);
        let mut state = self.state();
        let entry = entry(&mut state, id, &name);
        if entry.models.as_ref() == Some(&models) {
            return;
        }
        entry.models = Some(models.clone());
        state.broadcast(&ServerMessage::ProviderModels(models));
    }
}

impl ProviderEntry {
    /// The provider cannot be asked: it has no program to start yet.
    fn rest(&mut self, state: ProviderState, detail: String) {
        self.checked = true;
        self.status.state = state;
        self.status.account = None;
        self.status.detail = Some(self.note.clone().unwrap_or(detail));
        self.status.can_sign_in = false;
        self.status.sign_in_methods.clear();
        self.status.help_url = None;
        self.sign_ins.clear();
    }
}

/// Runs the sign-in with no window and no way to read what it prints: its
/// output can hold a one-time link, which is the provider's business.
fn spawn_hidden(command: &SignInCommand) -> std::io::Result<(Child, ProcessTree)> {
    let process = || {
        let mut process = Command::new(&command.program);
        process
            .args(&command.arguments)
            .envs(&command.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        process.process_group(0);
        process
    };
    #[cfg(windows)]
    let child = {
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        // A sign-in leaves the job of the daemon: the browser it opens is the
        // user's, and stays when the daemon ends. A job above the daemon's
        // that forbids it keeps the sign-in in the daemon's job.
        let mut outside = process();
        outside.creation_flags(CREATE_NO_WINDOW | crate::lifetime::BREAKAWAY);
        match outside.spawn() {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                let mut inside = process();
                inside.creation_flags(CREATE_NO_WINDOW);
                inside.spawn()?
            }
            spawned => spawned?,
        }
    };
    #[cfg(unix)]
    let child = process().spawn()?;
    let tree = ProcessTree::of(&child)?;
    Ok((child, tree))
}

async fn wait_for_sign_in(
    (mut child, tree): (Child, ProcessTree),
    cancelled: oneshot::Receiver<()>,
) -> SignInEnd {
    let end = tokio::select! {
        status = child.wait() => SignInEnd::Finished(status.is_ok_and(|status| status.success())),
        _ = cancelled => SignInEnd::Cancelled,
        () = tokio::time::sleep(SIGN_IN_PATIENCE) => SignInEnd::TimedOut,
    };
    // A sign-in that ended by itself keeps what it started: only one that is
    // cancelled or too long is ended with its whole tree.
    if !matches!(end, SignInEnd::Finished(_)) {
        tree.kill();
        child.kill().await.ok();
    }
    end
}

/// The sign-in process and every process it started: a launcher such as
/// `bunx` runs the provider's program as a child of its own.
#[cfg(windows)]
struct ProcessTree(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl ProcessTree {
    fn of(child: &Child) -> std::io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};

        let failed = std::io::Error::last_os_error;
        // SAFETY: both arguments may be null: the job gets the default security
        // and no name.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(failed());
        }
        // SAFETY: `CreateJobObjectW` succeeded, so `job` is a handle this code owns.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let process = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("the sign-in ended at once"))?;
        // SAFETY: `job` is a live job handle and `process` the live handle of the child.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) } == 0 {
            return Err(failed());
        }
        Ok(Self(job))
    }

    /// Ends every process in the job. A job that is only dropped ends none:
    /// what a finished sign-in left running, such as the browser it opened,
    /// is the user's.
    fn kill(self) {
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: the handle is a live job handle that this value owns.
        unsafe {
            TerminateJobObject(self.0.as_raw_handle(), 1);
        }
    }
}

#[cfg(unix)]
struct ProcessTree(Option<i32>);

#[cfg(unix)]
impl ProcessTree {
    #[allow(clippy::unnecessary_wraps)]
    fn of(child: &Child) -> std::io::Result<Self> {
        Ok(Self(child.id().and_then(|id| i32::try_from(id).ok())))
    }

    /// The child leads a process group of its own: the signal reaches the group.
    fn kill(self) {
        if let Some(group) = self.0 {
            // SAFETY: `kill` takes plain integers; a group that is gone is an
            // error it reports, with no other effect.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
    }
}

/// The version of a model id: the numbers of its parts that start with a
/// digit, in order, and the family: the other parts. `gpt-5.2-codex` is
/// version 5.2 of `gpt-codex`, and `claude-opus-4-1` version 4.1 of
/// `claude-opus`.
fn family_and_version(id: &str) -> (String, Vec<u64>) {
    let mut family = Vec::new();
    let mut version = Vec::new();
    for part in id.split('-') {
        if part.starts_with(|c: char| c.is_ascii_digit()) {
            version.extend(part.split('.').map(|number| {
                let digits: String = number.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().unwrap_or(0)
            }));
        } else {
            family.push(part);
        }
    }
    (family.join("-"), version)
}

/// The models worth offering: the newest of each family, in the order the
/// provider lists them. A provider lists the models it still serves, older
/// versions included, and an older version only crowds the list. The
/// default model always stays, since a bot runs on it when it names none.
fn latest_models(models: Vec<ModelChoice>, default: Option<&str>) -> Vec<ModelChoice> {
    let keys: Vec<(String, Vec<u64>)> = models
        .iter()
        .map(|model| family_and_version(&model.id))
        .collect();
    models
        .into_iter()
        .zip(&keys)
        .filter(|(model, (family, version))| {
            default == Some(model.id.as_str())
                || !keys
                    .iter()
                    .any(|(other, newer)| other == family && newer > version)
        })
        .map(|(model, _)| model)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models(ids: &[&str]) -> Vec<ModelChoice> {
        ids.iter()
            .map(|id| ModelChoice {
                id: (*id).to_owned(),
                name: (*id).to_owned(),
                ..ModelChoice::default()
            })
            .collect()
    }

    fn ids(models: &[ModelChoice]) -> Vec<&str> {
        models.iter().map(|model| model.id.as_str()).collect()
    }

    #[test]
    fn only_the_newest_model_of_each_family_is_offered() {
        let listed = models(&[
            "gpt-5.2-codex",
            "gpt-5.1-codex",
            "gpt-5-codex",
            "gpt-5.1-codex-max",
            "claude-opus-4-1",
            "claude-opus-4",
            "claude-sonnet-4-20250514",
            "grok-4.7",
            "grok-4.10",
            "grok-code-fast-1",
            "default",
            "opus",
        ]);
        assert_eq!(
            ids(&latest_models(listed, None)),
            [
                "gpt-5.2-codex",
                "gpt-5.1-codex-max",
                "claude-opus-4-1",
                "claude-sonnet-4-20250514",
                "grok-4.10",
                "grok-code-fast-1",
                "default",
                "opus",
            ]
        );
    }

    #[test]
    fn the_default_model_stays_even_when_a_newer_one_is_listed() {
        let listed = models(&["grok-4.7", "grok-4.8"]);
        assert_eq!(
            ids(&latest_models(listed, Some("grok-4.7"))),
            ["grok-4.7", "grok-4.8"]
        );
    }
}
