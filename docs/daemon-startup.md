# Daemon startup: single instance, readiness, and start order

> Status: implemented, 2026-11-15 (`instance.rs`, `main.rs`, `transport/unix.rs`, `connection.rs`). The open questions below record what was decided. Sources are pinned to a tag or a commit; line numbers refer to that version.

This note answers two questions. How does `crowbotd` make sure that only one daemon uses a database and an endpoint? And how does a client that started a daemon tell "it failed to start" from "it started, then the connection dropped"? It ends with a start order that answers both.

## Terms

- **Endpoint**: where clients connect. A named pipe on Windows, a Unix socket file elsewhere (`crates/crowbot-protocol/src/endpoint.rs`).
- **Lock file**: a file the daemon opens and locks with `File::try_lock` for its whole life. The operating system drops the lock when the process ends, however it ends.
- **Advisory lock**: a lock that only binds programs that ask for it. A **mandatory** lock also blocks reads and writes by other handles.
- **Ready**: the daemon has opened the database, migrated it, and run `Store::recover`, so it can answer any request.

## The problem in this codebase

`run` in `crates/crowbotd/src/main.rs` does, in order: parse the arguments, build `Runtimes`, `Listener::bind(&endpoint)`, `Store::open(&db)` (which migrates), `store.recover()`, then `server::serve`. The bind comes first on purpose: it is the single-instance guard, and `recover` ends every running turn, so it must never run while another daemon uses the database.

That order causes three faults.

1. **A client can reach a daemon that is not ready.** On Windows, a client's `CreateFile` connects as soon as `CreateNamedPipe` returns, before the server calls `ConnectNamedPipe` [R19]. On Unix, `connect` succeeds once the socket listens, before `accept` [R22]. So a client that polls every 25 ms (`START_POLL`, `crates/crowbot-desktop/src/connection.rs`) can connect between the bind and a failure of `Store::open`. It sends `hello`, reads end of file, reports `Disconnected`, reconnects within `RESTART_GUARD`, and shows "The daemon stopped again right after it started" instead of "The daemon stopped as soon as it started (exit status)". This is why `connection::tests::a_daemon_that_exits_at_once_is_reported_with_its_reason` fails about one run in three. The test passes a folder as `--db`, so `Store::open` fails after the bind.
2. **The endpoint does not guard the database.** Two daemons on the same `--db` with different `--endpoint` values both bind, both open the database, and the second one's `recover` ends the first one's running turns. Tests and developers pass `--endpoint`, so this happens outside production too.
3. **The Unix bind is not atomic** (AGENTS.md, "The Unix side of the endpoint"). `Listener::bind` in `transport/unix.rs` connects to test for a live socket, then removes the file, then binds. Two daemons that start together can both pass the test; the second removes the socket that the first has bound and does not yet listen on. Also, `~/.crowbot` keeps the default mode when the client creates it first (`DaemonLauncher::launch` calls `create_dir_all` for the log), because `DirBuilder::mode(0o700)` in `unix.rs` applies only when the daemon creates the folder.

## How other per-user daemons do it

| Project | Single-instance guard | Readiness | Order |
|---|---|---|---|
| PostgreSQL 17 | `postmaster.pid` created with `O_CREAT \| O_EXCL`; a stale file is detected with `kill(pid, 0)` plus checks that the recorded pid is not its own, its parent's, or its grandparent's, because "a reboot will assign exactly the same PID" [R1]. A second lock file `.s.PGSQL.<port>.lock` guards each socket: "Once we have the interlock, we can safely delete any pre-existing socket file" [R2]. | A status line in `postmaster.pid` goes from `starting` to `ready` [R3, R4]. `pg_ctl` reads it and calls `waitpid` on the child to tell a crash from a slow start [R6]. Connections during startup are accepted and refused with "the database system is starting up" [R5]. | data folder lock, shared memory, socket lock, unlink, bind, `starting`, recovery, `ready` [R4]. |
| Docker (dockerd 27.5) | `pkg/pidfile`: read the pid, test that it is alive, then `os.WriteFile`. Check then write, so two starts can race [R7]. | `notifyReady()` (systemd) after the daemon finished initialization [R7]. | pid file, listeners bound (`loadListeners`), `NewDaemon`, ready, then the HTTP server starts serving [R7]. |
| gpg-agent (GnuPG 2.4.7) | The spawner, not the agent, takes a dotlock `gnupg_spawn_agent_sentinel` in the home folder while it starts the agent and waits up to 8 s for the socket [R8]. The agent binds; on `EADDRINUSE` it connects to test for a live agent, then removes and binds again [R8]. | The socket answering is the readiness signal. `--supervised` (systemd socket activation through `LISTEN_FDS`) is printed as deprecated [R8]. | spawn lock, bind, serve. |
| ssh-agent (OpenSSH 9.9p1) | None: each agent gets a fresh `mkdtemp` folder [R9]. | It binds under `umask(0177)` before it forks, so the parent prints `SSH_AUTH_SOCK` only once the socket exists [R9]. | folder, bind, fork. |
| Watchman | `flock(LOCK_EX \| LOCK_NB)` on a pid file, close-on-exec, held for life; the losing process prints the pid read from the file [R10]. Windows: a named mutex [R10]. | The lock is taken "before we daemonize so that errors can be detected and returned (for logging) before we drop stderr" [R10]. | lock, fork, write pid, listen. |
| Bazel 8.0 | Output base lock with `fcntl(F_OFD_SETLK)`, falling back to `F_SETLK`, because POSIX locks "can be lost 'accidentally' due to any close()" [R11]. The holder writes `pid=`, `owner=`, `cwd=` into the locked file for the next process to print [R11]. | The client keeps one end of a `socketpair` whose other end the server inherits; `poll` reporting a hangup means the server died, and the client prints "Server crashed during startup" with the server log [R11]. On Windows it checks the exit time of the process handle [R11]. | lock, start server, poll connect and liveness every 100 ms [R11]. |
| sccache 0.10 | None beyond the TCP port. | The client creates a one-shot channel (a Unix socket, or a named pipe on Windows) "bound before spawning" the server, passes its name in `SCCACHE_STARTUP_NOTIFY`, and reads `Ok`, `AddrInUse`, or `Err { reason }`, with a 10 s timeout [R12]. The server reports each failure it meets during startup on that channel [R12]. | storage check, bind, notify `Ok`, serve. |
| Zed | Linux: connect to a datagram socket, remove it on `ConnectionRefused`, bind: the same race as `crowbotd` [R13]. Windows: a named mutex, then a pipe for arguments [R13]. macOS: a TCP port derived from the user id, plus a handshake [R13]. | Not needed: a second instance hands over its arguments and exits. | |
| tailscaled 1.104 | Connect, remove, listen. The source names the race: "A 'proper' daemon usually uses a dance involving pidfiles to first ensure that no other instances of itself are running" [R14]. | | |
| Hermes Agent (reference project) | A host lock (`flock`, `msvcrt` on Windows) held for life, plus a record with the pid and the process start time; a free lock proves a record stale even when its pid looks alive [R29]. | | |

What the best ones share: **a lock held for the life of the process guards the state, the endpoint is claimed under it, and a stale socket file is removed only while holding a lock** (PostgreSQL, Watchman, Bazel). Pid files checked for liveness (Docker, older PostgreSQL logic) need heuristics for pid reuse. Daemons that only probe the socket (tailscaled, Zed on Linux) carry the race and say so.

## Platform facts

### Locks

- **Rust std.** `File::lock` and `File::try_lock` call `flock(LOCK_EX)` and `flock(LOCK_EX | LOCK_NB)` on Linux, macOS, the BSDs, illumos, and a few others, and return `Unsupported` on the remaining Unix targets [R15]. On Windows they call `LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK` (plus `LOCKFILE_FAIL_IMMEDIATELY` for `try_lock`) over offset 0 and length `u64::MAX`, that is, the whole file; a lock conflict maps to `TryLockError::WouldBlock` [R15]. The documentation says the lock "may be advisory or mandatory" and is released when the file and every handle duplicated or inherited from it are closed [R15]. Std opens files with `O_CLOEXEC` on Unix and non-inheritable on Windows [R15], so the agents and the `--guard-agent` process do not inherit the lock.
- **`flock`.** The lock belongs to the open file description: `fork` and `dup` share it, it survives `execve`, and it is released when all those descriptors are closed [R16]. On Linux it does not interact with `fcntl` locks, but on some systems, such as the modern BSDs, it does; and since Linux 2.6.12 an NFS client emulates `flock` as an `fcntl` lock on the whole file, so the two do interact over NFS [R16].
- **`fcntl` (POSIX record) locks.** Released when the process ends, and also when the process closes *any* descriptor of that file; not inherited across `fork` [R17]. This is what SQLite uses on Unix [R26].
- **`LockFileEx`.** Mandatory: an exclusive lock "denies all other processes both read and write access", and a second handle opened by the same process cannot access the region either [R18]. Locking past the end of the file is allowed [R18]. When a process ends, the system releases its locks, but "the time it takes ... depends upon available system resources" [R18].
- **Killed processes.** A process killed with `SIGKILL` or `TerminateProcess` has its descriptors closed by the kernel, so its `flock` or `LockFileEx` lock is released [R16, R17, R18], on Windows possibly after a short delay [R18]. No cleanup code is needed, which is why a lock beats a pid file.

### Endpoints

- **Windows named pipe.** With `FILE_FLAG_FIRST_PIPE_INSTANCE`, a second creation of the first instance fails with `ERROR_ACCESS_DENIED` [R20]: creation is atomic, so the pipe is a correct guard for its name. An instance is deleted when its last handle closes [R20], so a dead daemon leaves nothing stale. A client connects as soon as the instance exists: `ConnectNamedPipe` then returns `ERROR_PIPE_CONNECTED`, and "there is a good connection between client and server" [R19].
- **Unix socket.** `bind` creates a file "that must be deleted by the caller" [R21], so a killed daemon leaves a stale file. `connect` to a path that does not listen fails with `ECONNREFUSED` [R21]; once the socket listens, connections wait in the backlog queue until `accept` [R22]. Linux honors the permissions of the folder that holds the socket, but some systems ignore the socket's own mode [R21], so the folder mode is the real protection.
- **Linux abstract sockets.** They disappear when the last reference closes, so nothing is stale [R21]. But "socket permissions have no meaning for abstract sockets" [R21], any account can bind the name first, and the namespace is Linux only [R21]. PostgreSQL takes no lock file for them [R2]. They conflict with the rule "The endpoint belongs to one user".

### Service managers (for hosted servers later)

- systemd: a `Type=notify` service sends `READY=1` when startup is finished [R24]; daemon(7) asks new-style daemons to do so, and for socket activation the creation and binding of the socket "is moved out of the daemon code and into the service manager" [R23]. For self-forking daemons, daemon(7) step 14 notifies the original process "via an unnamed pipe or similar communication channel" [R23].
- launchd: the `Sockets` key makes launchd create the sockets; the job checks in to get them through `launch_activate_socket(3)` [R25].

Both hold the endpoint before the daemon is ready. A design whose only guard is the endpoint stops working under them; a lock beside the database keeps working.

## SQLite and the lock

The lock must be a separate file, not the database file.

- On Unix, SQLite uses POSIX advisory locks, and "the close() system call will cancel all POSIX advisory locks on the same file for all threads and all file descriptors in the process" [R26]. A second descriptor on `crowbot.db`, opened for a lock and closed later, would drop SQLite's locks. Even if it stays open, a `flock` on the database interacts with SQLite's `fcntl` locks on the BSDs, on macOS, and over NFS [R16].
- On Windows, `LockFileEx` is mandatory over the whole file [R15, R18]: SQLite's own handle could no longer read the database.
- A lock on `<db>.lock` touches no file SQLite uses: the database, `-wal`, `-shm` (the shared memory index, created in the same folder [R27]), and `-journal`. The two locking schemes never meet.
- `PRAGMA locking_mode = EXCLUSIVE` could serve as the guard, since the connection then "never releases file-locks" [R28]. It loses: the lock is taken at the first read or write, not at open [R28]; it locks out every outside reader (a support session with the `sqlite3` shell, a backup); and a conflict comes back as `SQLITE_BUSY`, which says nothing of who holds it.
- Two hosts cannot share the database anyway: "WAL does not work over a network filesystem" [R27], and network file systems are where locks break [R26]. The lock does not make NFS safe, and does not need to.

## Recommended design

### Start order

1. **Parse the arguments and resolve paths** (as today). Build `Runtimes` (it touches no files).
2. **Prepare the default folder.** On Unix, when the database or the socket is under the default `~/.crowbot`, create it with mode `0700`, or, when it exists, check that the current user owns it and set the mode to `0700` if group or others have any access; refuse to start when another account owns it. For a folder given by `--db` or `--endpoint`, change nothing, and log a warning when the socket's folder is writable by others. Windows: no change. The client creates `~/.crowbot` with `0700` too.

   *Since the Linux port (US-011 to US-013 of `tasks/prd-crow-bot-linux-port.md`):* the endpoint's length is checked first of all (exit status 5, nothing created); on Linux the default folders are the XDG ones (`Folders`, `folders.rs`); the folder of any database, when the daemon creates it, is `0700` and every file of the daemon `0600`; a database folder of another account is refused, whether it is the default one or not.
3. **Lock the database.** Open `<db>.lock` (create, write, no truncate) and call `try_lock`. On `WouldBlock`, read `<db>.owner` (step 4) and exit with status **3**, logging `[crowbotd] failed: another crowbotd (process 1234, endpoint ...) uses the database <db>`. On any other error, exit with status 1. Never delete the lock file: removing it would let a later process lock a new file while the holder keeps the old one. Keep the `File` alive until `run` returns.
4. **Record the owner.** Write `<db>.owner` (to a temporary name, then rename): `pid`, the start time, the endpoint, and the daemon version. It exists for messages only: the lock, not the file, says whether a daemon runs. A pid can be reused after the daemon ends, so nothing ever signals or trusts it. It is a separate file because on Windows the mandatory lock keeps others from reading `<db>.lock` [R18].
5. **Open and migrate the store** (`Store::open`), then **`recover`**. Both run only while this process holds the lock, and every daemon takes the lock before it opens the database, so `recover` never meets another daemon. This makes the hard rule "The database is the source of truth" safe for any choice of `--endpoint`.
6. **Claim the endpoint**, last.
   - Windows: create the first pipe instance, as today. A second daemon on the same name fails here with `ERROR_ACCESS_DENIED` [R20] and exits with status **4**.
   - Unix: open `<socket>.lock` and `try_lock` it, as PostgreSQL locks `.s.PGSQL.<port>.lock` [R2]. Held: exit with status 4. Taken: connect to the socket once; if something answers, it is a process that holds no lock (an older `crowbotd`, or another program), so exit with status 4 rather than steal it. Otherwise remove the stale file, bind, set the mode to `0600`, listen.
7. **Log `[crowbotd] listening on <endpoint>`** and serve. This line now means ready.

`main.rs` keeps `lifetime::bind_children` before the runtime starts, as today.

### Why this closes the Unix race

Two daemons on the same socket path contend for `<socket>.lock`; only the winner may remove or create the socket file, so no daemon removes a socket another one has bound. Two daemons on the same database contend for `<db>.lock` before either opens it. The open decision in AGENTS.md proposed "a lock file held for the life of the daemon"; this design uses two, one per resource, because the database and the endpoint can be chosen apart.

### How the client tells a failed start from a drop

With the endpoint claimed last, an endpoint that answers belongs to a ready daemon. A daemon that fails during startup is never reachable, so the client's existing `try_wait` branch in `Attachment::attach` always sees the exit and reports "The daemon stopped as soon as it started (status)" with the last log line. A drop after `Welcome` is a real drop, and "The daemon stopped again right after it started" becomes accurate.

The client reads the exit status of the daemon it started:

| Status | Meaning | Client |
|---|---|---|
| 1 | Startup failed | Report it with the last log line (as today). |
| 2 | Bad arguments | Report it (as today). |
| 3 | Another daemon holds the database | Keep polling the endpoint until `START_TIMEOUT`: the holder may still be migrating. On timeout, report the loser's log line, which names the holder. |
| 4 | The endpoint is held | Connect once more and attach to the holder (as today after a lost race). |

Status 3 replaces today's race handling: when two clients each start a daemon, the loser now exits at the database lock, possibly before the winner listens, so the client must wait instead of failing.

### What to log

- `process <pid>` (first line, as today; the tests read it).
- `database <db> (locked)` after step 3, and `owner record <db>.owner` after step 4.
- On a lost lock: the holder's pid, start time, and endpoint from `<db>.owner`, or "unknown" when it cannot be read.
- `tightened <folder> to 0700` when step 2 changes a mode.
- `removed a stale socket <path>` in step 6.
- `listening on <endpoint>` only after the bind.

### Failure modes

| Case | Covered? | What happens |
|---|---|---|
| Two daemons, same database, different endpoints | Yes | The second exits with status 3 before it opens the database. Today, its `recover` ends the first one's turns. |
| Two daemons, different databases, same endpoint | Yes | Each locks its own database and runs `recover` on it, which is safe. The second exits with status 4 at step 6, after a migration it did not need. |
| Two daemons start at once, same everything | Yes | One wins `<db>.lock`; the other exits with status 3; both clients attach to the winner. |
| A daemon killed (`SIGKILL`, `TerminateProcess`) | Yes | The system releases both locks [R16, R18]; the next daemon removes the stale Unix socket under the lock; on Windows the pipe is gone with its last handle [R20]. On Windows the release can lag [R18]: a daemon started in that moment exits with status 3 and the client gives up at `START_TIMEOUT`; Retry works. On Unix the old agents may live up to `GUARD_INTERVAL` (200 ms, `lifetime.rs`) after the new daemon starts. |
| `~/.crowbot` created by the client with a permissive umask | Yes | Step 2 sets `0700`, and the client creates it with `0700`. |
| A daemon from before this change, on another endpoint, same database | No | It holds no lock, so a new daemon locks and runs `recover` under it. A new client only starts a daemon when its endpoint is silent, so this needs `--endpoint`. |
| A daemon from before this change, same endpoint | Yes | The connect probe in step 6 finds it, and the new daemon exits with status 4. Its `recover` has already run, which is the case above. |
| Home on NFS | No | `flock` works only when the NFS server supports locking [R16], and SQLite WAL does not work over a network file system [R27]. Not supported, as today. |
| Another process of the same user | No | It can read the database and take the lock; no local design stops the same account. |
| `--data-dir` shared by two daemons on different databases | No | Runtimes have their own locks (`lock_beside`, `runtime.rs`), but `<data>/providers/<id>/home` would be shared (see open questions). |

## Alternatives considered

- **Keep the bind first and patch the client** (option d): after a drop shortly after a launch, wait for the child to exit and report its status. A heuristic built on timing. It still lets clients reach a daemon that is not ready, and fixes neither the Unix race nor `recover` on a database shared across endpoints.
- **Answer `hello` with the startup error** (option c), as PostgreSQL refuses connections with "the database system is starting up" [R5]. This needs the endpoint open before the daemon is ready, and the error must reach the client before the daemon exits, which is a race of its own. PostgreSQL needs it because independent clients connect to a server that can stay in recovery for long. Here, clients can simply wait. Worth revisiting if a migration ever takes long enough to need a "Upgrading the database" screen.
- **A readiness pipe from the daemon to the client** (option b), as sccache [R12], Bazel [R11], and daemon(7) [R23] do. It gives an exact reason and room for progress, but only the client that started the daemon has it: a second client still relies on the endpoint, so the start order must be right anyway. On Windows it means passing an inheritable handle to a detached process. The log file already carries the reason. Not needed now.
- **A pid file checked for liveness** (Docker [R7], PostgreSQL [R1]). Check then write races, and pid reuse needs heuristics (PostgreSQL compares with its own, its parent's, and its grandparent's pid [R1]). A lock answers the same question atomically.
- **A named mutex on Windows** (Watchman, Zed [R10, R13]). Correct, but Windows only; a lock file behaves the same on both platforms.
- **One lock file for both resources.** Simpler, but wrong when `--db` and `--endpoint` are chosen apart, which is exactly the case tests use.
- **Lock the database file, or use `locking_mode = EXCLUSIVE`.** See "SQLite and the lock".
- **Linux abstract socket** for the endpoint. Nothing stale, but no permissions [R21]: another account could take the name first. It breaks the owner-only rule.
- **A blocking `lock()`** for the second daemon. It would hang for the life of the first one; `try_lock` and a clear exit status are better.

## Test plan

All tests wait on a condition (a process exit, a log line, a file), never on a fixed sleep.

Daemon (`crates/crowbotd/tests/daemon/`):

1. `a_second_daemon_on_the_same_database_exits_and_leaves_its_turns_alone`: start daemon A on endpoint E1, start a turn with the scripted agent, start daemon B on E2 with the same `--db`. Wait for B to exit: status 3, its stderr names A's pid. A's turn has no `TurnInterrupted` event.
2. `a_daemon_whose_database_fails_never_claims_its_endpoint`: the test binds E itself with `transport::Listener`, then runs `crowbotd --endpoint E --db <folder>`. It must exit with status 1 and the store error, not the endpoint error: proof that the store opens before the bind.
3. `a_daemon_that_finds_its_database_locked_does_not_touch_it`: the test holds `<db>.lock` with `File::lock`; the daemon exits with status 3; the database file was not created.
4. Unix: `a_stale_socket_is_replaced_under_the_lock`: bind a std `UnixListener`, drop it (the file stays), start the daemon: it listens. `a_held_socket_lock_keeps_the_socket_file`: the test holds `<socket>.lock`; the daemon exits with status 4 and the socket file is still there.
5. Unix: `the_default_folder_is_made_private`: point `HOME` at a temporary folder whose `.crowbot` has mode `0755`; after start, the mode is `0700`.
6. `a_killed_daemon_releases_its_locks`: start, wait for "listening", kill, `wait()` for the exit, start again on the same database and endpoint: it listens and logs the turns it ended. Extends `recovery.rs`.
7. Unit: two `try_lock` calls on two handles of `<db>.lock` in one process: the second returns `WouldBlock` on both platforms, as the std documentation promises for any other handle [R15].

Desktop (`crates/crowbot-desktop/src/connection.rs`):

1. `a_daemon_that_exits_at_once_is_reported_with_its_reason` becomes deterministic without change: a daemon whose store fails never claims its endpoint, so the client can only see the exit.
2. `a_client_waits_for_the_daemon_that_holds_the_database`: the test holds `<db>.lock`, the client starts a daemon, the test waits until the loser's log line appears, then binds the endpoint with `answering_daemon` and answers `Welcome`. The client must report `Connected`, not `Failed`.
3. `two_clients_that_start_together_share_one_daemon` (exists) now covers status 3 as well.

Holding a lock from the desktop tests needs `File::lock`, stable since Rust 1.89, but `crates/crowbot-desktop/Cargo.toml` declares `rust-version = "1.88"`. Raise it, or let test 2 hold the lock through a small helper mode of `crowbotd`.

## Decisions

1. Exit statuses 3 (`EXIT_DATABASE_HELD`) and 4 (`EXIT_ENDPOINT_HELD`) are a contract between `crowbotd` and its clients, named in the protocol crate and in `--help`.
2. The daemon tightens an existing `~/.crowbot` to `0700` and logs it; it refuses to start when another account owns the folder.
3. `<db>.owner` is kept: the message of a daemon that finds the database held names the pid and the endpoint of the holder.
4. A daemon that finds the database locked retries `try_lock` for two seconds before it exits with status 3, for the late release of a killed process's lock on Windows [R18].
5. The shared data folder gets no lock of its own yet: two daemons on different databases with one `--data-dir` stay unsupported.
6. The Unix client now checks with `SO_PEERCRED` that the process on the socket runs as its own account, as the Windows client checks the owner of the pipe.
7. For hosted servers, socket activation only with the database lock kept as the guard remains to be done. `READY=1` after the endpoint is claimed is done, for the systemd user unit (`systemd.rs`, US-015).

Not covered by tests: the `SO_PEERCRED` refusal needs a second account; macOS is not verified.

## References

- [R1] PostgreSQL `REL_17_0` (commit d7ec59a63d74), `src/backend/utils/init/miscinit.c`: `CreateLockFile` L1150, `O_RDWR | O_CREAT | O_EXCL` L1215, `kill(other_pid, 0)` L1294, `CreateDataDirLockFile` L1455, `CreateSocketLockFile` L1464. https://github.com/postgres/postgres/blob/REL_17_0/src/backend/utils/init/miscinit.c
- [R2] PostgreSQL `REL_17_0`, `src/backend/libpq/pqcomm.c`, `Lock_AF_UNIX` L684. https://github.com/postgres/postgres/blob/REL_17_0/src/backend/libpq/pqcomm.c
- [R3] PostgreSQL `REL_17_0`, `src/include/utils/pidfile.h` (`LOCK_FILE_LINE_PM_STATUS`, `PM_STATUS_STARTING`, `PM_STATUS_READY`). https://github.com/postgres/postgres/blob/REL_17_0/src/include/utils/pidfile.h
- [R4] PostgreSQL `REL_17_0`, `src/backend/postmaster/postmaster.c`: `CreateDataDirLockFile` L891, `ListenServerPort` L1117 to L1218, `PM_STATUS_STARTING` L1353, `PM_STATUS_READY` L2487. https://github.com/postgres/postgres/blob/REL_17_0/src/backend/postmaster/postmaster.c
- [R5] PostgreSQL `REL_17_0`, `src/backend/tcop/backend_startup.c` L274 (`CAC_STARTUP`). https://github.com/postgres/postgres/blob/REL_17_0/src/backend/tcop/backend_startup.c
- [R6] PostgreSQL `REL_17_0`, `src/bin/pg_ctl/pg_ctl.c`, `wait_for_postmaster_start` L591. https://github.com/postgres/postgres/blob/REL_17_0/src/bin/pg_ctl/pg_ctl.c
- [R7] Moby `v27.5.0` (commit 38b84dce32c4): `pkg/pidfile/pidfile.go`; `cmd/dockerd/daemon.go` L156 to L163 (pid file), L179 (`loadListeners`), L259 (`NewDaemon`), L317 (`notifyReady`). https://github.com/moby/moby/tree/v27.5.0
- [R8] GnuPG `gnupg-2.4.7` (commit 7bdaf5647991, GitHub mirror of the GnuPG git): `common/asshelp.c` L57 (`SECS_TO_WAIT_FOR_AGENT`), `lock_spawning` L258 to L296; `agent/gpg-agent.c` L1402 (`--supervised` deprecated), `create_server_socket` L2262 to L2300. https://github.com/gpg/gnupg/tree/gnupg-2.4.7
- [R9] OpenSSH `V_9_9_P1` (commit 53a80baaebda), `ssh-agent.c` L2344 (`mkdtemp`), L2360 (`umask(0177)`), L2361 (`unix_listener`), L2384 (`fork`). https://github.com/openssh/openssh-portable/blob/V_9_9_P1/ssh-agent.c
- [R10] Watchman (commit b9617699bde0): `watchman/main.cpp` L271 to L305 (`run_service_as_daemon`); `watchman/ProcessLock.cpp` L30 to L135. https://github.com/facebook/watchman/tree/b9617699bde057b1d919b129e072759ecae2dcdd/watchman
- [R11] Bazel `8.0.0` (commit c8217fdd2f20): `src/main/cpp/blaze_util_posix.cc` L372 (`IsStillAlive`), L430 (`socketpair`), L609 (`setlk`, OFD locks), L641 to L740 (`AcquireLock`); `src/main/cpp/blaze.cc` L752 to L808 (`ConnectOrDie`); `src/main/cpp/blaze_util_windows.cc` L619. https://github.com/bazelbuild/bazel/tree/8.0.0/src/main/cpp
- [R12] sccache `v0.10.0` (commit e3d1ed560a28): `src/commands.rs` L49, L91 to L120 (`run_server_process`); `src/server.rs` L419 to L556 (`start_server`). https://github.com/mozilla/sccache/tree/v0.10.0/src
- [R13] Zed (commit 703b5ac3f4ff): `crates/zed/src/zed/open_listener.rs` L415 to L428; `crates/zed/src/zed/windows_only_instance.rs` L28 to L40; `crates/zed/src/zed/mac_only_instance.rs` L17 to L104; `crates/zed/src/main.rs` L364 to L388. https://github.com/zed-industries/zed
- [R14] Tailscale `v1.104.7` (commit cbc74fe48ef1), `safesocket/unixsocket.go` L24 to L69. https://github.com/tailscale/tailscale/blob/v1.104.7/safesocket/unixsocket.go
- [R15] Rust std 1.100.0 (commit 0ef1bc386059): `library/std/src/fs.rs` (`File::lock` documentation); `library/std/src/sys/fs/unix.rs` L1235 (`O_CLOEXEC`), L1323 to L1390 (`flock`); `library/std/src/sys/fs/windows.rs` L213 (`inherit_handle: false`), L390 to L460 (`LockFileEx`). https://github.com/rust-lang/rust/tree/0ef1bc386059596dada8fb65b50425aab2ea637a/library/std/src
- [R16] flock(2), Linux man-pages. https://man7.org/linux/man-pages/man2/flock.2.html
- [R17] F_SETLK(2const), Linux man-pages. https://man7.org/linux/man-pages/man2/F_SETLK.2const.html
- [R18] LockFileEx, Microsoft Learn. https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex
- [R19] ConnectNamedPipe, Microsoft Learn. https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-connectnamedpipe
- [R20] CreateNamedPipeA, Microsoft Learn. https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
- [R21] unix(7), Linux man-pages (pathname permissions, abstract sockets, `ECONNREFUSED`, `SO_PEERCRED`). https://man7.org/linux/man-pages/man7/unix.7.html
- [R22] listen(2), Linux man-pages. https://man7.org/linux/man-pages/man2/listen.2.html
- [R23] daemon(7), systemd. https://man7.org/linux/man-pages/man7/daemon.7.html
- [R24] sd_notify(3), systemd. https://www.freedesktop.org/software/systemd/man/latest/sd_notify.html
- [R25] launchd.plist(5), macOS manual page (`man 5 launchd.plist`), key `Sockets`.
- [R26] SQLite, "How To Corrupt An SQLite Database File", sections 2.1 and 2.2. https://www.sqlite.org/howtocorrupt.html
- [R27] SQLite, "Write-Ahead Logging", sections 1, 2.2, and 7. https://www.sqlite.org/wal.html
- [R28] SQLite, "Pragma statements", `locking_mode`. https://www.sqlite.org/pragma.html#pragma_locking_mode
- [R29] Hermes Agent (commit d3d4ee4c7c0e), `gateway/host_rendezvous.py` L1 to L30, L556 to L600. Reference project, read for ideas only.
