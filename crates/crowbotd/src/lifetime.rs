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
//! whole group once the daemon is gone.
//!
//! The guard learns it from a pipe. The daemon makes one at its start and holds
//! its write end for its whole life, closed on exec, so no other process holds
//! it; the guard inherits the read end. When the daemon ends, however it ends,
//! the system closes the write end, and the guard's read returns at once. The
//! guard blocks in that read on one thread and in the wait for the agent on
//! the other, so it never wakes on a timer, and a daemon that died before the
//! guard's first read is noticed by that read. An agent that leaves the process
//! group escapes the guard; under the systemd unit, the unit's control group
//! still ends it with the daemon.

#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::fd::RawFd;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::ExitCode;
#[cfg(unix)]
use std::sync::OnceLock;

/// The argument that makes the daemon's program run as the guard of an agent.
#[cfg(unix)]
pub const GUARD: &str = "--guard-agent";

/// The read end of the daemon's pipe, which every process it starts inherits.
#[cfg(unix)]
static LIFELINE: OnceLock<RawFd> = OnceLock::new();

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

/// Makes the pipe that tells each guard when the daemon is gone. Its write
/// end stays open, and closed on exec, until the daemon ends; its read end is
/// left open across exec, for the guards.
#[cfg(unix)]
pub fn bind_children() -> std::io::Result<()> {
    let mut ends = [0; 2];
    // SAFETY: `ends` holds the two descriptors that `pipe2` writes.
    if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let [read, _write] = ends;
    // SAFETY: `read` is a descriptor this process just made.
    if unsafe { libc::fcntl(read, libc::F_SETFD, 0) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // The write end is never closed: the system closes it when the daemon ends.
    LIFELINE.set(read).ok();
    Ok(())
}

/// The program and the arguments that start an agent under its guard.
#[cfg(unix)]
pub fn guarded(program: &Path, args: &[String]) -> (PathBuf, Vec<String>) {
    match (std::env::current_exe(), LIFELINE.get()) {
        (Ok(daemon), Some(lifeline)) => {
            let mut guarded = vec![
                GUARD.to_owned(),
                lifeline.to_string(),
                program.to_string_lossy().into_owned(),
            ];
            guarded.extend(args.iter().cloned());
            (daemon, guarded)
        }
        // With no path to its own program, or no pipe, the daemon starts the
        // agent as it is.
        _ => (program.to_owned(), args.to_vec()),
    }
}

/// Windows binds agents through the daemon's job (`bind_children`).
#[cfg(windows)]
pub fn guarded(program: &std::path::Path, args: &[String]) -> (std::path::PathBuf, Vec<String>) {
    (program.to_owned(), args.to_vec())
}

/// Runs an agent, given as the read end of the daemon's pipe, its program, and
/// its arguments, under the guard: the agent gets the guard's input and
/// outputs, and its exit status is the guard's. Once the daemon is gone, the
/// guard kills its process group: the agent, what the agent started, and
/// itself.
#[cfg(unix)]
pub fn guard(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let lifeline = args
        .next()
        .and_then(|fd| fd.to_str()?.parse::<RawFd>().ok());
    let (Some(lifeline), Some(program)) = (lifeline, args.next()) else {
        eprintln!("[crowbotd] {GUARD} needs the daemon's pipe and the program of an agent");
        return ExitCode::from(2);
    };
    // The agent does not inherit the pipe: only the guard watches it.
    // SAFETY: `fcntl` on a descriptor number has no memory effects; a number
    // that is not open fails, and the read below then ends the group.
    unsafe {
        libc::fcntl(lifeline, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let watching = std::thread::Builder::new()
        .name("daemon-watch".to_owned())
        .spawn(move || watch(lifeline));
    if watching.is_err() {
        // Without a watch the agent could outlive the daemon: it does not start.
        eprintln!("[crowbotd] cannot watch the daemon");
        return ExitCode::FAILURE;
    }
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
    match agent.wait() {
        Ok(status) => ExitCode::from(exit_code(status)),
        Err(_) => ExitCode::FAILURE,
    }
}

/// Blocks until the daemon is gone, then kills the guard's process group.
#[cfg(unix)]
fn watch(lifeline: RawFd) {
    let mut byte = 0_u8;
    loop {
        // SAFETY: the buffer is one live byte. The daemon never writes to
        // the pipe: the read returns only when the daemon is gone.
        let read = unsafe { libc::read(lifeline, (&raw mut byte).cast(), 1) };
        let interrupted =
            read == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted;
        if read <= 0 && !interrupted {
            break;
        }
    }
    // SAFETY: `kill` takes plain integers; 0 names the process group of the
    // guard, which the agent connection made it lead.
    unsafe {
        libc::kill(0, libc::SIGKILL);
    }
}

/// The exit status of the guard for the agent's: its own, or `128 + n` for an
/// agent that signal `n` ended, as a shell reports it.
#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> u8 {
    use std::os::unix::process::ExitStatusExt;

    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}
