//! The agents end with the daemon. A daemon that is killed, or that crashes,
//! runs no cleanup: without this, an agent that does not notice the end of its
//! input would go on running, with tools it was already allowed to run.
//!
//! On Windows the daemon puts itself in a job object that kills its processes
//! when its last handle closes, which happens when the daemon ends, however it
//! ends. Every process the daemon starts afterwards is in it, with the processes
//! those start. A sign-in leaves it (`BREAKAWAY`): the browser it opens is the
//! user's.
//!
//! Unix has no such object. The daemon starts each agent through a guard, the
//! daemon's own program with `GUARD`: the agent connection makes the guard lead
//! a process group, the guard starts the agent in that group, and it kills the
//! whole group once its parent, the daemon, is gone.

#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::ExitCode;
#[cfg(unix)]
use std::time::Duration;

/// The argument that makes the daemon's program run as the guard of an agent.
#[cfg(unix)]
pub const GUARD: &str = "--guard-agent";

/// How often the guard checks that the daemon is still its parent.
#[cfg(unix)]
const GUARD_INTERVAL: Duration = Duration::from_millis(200);

/// The flag that starts a process outside the daemon's job.
#[cfg(windows)]
pub const BREAKAWAY: u32 = windows_sys::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB;

/// Makes the processes this daemon starts from now on end when it ends.
#[cfg(windows)]
pub fn bind_children() -> std::io::Result<()> {
    use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle};

    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let failed = std::io::Error::last_os_error;
    // SAFETY: both arguments may be null: the job gets the default security,
    // so its handle is not inherited by the processes the daemon starts, and
    // no name.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(failed());
    }
    // SAFETY: `CreateJobObjectW` succeeded, so `job` is a handle this code owns.
    let job = unsafe { OwnedHandle::from_raw_handle(job) };
    // SAFETY: the structure is plain data, for which all zeroes is valid.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    let size = u32::try_from(std::mem::size_of_val(&limits)).map_err(std::io::Error::other)?;
    let raw = std::os::windows::io::AsRawHandle::as_raw_handle(&job);
    // SAFETY: `raw` is a live job handle, and the pointer and the size describe
    // `limits`, which outlives the call.
    let set = unsafe {
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size,
        )
    };
    if set == 0 {
        return Err(failed());
    }
    // SAFETY: `raw` is a live job handle, and the pseudo handle of the current
    // process is always valid.
    if unsafe { AssignProcessToJobObject(raw, GetCurrentProcess()) } == 0 {
        return Err(failed());
    }
    // The handle stays open for the life of the daemon: the system closes it
    // when the daemon ends, and the job then ends what is left in it.
    let _kept = job.into_raw_handle();
    Ok(())
}

/// Unix binds each agent through its guard instead (`guarded`).
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
pub fn bind_children() -> std::io::Result<()> {
    Ok(())
}

/// The program and the arguments that start an agent under its guard.
#[cfg(unix)]
pub fn guarded(program: &Path, args: &[String]) -> (PathBuf, Vec<String>) {
    match std::env::current_exe() {
        Ok(daemon) => {
            let mut guarded = vec![GUARD.to_owned(), program.to_string_lossy().into_owned()];
            guarded.extend(args.iter().cloned());
            (daemon, guarded)
        }
        // With no path to its own program, the daemon starts the agent as it is.
        Err(_) => (program.to_owned(), args.to_vec()),
    }
}

/// Windows binds agents through the daemon's job (`bind_children`).
#[cfg(windows)]
pub fn guarded(program: &std::path::Path, args: &[String]) -> (std::path::PathBuf, Vec<String>) {
    (program.to_owned(), args.to_vec())
}

/// Runs an agent, given as its program and its arguments, under the guard: the
/// agent gets the guard's input and outputs, and its exit status is the
/// guard's. Once the daemon is gone, the guard kills its process group: the
/// agent, what the agent started, and itself.
#[cfg(unix)]
pub fn guard(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let Some(program) = args.next() else {
        eprintln!("[crowbotd] {GUARD} needs the program of an agent");
        return ExitCode::from(2);
    };
    // SAFETY: `getppid` has no preconditions.
    let daemon = unsafe { libc::getppid() };
    let mut agent = match std::process::Command::new(&program).args(args).spawn() {
        Ok(agent) => agent,
        Err(error) => {
            eprintln!(
                "[crowbotd] cannot start {}: {error}",
                program.to_string_lossy()
            );
            return ExitCode::from(127);
        }
    };
    loop {
        match agent.try_wait() {
            Ok(Some(status)) => {
                return ExitCode::from(
                    status
                        .code()
                        .and_then(|code| u8::try_from(code).ok())
                        .unwrap_or(1),
                );
            }
            Ok(None) => {}
            Err(_) => return ExitCode::FAILURE,
        }
        // SAFETY: `getppid` has no preconditions.
        if unsafe { libc::getppid() } != daemon {
            // SAFETY: `kill` takes plain integers; 0 names the process group
            // of the guard, which the agent connection made it lead.
            unsafe {
                libc::kill(0, libc::SIGKILL);
            }
        }
        std::thread::sleep(GUARD_INTERVAL);
    }
}
