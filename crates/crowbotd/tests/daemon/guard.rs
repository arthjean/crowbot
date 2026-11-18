//! The guard that ends an agent with its daemon (`lifetime.rs`), run as the
//! daemon runs it: the daemon's program with `--guard-agent`, leading a
//! process group, and holding the read end of a pipe whose write end stands
//! for the daemon.

use std::io::PipeWriter;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::support;

/// Starts the guard on an agent command, with the read end of a new pipe,
/// and returns the write end: the daemon, for the guard.
fn guard(agent: &[&str]) -> (Child, PipeWriter) {
    let (reader, writer) = std::io::pipe().unwrap();
    let fd = reader.as_raw_fd();
    // The guard inherits the read end, as it inherits the daemon's.
    // SAFETY: `fd` is a descriptor of this process.
    assert_ne!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, -1);
    let child = Command::new(env!("CARGO_BIN_EXE_crowbotd"))
        .arg("--guard-agent")
        .arg(fd.to_string())
        .args(agent)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    drop(reader);
    (child, writer)
}

/// The processes of a process group.
fn group(leader: u32) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            // Fields after the name: state, parent, group.
            let fields: Vec<&str> = stat
                .rsplit_once(')')
                .map(|(_, rest)| rest.split_whitespace().collect())
                .unwrap_or_default();
            fields.first() != Some(&"Z") && fields.get(2) == Some(&leader.to_string().as_str())
        })
        .collect()
}

/// How many times the threads of a process were switched out.
fn switches(pid: u32) -> u64 {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|task| {
            std::fs::read_to_string(task.path().join("status"))
                .unwrap_or_default()
                .lines()
                .filter(|line| line.contains("ctxt_switches"))
                .filter_map(|line| line.split_whitespace().last()?.parse::<u64>().ok())
                .sum::<u64>()
        })
        .sum()
}

fn exited_within(child: &mut Child, patience: Duration) -> Option<std::process::ExitStatus> {
    let mut status = None;
    support::eventually(patience, || {
        status = child.try_wait().unwrap();
        status.is_some()
    });
    status
}

#[test]
fn a_daemon_that_died_before_the_first_check_still_ends_the_agent() {
    let (reader, writer) = std::io::pipe().unwrap();
    // The daemon is gone before the guard even starts.
    drop(writer);
    let fd = reader.as_raw_fd();
    // SAFETY: `fd` is a descriptor of this process.
    assert_ne!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, -1);
    let mut child = Command::new(env!("CARGO_BIN_EXE_crowbotd"))
        .arg("--guard-agent")
        .arg(fd.to_string())
        .args(["sleep", "30"])
        .process_group(0)
        .spawn()
        .unwrap();
    drop(reader);
    let leader = child.id();
    // The second is counted from the moment the guard is the daemon's program:
    // the time the computer takes to start a process is not the guard's.
    let program = std::fs::canonicalize(env!("CARGO_BIN_EXE_crowbotd")).unwrap();
    support::eventually(Duration::from_secs(20), || {
        std::fs::read_link(format!("/proc/{leader}/exe")).map_or(true, |exe| exe == program)
    });
    let status = exited_within(&mut child, Duration::from_secs(1))
        .expect("the guard still runs a second after it started");
    assert!(!status.success(), "{status}");
    assert!(
        support::eventually(Duration::from_secs(1), || group(leader).is_empty()),
        "the agent outlived the daemon: {:?}",
        group(leader)
    );
}

#[test]
fn an_idle_guard_never_wakes_and_ends_the_group_as_soon_as_the_daemon_is_gone() {
    let (mut child, daemon) = guard(&["sh", "-c", "sleep 60 & sleep 60"]);
    let leader = child.id();
    assert!(support::eventually(Duration::from_secs(5), || group(
        leader
    )
    .len()
        >= 3));
    std::thread::sleep(Duration::from_millis(300));
    let before = switches(leader);
    std::thread::sleep(Duration::from_secs(10));
    assert_eq!(
        switches(leader),
        before,
        "the guard woke while nothing happened"
    );

    drop(daemon);
    assert!(exited_within(&mut child, Duration::from_secs(1)).is_some());
    assert!(
        support::eventually(Duration::from_secs(1), || group(leader).is_empty()),
        "the agent's group outlived the daemon: {:?}",
        group(leader)
    );
}

#[test]
fn a_guard_exits_with_the_status_of_an_agent_that_ends_by_itself() {
    let (mut child, _daemon) = guard(&["sh", "-c", "exit 7"]);
    let status = exited_within(&mut child, Duration::from_secs(5)).unwrap();
    assert_eq!(status.code(), Some(7));
    let (mut child, _daemon) = guard(&["sh", "-c", "kill -TERM $$"]);
    let status = exited_within(&mut child, Duration::from_secs(5)).unwrap();
    assert_eq!(status.code(), Some(128 + libc::SIGTERM));
}
