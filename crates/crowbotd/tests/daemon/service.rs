//! The daemon under the systemd user unit of the repository
//! (`resources/linux/systemd/crowbotd.service`), on the user's own systemd.
//! Each test installs the unit under a name of its own in the runtime unit
//! folder (`$XDG_RUNTIME_DIR/systemd/user`), which systemd forgets at logout,
//! with `ExecStart` pointed at the built daemon and a folder of the test.
//! With no user systemd, each test says so and does nothing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crowbot_protocol::Endpoint;
use crowbot_protocol::transport;

use crate::support;

const UNIT: &str = include_str!("../../resources/linux/systemd/crowbotd.service");

fn systemctl(args: &[&str]) -> std::process::Output {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .unwrap()
}

/// Whether the user's systemd answers. A test with none says so.
fn user_systemd() -> bool {
    let answers = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !answers {
        eprintln!("skipped: no user systemd answers");
    }
    answers
}

/// A path as one argument of a systemd command line.
fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    assert!(!text.contains(['"', '\\', '%', '$']), "{text}");
    format!("\"{text}\"")
}

/// The unit, installed for one test, and removed with it.
struct Unit {
    name: String,
    file: PathBuf,
    dir: support::Scratch,
    endpoint: Endpoint,
}

impl Unit {
    fn install(extra: &[&str]) -> Self {
        let dir = support::test_dir();
        let name = format!(
            "crowbotd-test-{}.service",
            dir.file_name().unwrap().to_string_lossy()
        );
        let endpoint = Endpoint::new(dir.join("crowbotd.sock").to_string_lossy().into_owned());
        let mut command = vec![
            quoted(Path::new(env!("CARGO_BIN_EXE_crowbotd"))),
            "--db".to_owned(),
            quoted(&dir.join("crowbot.db")),
            "--bots-dir".to_owned(),
            quoted(&dir.join("bots")),
            "--endpoint".to_owned(),
            quoted(Path::new(endpoint.address())),
        ];
        command.extend(extra.iter().map(|arg| (*arg).to_owned()));
        let text: String = UNIT
            .lines()
            .map(|line| {
                if line.starts_with("ExecStart=") {
                    format!("ExecStart={}\n", command.join(" "))
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("no XDG_RUNTIME_DIR");
        let folder = Path::new(&runtime).join("systemd").join("user");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join(&name);
        // No `daemon-reload`: systemd reads a unit file it never loaded when
        // the unit is first named, and a reload would read every unit of the
        // user's manager again, for each test of each run.
        std::fs::write(&file, text).unwrap();
        Self {
            name,
            file,
            dir,
            endpoint,
        }
    }

    fn start(&self) -> std::process::Output {
        systemctl(&["start", &self.name])
    }

    fn property(&self, name: &str) -> String {
        let output = systemctl(&["show", &self.name, &format!("--property={name}"), "--value"]);
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn main_pid(&self) -> u32 {
        self.property("MainPID").parse().unwrap_or(0)
    }
}

impl Drop for Unit {
    fn drop(&mut self) {
        systemctl(&["stop", &self.name]);
        std::fs::remove_file(&self.file).ok();
        // A stopped unit whose file is gone is unloaded once it holds no
        // failed state.
        systemctl(&["reset-failed", &self.name]);
    }
}

/// Whether the endpoint answers at the first try.
fn answers(endpoint: &Endpoint) -> bool {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(transport::connect(endpoint)).is_ok()
}

/// `CLOCK_MONOTONIC` in microseconds: the clock of systemd's `…Monotonic`
/// timestamps.
fn monotonic_micros() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid `timespec` for the call to fill.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut now) },
        0
    );
    u64::try_from(now.tv_sec).unwrap() * 1_000_000 + u64::try_from(now.tv_nsec).unwrap() / 1_000
}

#[test]
fn the_unit_is_ready_only_once_the_daemon_listens_and_systemd_restarts_it_after_a_kill() {
    if !user_systemd() {
        return;
    }
    let unit = Unit::install(&[]);
    let started = unit.start();
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    // `Type=notify`: the start returns at READY=1, sent once the endpoint is claimed.
    assert!(
        answers(&unit.endpoint),
        "the daemon was ready before it listened"
    );
    let first = unit.main_pid();
    let cgroup = std::fs::read_to_string(format!("/proc/{first}/cgroup")).unwrap();
    assert!(
        cgroup.trim_end().ends_with(&format!("/{}", unit.name)),
        "{cgroup}"
    );

    let killed = monotonic_micros();
    // SAFETY: `kill` takes plain integers.
    unsafe {
        libc::kill(i32::try_from(first).unwrap(), libc::SIGKILL);
    }
    // The socket is asked first, and systemd only once it answers: the
    // manager starts queued units only when no request waits, so a test that
    // asks it in a tight loop, with others doing the same, would starve the
    // very restart it waits for.
    assert!(
        support::eventually(Duration::from_secs(20), || {
            answers(&unit.endpoint) && {
                let now = unit.main_pid();
                now != 0 && now != first
            }
        }),
        "systemd did not start the daemon again"
    );
    // The bound of 5 seconds is read on systemd's own clock, whatever time
    // the test took to see the restart. It is what the unit and the daemon
    // decide: the restart delay of the unit, then the daemon's start until
    // READY=1. The time the user manager takes to run the restart job is
    // left out: on a computer loaded with 24 test runs at once, that alone
    // went past 5 seconds.
    let timestamp = |name: &str| -> u64 { unit.property(name).parse().unwrap() };
    let started = timestamp("ExecMainStartTimestampMonotonic");
    let ready = timestamp("ActiveEnterTimestampMonotonic");
    assert!(
        started > killed,
        "systemd shows the first daemon as the main process"
    );
    assert!(
        ready >= started,
        "systemd kept the readiness of the first daemon"
    );
    assert_eq!(unit.property("RestartUSec"), "1s");
    let restarted = Duration::from_secs(1) + Duration::from_micros(ready - started);
    assert!(
        restarted < Duration::from_secs(5),
        "the daemon was ready {:?} after it started, and {:?} after it was killed",
        Duration::from_micros(ready - started),
        Duration::from_micros(ready - killed),
    );

    // A stop is a signal: the daemon ends cleanly, and systemd sees success.
    assert!(systemctl(&["stop", &unit.name]).status.success());
    assert_eq!(unit.property("Result"), "success");
    assert_eq!(unit.property("ExecMainStatus"), "0");
    assert!(!Path::new(unit.endpoint.address()).exists());
}

#[test]
fn a_daemon_that_cannot_start_fails_its_unit_with_its_exit_status_and_is_not_restarted() {
    if !user_systemd() {
        return;
    }
    // The database is held, as by a daemon started before the unit.
    let unit = Unit::install(&[]);
    let lock = std::fs::File::create(unit.dir.join("crowbot.db.lock")).unwrap();
    lock.lock().unwrap();
    let started = unit.start();
    assert!(!started.status.success());
    assert_eq!(unit.property("ExecMainStatus"), "3");
    assert_eq!(unit.property("ActiveState"), "failed");
    drop(lock);
}

#[tokio::test]
async fn an_agent_that_left_its_process_group_ends_with_the_unit() {
    if !user_systemd() {
        return;
    }
    let agent = quoted(support::fake_agent());
    let unit = Unit::install(&["--agent", &agent]);
    assert!(unit.start().status.success());
    let mut client = support::Client::connect(&unit.endpoint).await;
    let id = client.new_conversation().await;
    let pid_file = unit.dir.join("escapee");
    client
        .say(id, &format!("escape {}", pid_file.display()))
        .await;
    client.until_reply("escaped").await;
    let escapee: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: `kill` with signal 0 only checks that the process exists.
    let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
    assert!(alive(escapee));

    // The daemon crashes: the guard cannot reach a process outside its group,
    // but the unit's control group holds it.
    let daemon = i32::try_from(unit.main_pid()).unwrap();
    // SAFETY: `kill` takes plain integers.
    unsafe {
        libc::kill(daemon, libc::SIGKILL);
    }
    assert!(
        support::eventually(Duration::from_secs(5), || !alive(escapee)),
        "the process that left the agent's group outlived the daemon"
    );
}

#[tokio::test]
async fn the_daemon_reports_whether_the_users_services_outlive_the_session() {
    let daemon = support::Daemon::start();
    let mut client = daemon.client().await;
    client
        .send(crowbot_protocol::ClientMessage::GetDaemonInfo)
        .await;
    let info = client
        .until(|message| match message {
            crowbot_protocol::ServerMessage::DaemonInfo(info) => Some(info),
            _ => None,
        })
        .await;
    // The account the test runs as, which `USER` may not name: a container
    // job sets none, and a user namespace keeps the one of its creator.
    let account = Command::new("id").arg("-un").output().unwrap();
    assert!(account.status.success());
    let user = String::from_utf8(account.stdout).unwrap().trim().to_owned();
    let expected = Path::new("/run/systemd/system")
        .is_dir()
        .then(|| Path::new("/var/lib/systemd/linger").join(&user).exists());
    assert_eq!(info.linger, expected);
    // What `loginctl` says agrees.
    if let Some(linger) = expected
        && let Ok(output) = Command::new("loginctl")
            .args(["show-user", &user, "--property=Linger", "--value"])
            .output()
        && output.status.success()
    {
        let said = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        assert_eq!(said, if linger { "yes" } else { "no" });
    }
}

#[tokio::test]
async fn a_service_stop_stores_the_running_turn_as_stopped_and_cancels_its_approval() {
    if !user_systemd() {
        return;
    }
    let agent = quoted(support::fake_agent());
    let unit = Unit::install(&["--agent", &agent]);
    assert!(unit.start().status.success());
    let mut client = support::Client::connect(&unit.endpoint).await;
    let id = client.new_conversation().await;
    client.open(id).await;
    client.say(id, "ask Write notes.txt").await;
    let crowbot_protocol::Event::ApprovalRequested { id: approval, .. } = client
        .until_event(|event| matches!(event, crowbot_protocol::Event::ApprovalRequested { .. }))
        .await
    else {
        unreachable!()
    };

    // `systemctl stop` signals every process of the unit at once, the
    // agent and its guard too, not the daemon alone.
    assert!(systemctl(&["stop", &unit.name]).status.success());
    assert_eq!(unit.property("Result"), "success");

    assert!(unit.start().status.success());
    let mut client = support::Client::connect(&unit.endpoint).await;
    let snapshot = client.open(id).await;
    assert!(!snapshot.turn_active);
    let stored = support::events(&snapshot);
    assert!(
        stored.iter().any(|event| matches!(
            event,
            crowbot_protocol::Event::ApprovalClosed {
                id,
                reason: crowbot_protocol::ApprovalClosure::Cancelled,
                ..
            } if *id == approval
        )),
        "{stored:?}"
    );
    assert_eq!(
        stored.last(),
        Some(&crowbot_protocol::Event::TurnEnded(
            crowbot_protocol::TurnEnd::Cancelled
        )),
        "{stored:?}"
    );
}
