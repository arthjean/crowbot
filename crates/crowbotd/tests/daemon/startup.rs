//! The start of a daemon: one daemon per database, the database opened before
//! the endpoint, and an endpoint that a client can reach only once the daemon
//! is ready.

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crowbot_protocol::transport::Listener;
use crowbot_protocol::{EXIT_DATABASE_HELD, EXIT_ENDPOINT_HELD, Endpoint, Event, TurnEnd};

use crate::support::{self, Daemon};

/// A second endpoint for the folder of a test, apart from the one
/// `support::test_endpoint` gives.
fn other_endpoint(dir: &Path) -> Endpoint {
    if cfg!(windows) {
        let named = support::test_endpoint(dir);
        Endpoint::new(format!("{}-other", named.address()))
    } else {
        Endpoint::new(dir.join("other.sock").to_string_lossy().into_owned())
    }
}

/// Runs a daemon on `db` and `endpoint` until it exits on its own. One that
/// still runs after a while is killed, and reported with no status.
fn run(db: &Path, endpoint: &Endpoint) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_crowbotd"))
        .arg("--db")
        .arg(db)
        .args(["--endpoint", endpoint.address()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().ok();
    child.wait_with_output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[tokio::test]
async fn a_second_daemon_on_the_same_database_exits_and_leaves_its_turns_alone() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "stream 100 20").await;
    client.until_reply("w0").await;

    // Another endpoint, so the endpoint does not stop it: the database does.
    let second = run(&daemon.database(), &other_endpoint(daemon.data_dir()));
    let said = stderr(&second);
    assert_eq!(
        second.status.code(),
        Some(i32::from(EXIT_DATABASE_HELD)),
        "{said}"
    );
    assert!(said.contains("another crowbotd"), "{said}");
    let log = std::fs::read_to_string(daemon.data_dir().join("daemon.log")).unwrap();
    let pid = log
        .lines()
        .find_map(|line| line.strip_prefix("[crowbotd] process "))
        .unwrap()
        .trim()
        .to_owned();
    assert!(said.contains(&format!("process {pid}")), "{said}");
    assert!(!said.contains("listening on"), "{said}");

    // The first daemon's turn was not ended by the second one.
    client.until_reply("w99").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
}

#[test]
fn a_daemon_that_finds_its_database_locked_does_not_touch_it() {
    let dir = support::test_dir();
    let db = dir.join("crowbot.db");
    let lock = std::fs::File::create(dir.join("crowbot.db.lock")).unwrap();
    lock.lock().unwrap();

    let output = run(&db, &support::test_endpoint(&dir));
    assert_eq!(
        output.status.code(),
        Some(i32::from(EXIT_DATABASE_HELD)),
        "{}",
        stderr(&output)
    );
    assert!(
        !db.exists(),
        "the daemon opened a database another one holds"
    );
    drop(lock);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_daemon_whose_database_fails_never_claims_its_endpoint() {
    let dir = support::test_dir();
    let endpoint = support::test_endpoint(&dir);
    // The endpoint is taken: a daemon that claimed it first would report it.
    let _holder = Listener::bind(&endpoint).unwrap();
    // A folder is not a database.
    let output = run(&dir, &endpoint);
    let said = stderr(&output);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(said.contains("[crowbotd] failed:"), "{said}");
    assert!(!said.contains("endpoint"), "{said}");
}

#[test]
fn a_daemon_on_a_held_endpoint_exits_with_its_own_status() {
    let dir = support::test_dir();
    let endpoint = support::test_endpoint(&dir);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let holder = runtime.block_on(async { Listener::bind(&endpoint).unwrap() });
    let output = run(&dir.join("crowbot.db"), &endpoint);
    drop(holder);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(
        output.status.code(),
        Some(i32::from(EXIT_ENDPOINT_HELD)),
        "{}",
        stderr(&output)
    );
}

#[cfg(unix)]
mod unix {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use crowbot_protocol::Endpoint;
    use crowbot_protocol::transport::Listener;

    use crate::support;

    #[test]
    fn the_default_folder_is_made_private() {
        let home = support::test_dir();
        let folder = home.join(".crowbot");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        let log = home.join("daemon.log");
        let mut daemon = Command::new(env!("CARGO_BIN_EXE_crowbotd"))
            .env("HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut said = String::new();
        while !said.contains("listening on") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            said.clear();
            std::fs::File::open(&log)
                .unwrap()
                .read_to_string(&mut said)
                .unwrap();
        }
        daemon.kill().ok();
        daemon.wait().ok();
        let mode = std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777;
        std::fs::remove_dir_all(&home).ok();
        assert!(said.contains("listening on"), "{said}");
        assert_eq!(mode, 0o700, "{said}");
        assert!(said.contains("private"), "{said}");
    }

    #[tokio::test]
    async fn a_stale_socket_is_replaced_under_the_lock() {
        let dir = support::test_dir();
        let path = dir.join("crowbotd.sock");
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists(), "the test left no stale socket");
        let endpoint = Endpoint::new(path.to_string_lossy().into_owned());
        let listener = Listener::bind(&endpoint);
        assert!(listener.is_ok());
        drop(listener);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A daemon that holds the lock of a socket may have bound it and not
    /// listen yet: a second one must not remove that socket.
    #[tokio::test]
    async fn a_held_socket_lock_keeps_the_socket_file() {
        let dir = support::test_dir();
        let path = dir.join("crowbotd.sock");
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        let lock = std::fs::File::create(dir.join("crowbotd.sock.lock")).unwrap();
        lock.lock().unwrap();
        let endpoint = Endpoint::new(path.to_string_lossy().into_owned());
        let refused = Listener::bind(&endpoint).err().map(|error| error.kind());
        let kept = path.exists();
        drop(lock);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(refused, Some(std::io::ErrorKind::AddrInUse));
        assert!(kept, "the socket of the lock holder was removed");
    }
}
