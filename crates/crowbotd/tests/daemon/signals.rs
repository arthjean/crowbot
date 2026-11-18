//! A daemon that is told to stop, with SIGTERM, SIGINT, or SIGHUP: it ends its
//! turns, closes its clients and its database, removes its socket, and exits.

use std::path::Path;
use std::time::Duration;

use crowbot_protocol::{ApprovalClosure, Event, Interruption, ServerMessage, TurnEnd};

use crate::support::{self, Daemon};

/// The database has no write-ahead content left: SQLite removes the `-wal`
/// file when the last connection closes.
fn wal_is_empty(db: &Path) -> bool {
    let mut wal = db.as_os_str().to_os_string();
    wal.push("-wal");
    std::fs::metadata(wal).map_or(true, |metadata| metadata.len() == 0)
}

#[test]
fn an_idle_daemon_stops_on_each_signal_within_a_second_and_removes_its_socket() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        let mut daemon = Daemon::start();
        let socket = Path::new(daemon.endpoint().address()).to_owned();
        runtime.block_on(async { drop(daemon.client().await) });
        assert!(socket.exists());

        daemon.signal(signal);
        let (status, took) = daemon
            .wait_exit(Duration::from_secs(1))
            .unwrap_or_else(|| panic!("the daemon still runs a second after signal {signal}"));
        assert!(
            status.success(),
            "signal {signal}: {status}\n{}",
            daemon.log()
        );
        assert!(took < Duration::from_secs(1), "{took:?}");
        assert!(!socket.exists(), "signal {signal} left the socket");
        assert!(wal_is_empty(&daemon.database()), "signal {signal}");
        assert!(
            daemon.log().contains("[crowbotd] stopped"),
            "{}",
            daemon.log()
        );
    }
}

#[tokio::test]
async fn a_signal_stops_the_turn_cancels_its_approval_and_tells_every_client() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut watcher = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;
    client.say(id, "ask Write notes.txt").await;
    let Event::ApprovalRequested {
        id: approval,
        tool_call_id,
        ..
    } = client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
    else {
        unreachable!()
    };
    assert!(!daemon.agent_processes().is_empty());

    daemon.signal(libc::SIGTERM);
    let sent = client.until_closed().await;
    let events: Vec<&Event> = sent
        .iter()
        .filter_map(|message| match message {
            ServerMessage::ConversationEvent { event, .. } => Some(event),
            _ => None,
        })
        .collect();
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::ApprovalClosed { id, reason: ApprovalClosure::Cancelled, .. } if *id == approval
        )),
        "{sent:?}"
    );
    assert_eq!(
        events.last(),
        Some(&&Event::TurnEnded(TurnEnd::Cancelled)),
        "{sent:?}"
    );
    assert_eq!(sent.last(), Some(&ServerMessage::Stopping), "{sent:?}");
    // A client with nothing open learns it too, then sees the connection close.
    assert_eq!(
        watcher.until_closed().await.last(),
        Some(&ServerMessage::Stopping)
    );

    let (status, took) = daemon
        .wait_exit(Duration::from_secs(5))
        .expect("the daemon still runs 5 seconds after the signal");
    assert!(status.success(), "{status}\n{}", daemon.log());
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(
        support::eventually(Duration::from_secs(1), || daemon
            .agent_processes()
            .is_empty()),
        "the agent outlived the daemon: {:?}",
        daemon.agent_processes()
    );
    assert!(wal_is_empty(&daemon.database()));

    // The next daemon finds the turn stored as stopped, with nothing to recover.
    daemon.spawn();
    let mut client = daemon.client().await;
    let snapshot = client.open(id).await;
    assert!(!snapshot.turn_active);
    assert!(snapshot.pending_approvals.is_empty());
    let stored = support::events(&snapshot);
    assert!(
        stored.contains(&Event::ApprovalClosed {
            id: approval,
            tool_call_id,
            title: Some("Write notes.txt".to_owned()),
            reason: ApprovalClosure::Cancelled,
        }),
        "{stored:?}"
    );
    assert_eq!(stored.last(), Some(&Event::TurnEnded(TurnEnd::Cancelled)));
    assert!(!daemon.log().contains("previous daemon left running"));
}

#[tokio::test]
async fn an_agent_that_ignores_the_stop_is_ended_and_the_daemon_still_exits() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    // A deaf agent ignores the cancel and the end of its input.
    client.say(id, "deaf").await;
    client.until_reply("deaf").await;

    daemon.signal(libc::SIGTERM);
    let (status, took) = daemon
        .wait_exit(Duration::from_secs(5))
        .expect("the daemon still runs 5 seconds after the signal");
    assert!(status.success(), "{status}\n{}", daemon.log());
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(
        support::eventually(Duration::from_secs(1), || daemon
            .agent_processes()
            .is_empty()),
        "the deaf agent outlived the daemon"
    );

    daemon.spawn();
    let mut client = daemon.client().await;
    let snapshot = client.open(id).await;
    assert_eq!(
        support::events(&snapshot).last(),
        Some(&Event::TurnInterrupted {
            cause: Interruption::AgentUnresponsive
        })
    );
}

#[tokio::test]
async fn a_second_signal_during_the_stop_ends_the_daemon_at_once() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "deaf").await;
    client.until_reply("deaf").await;

    daemon.signal(libc::SIGTERM);
    tokio::time::sleep(Duration::from_millis(300)).await;
    daemon.signal(libc::SIGINT);
    let (status, took) = daemon
        .wait_exit(Duration::from_secs(1))
        .expect("the daemon still runs a second after the second signal");
    assert_eq!(status.code(), Some(128 + libc::SIGINT), "{}", daemon.log());
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert!(daemon.log().contains("stopped at once"), "{}", daemon.log());
    assert!(
        support::eventually(Duration::from_secs(1), || daemon
            .agent_processes()
            .is_empty()),
        "the agent outlived the daemon"
    );
}
