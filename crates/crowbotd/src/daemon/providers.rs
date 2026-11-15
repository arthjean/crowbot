//! What the daemon knows about each provider: whether it can run a turn, and
//! the sign-in it can start. The sign-in is the provider's own program, run as
//! a hidden process: the daemon reads its exit status and nothing else.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crowbot_protocol::{ProviderState, ProviderStatus, ServerMessage};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;

use super::{CUSTOM_PROVIDER, Daemon};
use crate::provider::acp::{self, Probe, SignInCommand};
use crate::provider::claude;

/// A sign-in that nobody finished ends after this long.
const SIGN_IN_PATIENCE: Duration = Duration::from_secs(600);

#[derive(Default)]
pub(super) struct ProviderEntry {
    status: ProviderStatus,
    checked: bool,
    sign_in: Option<SignInCommand>,
    cancel: Option<oneshot::Sender<()>>,
    /// Why the last sign-in did not finish, shown until the provider is ready.
    note: Option<String>,
}

enum SignInEnd {
    Finished(bool),
    Cancelled,
    TimedOut,
}

impl Daemon {
    /// The providers a bot of this daemon can run on.
    fn provider_ids(&self) -> [&'static str; 1] {
        if self.command.is_some() {
            [CUSTOM_PROVIDER]
        } else {
            [claude::PROVIDER]
        }
    }

    fn known_provider(&self, provider: &str) -> Result<&'static str, String> {
        self.provider_ids()
            .into_iter()
            .find(|id| *id == provider)
            .ok_or_else(|| format!("unknown provider: {provider}"))
    }

    /// The state of every provider. A provider that was never checked is
    /// checked now, and its state follows as a change.
    pub fn list_providers(self: &Arc<Self>) -> ServerMessage {
        let ids = self.provider_ids();
        let unchecked: Vec<&str> = {
            let state = self.state();
            ids.into_iter()
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

    /// Asks the provider for its state, and tells every client when it answers.
    pub fn check_provider(self: &Arc<Self>, provider: &str) -> Result<(), String> {
        let id = self.known_provider(provider)?;
        let profile = self.agent_profile(id)?;
        {
            let mut state = self.state();
            let entry = state.providers.entry(id).or_insert_with(|| ProviderEntry {
                status: ProviderStatus {
                    id: id.to_owned(),
                    name: profile.name.to_owned(),
                    ..ProviderStatus::default()
                },
                ..ProviderEntry::default()
            });
            if entry.status.checking {
                return Ok(());
            }
            entry.status.checking = true;
            let status = entry.status.clone();
            state.broadcast(&ServerMessage::ProviderChanged(status));
        }
        let daemon = self.clone();
        tokio::spawn(async move {
            let probe = acp::probe(&profile.command).await;
            daemon.record_probe(id, probe, profile.runtime_url);
        });
        Ok(())
    }

    fn record_probe(&self, id: &'static str, probe: Probe, runtime_url: Option<&'static str>) {
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
        entry.status.can_sign_in = probe.sign_in.is_some();
        entry.status.help_url = runtime_url
            .filter(|_| probe.state == ProviderState::RuntimeMissing)
            .map(str::to_owned);
        entry.sign_in = probe.sign_in;
        let status = entry.status.clone();
        state.broadcast(&ServerMessage::ProviderChanged(status));
    }

    /// Starts the provider's own sign-in. Its state is read again when the
    /// sign-in ends, however it ends.
    pub fn sign_in(self: &Arc<Self>, provider: &str) -> Result<(), String> {
        let id = self.known_provider(provider)?;
        let (child, cancelled) = {
            let mut state = self.state();
            let entry = state
                .providers
                .get_mut(id)
                .ok_or_else(|| format!("the provider {provider} was not checked yet"))?;
            if entry.status.signing_in {
                return Ok(());
            }
            let name = entry.status.name.clone();
            let command = entry.sign_in.as_ref().ok_or_else(|| {
                format!(
                    "{name} offers no sign-in that Crow Bot can start. Sign in with its own program, then check again."
                )
            })?;
            let child = spawn_hidden(command)
                .map_err(|error| format!("Cannot start the sign-in of {name}: {error}"))?;
            let (cancel, cancelled) = oneshot::channel();
            entry.cancel = Some(cancel);
            entry.note = None;
            entry.status.signing_in = true;
            entry.status.detail = None;
            let status = entry.status.clone();
            state.broadcast(&ServerMessage::ProviderChanged(status));
            (child, cancelled)
        };
        let daemon = self.clone();
        tokio::spawn(async move {
            let end = wait_for_sign_in(child, cancelled).await;
            daemon.sign_in_ended(id, &end);
        });
        Ok(())
    }

    fn sign_in_ended(self: &Arc<Self>, id: &'static str, end: &SignInEnd) {
        {
            let mut state = self.state();
            let Some(entry) = state.providers.get_mut(id) else {
                return;
            };
            entry.cancel = None;
            entry.status.signing_in = false;
            entry.note = match end {
                SignInEnd::Finished(true) | SignInEnd::Cancelled => None,
                SignInEnd::Finished(false) => Some("The sign-in did not finish.".to_owned()),
                SignInEnd::TimedOut => {
                    Some("The sign-in took too long and was stopped.".to_owned())
                }
            };
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
}

/// Runs the sign-in with no window and no way to read what it prints: its
/// output can hold a one-time link, which is the provider's business.
fn spawn_hidden(command: &SignInCommand) -> std::io::Result<(Child, ProcessTree)> {
    let mut process = Command::new(&command.program);
    process
        .args(&command.arguments)
        .envs(&command.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    #[cfg(unix)]
    process.process_group(0);
    let child = process.spawn()?;
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
