use std::time::{Duration, Instant};

use crowbot_protocol::{ApprovalClosure, Event, Interruption, ServerMessage, TimedEvent, TurnEnd};

use crate::support::{self, Daemon};

const LOSS_BOUND: Duration = Duration::from_millis(500);

fn message_text(events: &[TimedEvent]) -> String {
    events
        .iter()
        .filter_map(|timed| match &timed.event {
            Event::MessageText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_killed_daemon_keeps_the_text_streamed_until_half_a_second_before() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "stream 2000 10").await;

    let mut received = Vec::new();
    let streaming = Instant::now();
    while streaming.elapsed() < Duration::from_millis(1800) {
        if let Some(ServerMessage::ConversationEvent {
            event: Event::MessageText(text),
            ..
        }) = client.next_within(Duration::from_millis(50)).await
        {
            received.push((Instant::now(), text));
        }
    }
    let killed = Instant::now();
    daemon.kill();

    let must_be_stored: String = received
        .iter()
        .filter(|(at, _)| killed.duration_since(*at) >= LOSS_BOUND)
        .map(|(_, text)| text.as_str())
        .collect();
    assert!(
        must_be_stored.len() > 100,
        "the stream was too short to test: {must_be_stored:?}"
    );

    daemon.spawn();
    let mut client = daemon.client().await;
    let snapshot = client.open(id).await;
    let stored = message_text(&snapshot.events);
    assert!(
        stored.starts_with(&must_be_stored),
        "stored {} bytes, expected at least the {} bytes received {LOSS_BOUND:?} before the kill",
        stored.len(),
        must_be_stored.len()
    );
    assert!(!snapshot.turn_active);
    assert_eq!(
        snapshot.events.last().map(|timed| &timed.event),
        Some(&Event::TurnInterrupted {
            cause: Interruption::DaemonRestarted
        })
    );
}

#[tokio::test]
async fn a_restart_expires_a_waiting_approval_and_the_next_message_resumes_the_session() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "ask Write notes.txt").await;
    let Event::ApprovalRequested {
        id: approval,
        tool_call_id,
        title,
        ..
    } = client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
    else {
        unreachable!()
    };
    daemon.kill();

    daemon.spawn();
    let mut client = daemon.client().await;
    let snapshot = client.open(id).await;
    assert!(!snapshot.turn_active);
    assert!(snapshot.pending_approvals.is_empty());
    assert_eq!(
        support::events(&snapshot)[snapshot.events.len() - 2..],
        [
            Event::ApprovalClosed {
                id: approval,
                tool_call_id,
                title,
                reason: ApprovalClosure::Expired,
            },
            Event::TurnInterrupted {
                cause: Interruption::DaemonRestarted
            },
        ]
    );

    client.say(id, "session").await;
    client.until_reply("resumed=true").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
}

#[tokio::test]
async fn the_agent_of_a_turn_ends_with_a_daemon_that_crashes() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    // A deaf agent ignores the end of its input: only its own end stops it.
    client.say(id, "deaf").await;
    client.until_reply("deaf").await;
    assert!(daemon.heartbeat().is_some());

    daemon.kill();
    tokio::time::sleep(std::time::Duration::from_millis(1_000)).await;
    let after_kill = daemon.heartbeat();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        daemon.heartbeat(),
        after_kill,
        "the agent still runs after the daemon crashed"
    );
}
