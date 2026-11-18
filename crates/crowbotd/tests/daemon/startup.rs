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
    assert_eq!(
        output.status.code(),
        Some(i32::from(EXIT_ENDPOINT_HELD)),
        "{}",
        stderr(&output)
    );
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsStr;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use crowbot_protocol::transport::Listener;
    use crowbot_protocol::{
        EXIT_ENDPOINT_HELD, EXIT_ENDPOINT_TOO_LONG, Endpoint, SOCKET_PATH_LIMIT,
    };

    use crate::support;

    /// A daemon started with `HOME` set to a folder of the test, the XDG
    /// variables given, and every other XDG variable removed.
    struct Started {
        child: std::process::Child,
        log: PathBuf,
    }

    impl Started {
        fn new(home: &Path, vars: &[(&str, &OsStr)], args: &[&OsStr], umask: Option<u32>) -> Self {
            let log = home.join("daemon.log");
            let mut command = Command::new(env!("CARGO_BIN_EXE_crowbotd"));
            command.env("HOME", home);
            for name in [
                "XDG_DATA_HOME",
                "XDG_STATE_HOME",
                "XDG_CONFIG_HOME",
                "XDG_RUNTIME_DIR",
            ] {
                command.env_remove(name);
            }
            command
                .envs(vars.iter().copied())
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(&log).unwrap());
            if let Some(mask) = umask {
                use std::os::unix::process::CommandExt;
                // SAFETY: `umask` is async-signal-safe.
                unsafe {
                    command.pre_exec(move || {
                        libc::umask(mask);
                        Ok(())
                    });
                }
            }
            Self {
                child: command.spawn().unwrap(),
                log,
            }
        }

        fn said(&self) -> String {
            let mut said = String::new();
            std::fs::File::open(&self.log)
                .unwrap()
                .read_to_string(&mut said)
                .unwrap();
            said
        }

        /// Waits until the daemon listens, or exits.
        fn listening(&mut self) -> String {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let said = self.said();
                // The line is read whole: the daemon may be writing it.
                let listens = said
                    .split_inclusive('\n')
                    .any(|line| line.ends_with('\n') && line.contains("listening on"));
                if listens || self.child.try_wait().unwrap().is_some() || Instant::now() > deadline
                {
                    return said;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn exit(&mut self) -> Option<i32> {
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                if let Some(status) = self.child.try_wait().unwrap() {
                    return status.code();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            None
        }
    }

    impl Drop for Started {
        fn drop(&mut self) {
            // SAFETY: `kill` takes plain integers.
            unsafe {
                libc::kill(i32::try_from(self.child.id()).unwrap(), libc::SIGTERM);
            }
            if self.exit().is_none() {
                self.child.kill().ok();
            }
            self.child.wait().ok();
        }
    }

    /// A socket file that nothing listens on, as a daemon that died leaves
    /// it. The test waits until it refuses connections: a process that
    /// another test thread forks at that moment holds the listener until it
    /// runs its program.
    fn stale_socket(path: &Path) {
        drop(std::os::unix::net::UnixListener::bind(path).unwrap());
        assert!(
            support::eventually(Duration::from_secs(10), || {
                std::os::unix::net::UnixStream::connect(path).is_err()
            }),
            "the socket still listens"
        );
        assert!(path.exists(), "the test left no stale socket");
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
            .permissions()
            .mode()
            & 0o777
    }

    /// Writes, so that SQLite opens its `-wal` and `-shm` files.
    fn write_something(endpoint: &Endpoint) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut client = support::Client::connect(endpoint).await;
            client.new_bot("Private").await;
        });
    }

    fn beside(db: &Path, suffix: &str) -> PathBuf {
        let mut path = db.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn with_no_flag_the_files_sit_in_the_xdg_folders() {
        let home = support::test_dir();
        let runtime_dir = home.join("run");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        // A relative variable is ignored, as the specification says.
        let mut daemon = Started::new(
            &home,
            &[
                ("XDG_RUNTIME_DIR", runtime_dir.as_os_str()),
                ("XDG_DATA_HOME", OsStr::new("relative/data")),
            ],
            &[],
            None,
        );
        let said = daemon.listening();
        let data = home.join(".local/share/crowbot");
        let socket = runtime_dir.join("crowbot/crowbotd.sock");
        assert!(
            said.contains(&format!("listening on {}", socket.display())),
            "{said}"
        );
        assert!(data.join("crowbot.db").is_file(), "{said}");
        assert!(beside(&socket, ".lock").is_file());
        assert_eq!(mode(&data), 0o700);
        assert_eq!(mode(&runtime_dir.join("crowbot")), 0o700);
        assert!(!said.contains("warning"), "{said}");
        assert!(!home.join(".crowbot").exists());
        assert!(!home.join("relative").exists());
        drop(daemon);
        assert!(!socket.exists(), "the stopped daemon left its socket");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn without_a_runtime_folder_the_socket_goes_in_a_private_folder_under_the_data_folder() {
        let home = support::test_dir();
        let data_home = home.join("data");
        let mut daemon = Started::new(
            &home,
            &[("XDG_DATA_HOME", data_home.as_os_str())],
            &[],
            None,
        );
        let said = daemon.listening();
        let socket = data_home.join("crowbot/run/crowbotd.sock");
        assert!(
            said.contains("warning: XDG_RUNTIME_DIR is not set"),
            "{said}"
        );
        assert!(
            said.contains(&format!("listening on {}", socket.display())),
            "{said}"
        );
        assert_eq!(mode(&data_home.join("crowbot/run")), 0o700);
        assert!(data_home.join("crowbot/crowbot.db").is_file());
        drop(daemon);
    }

    #[test]
    fn a_socket_path_that_is_too_long_is_refused_before_anything_exists() {
        let home = support::test_dir();
        let folder = home.join("never");
        let prefix = folder.join("s").to_string_lossy().len();
        let path = |length: usize| {
            let name = "s".repeat(length - prefix + 1);
            folder.join(name).to_string_lossy().into_owned()
        };
        let too_long = path(SOCKET_PATH_LIMIT + 1);
        assert_eq!(too_long.len(), SOCKET_PATH_LIMIT + 1);
        let db = folder.join("data/crowbot.db");
        let mut refused = Started::new(
            &home,
            &[],
            &[
                OsStr::new("--endpoint"),
                OsStr::new(&too_long),
                OsStr::new("--db"),
                db.as_os_str(),
            ],
            None,
        );
        assert_eq!(refused.exit(), Some(i32::from(EXIT_ENDPOINT_TOO_LONG)));
        let said = refused.said();
        assert!(said.contains(&too_long), "{said}");
        assert!(
            said.contains(&format!("{} bytes long", too_long.len())),
            "{said}"
        );
        assert!(
            said.contains(&format!("limit of {SOCKET_PATH_LIMIT} bytes")),
            "{said}"
        );
        assert!(said.contains("--endpoint"), "{said}");
        assert!(
            !folder.exists(),
            "the refused daemon created {}",
            folder.display()
        );
        assert!(!home.join(".local").exists() && !home.join(".crowbot").exists());

        let longest = path(SOCKET_PATH_LIMIT);
        let mut daemon = Started::new(
            &home,
            &[],
            &[
                OsStr::new("--endpoint"),
                OsStr::new(&longest),
                OsStr::new("--db"),
                db.as_os_str(),
            ],
            None,
        );
        let said = daemon.listening();
        assert!(said.contains("listening on"), "{said}");
        drop((refused, daemon));
    }

    #[test]
    fn a_new_database_folder_and_every_file_are_private_under_a_permissive_umask() {
        let home = support::test_dir();
        let db = home.join("new/deeper/crowbot.db");
        let endpoint = support::test_endpoint(&home);
        let mut daemon = Started::new(
            &home,
            &[],
            &[
                OsStr::new("--db"),
                db.as_os_str(),
                OsStr::new("--endpoint"),
                OsStr::new(endpoint.address()),
            ],
            Some(0o022),
        );
        let said = daemon.listening();
        assert!(said.contains("listening on"), "{said}");
        write_something(&endpoint);
        assert_eq!(mode(&home.join("new")), 0o700);
        assert_eq!(mode(&home.join("new/deeper")), 0o700);
        // With its database and endpoint given, it creates nothing in the
        // default folders.
        assert!(!home.join(".local").exists() && !home.join(".crowbot").exists());
        for suffix in ["", "-wal", "-shm", ".lock", ".owner"] {
            assert_eq!(mode(&beside(&db, suffix)), 0o600, "{suffix}");
        }
        drop(daemon);
    }

    #[test]
    fn a_folder_the_user_opened_keeps_its_mode_and_the_files_inside_are_made_private() {
        let home = support::test_dir();
        let folder = home.join("open");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = folder.join("crowbot.db");
        // Files an earlier daemon left readable by everyone.
        for suffix in ["", ".lock", ".owner"] {
            let path = beside(&db, suffix);
            std::fs::write(&path, "").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let endpoint = support::test_endpoint(&home);
        let mut daemon = Started::new(
            &home,
            &[],
            &[
                OsStr::new("--db"),
                db.as_os_str(),
                OsStr::new("--endpoint"),
                OsStr::new(endpoint.address()),
            ],
            Some(0o022),
        );
        let said = daemon.listening();
        assert!(said.contains("listening on"), "{said}");
        write_something(&endpoint);
        assert_eq!(mode(&folder), 0o755);
        for suffix in ["", "-wal", "-shm", ".lock", ".owner"] {
            assert_eq!(mode(&beside(&db, suffix)), 0o600, "{suffix}");
        }
        drop(daemon);
    }

    #[test]
    fn a_database_folder_of_another_account_is_refused() {
        if support::skipped_as_root("the root folder belongs to the account of the test") {
            return;
        }
        let home = support::test_dir();
        let endpoint = support::test_endpoint(&home);
        // The root folder belongs to root.
        let mut daemon = Started::new(
            &home,
            &[],
            &[
                OsStr::new("--db"),
                OsStr::new("/crowbot.db"),
                OsStr::new("--endpoint"),
                OsStr::new(endpoint.address()),
            ],
            None,
        );
        assert_eq!(daemon.exit(), Some(1));
        let said = daemon.said();
        assert!(
            said.contains("the folder / belongs to root (uid 0)"),
            "{said}"
        );
        assert!(!said.contains("listening on"), "{said}");
        drop(daemon);
    }

    #[tokio::test]
    async fn a_stale_socket_is_replaced_under_the_lock() {
        let dir = support::test_dir();
        let path = dir.join("crowbotd.sock");
        stale_socket(&path);
        let endpoint = Endpoint::new(path.to_string_lossy().into_owned());
        let listener = Listener::bind(&endpoint);
        assert!(listener.is_ok(), "{:?}", listener.err());
        drop(listener);
    }

    /// A daemon that holds the lock of a socket may have bound it and not
    /// listen yet: a second one must not remove that socket.
    #[tokio::test]
    async fn a_held_socket_lock_keeps_the_socket_file() {
        let dir = support::test_dir();
        let path = dir.join("crowbotd.sock");
        stale_socket(&path);
        let lock = std::fs::File::create(dir.join("crowbotd.sock.lock")).unwrap();
        lock.lock().unwrap();
        let endpoint = Endpoint::new(path.to_string_lossy().into_owned());
        let refused = Listener::bind(&endpoint).err().map(|error| error.kind());
        let kept = path.exists();
        drop(lock);
        assert_eq!(refused, Some(std::io::ErrorKind::AddrInUse));
        assert!(kept, "the socket of the lock holder was removed");
    }

    /// The arguments that start a daemon on its own database, in `home`, and
    /// on `endpoint`.
    fn on(home: &Path, endpoint: &Endpoint) -> Vec<std::ffi::OsString> {
        vec![
            "--db".into(),
            home.join("crowbot.db").into(),
            "--endpoint".into(),
            endpoint.address().into(),
        ]
    }

    fn started(home: &Path, endpoint: &Endpoint) -> Started {
        let args = on(home, endpoint);
        let args: Vec<&OsStr> = args.iter().map(std::ffi::OsString::as_os_str).collect();
        Started::new(home, &[], &args, None)
    }

    #[test]
    fn a_daemon_replaces_a_stale_socket_and_listens() {
        let home = support::test_dir();
        let endpoint = support::test_endpoint(&home);
        // A socket file that no program listens on, and whose lock nobody holds.
        stale_socket(Path::new(endpoint.address()));
        let mut daemon = started(&home, &endpoint);
        let said = daemon.listening();
        assert!(said.contains("listening on"), "{said}");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(support::Client::connect(&endpoint));
    }

    #[test]
    fn of_two_daemons_that_start_at_once_on_one_socket_exactly_one_listens() {
        let endpoint_home = support::test_dir();
        let endpoint = support::test_endpoint(&endpoint_home);
        for _ in 0..5 {
            let homes = [support::test_dir(), support::test_dir()];
            let mut daemons = [started(&homes[0], &endpoint), started(&homes[1], &endpoint)];
            let said: Vec<String> = daemons.iter_mut().map(Started::listening).collect();
            let listening: Vec<usize> = (0..2)
                .filter(|&index| said[index].contains("listening on"))
                .collect();
            assert_eq!(listening.len(), 1, "{said:?}");
            let other = 1 - listening[0];
            assert_eq!(
                daemons[other].exit(),
                Some(i32::from(EXIT_ENDPOINT_HELD)),
                "{}",
                said[other]
            );
            drop(daemons);
        }
    }
}
