use std::time::{Duration, Instant};

use crowbot_protocol::{ApprovalClosure, ClientMessage, Event, Interruption, TurnEnd};

use crate::support::{self, Daemon};

#[tokio::test]
async fn a_stop_ends_the_running_turn_as_cancelled_within_two_seconds() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "stream 2000 20").await;
    client.until_reply("w0").await;

    let stopped = Instant::now();
    client
        .send(ClientMessage::StopTurn {
            conversation_id: id,
        })
        .await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Cancelled)
    );
    assert!(stopped.elapsed() < Duration::from_secs(2));

    let snapshot = client.open(id).await;
    assert!(!snapshot.turn_active);
    assert_eq!(
        snapshot.events.last().map(|timed| &timed.event),
        Some(&Event::TurnEnded(TurnEnd::Cancelled))
    );
}

#[tokio::test]
async fn a_stop_answers_a_waiting_approval_as_cancelled_and_records_it() {
    let daemon = Daemon::start();
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

    client
        .send(ClientMessage::StopTurn {
            conversation_id: id,
        })
        .await;
    let closed = Event::ApprovalClosed {
        id: approval.clone(),
        tool_call_id,
        title,
        reason: ApprovalClosure::Cancelled,
    };
    assert_eq!(
        client
            .until_event(|event| matches!(event, Event::ApprovalClosed { .. }))
            .await,
        closed
    );
    // The agent reports the outcome it received for its permission request.
    client.until_reply("outcome=cancelled").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Cancelled)
    );

    let snapshot = client.open(id).await;
    assert!(snapshot.pending_approvals.is_empty());
    assert!(support::events(&snapshot).contains(&closed));

    client
        .send(ClientMessage::ResolveApproval {
            conversation_id: id,
            approval_id: approval.clone(),
            decision: crowbot_protocol::ApprovalDecision::Allowed,
        })
        .await;
    let (conversation, refused, why) = client.until_not_resolved().await;
    assert_eq!((conversation, refused), (id, approval));
    assert!(why.contains("no longer waiting"), "{why}");
}

#[tokio::test]
async fn an_agent_that_ignores_the_stop_is_closed_after_ten_seconds() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "deaf").await;
    client.until_reply("deaf").await;

    let stopped = Instant::now();
    client
        .send(ClientMessage::StopTurn {
            conversation_id: id,
        })
        .await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnInterrupted {
            cause: Interruption::AgentUnresponsive
        }
    );
    let waited = stopped.elapsed();
    assert!(waited >= Duration::from_millis(9_500), "{waited:?}");
    assert!(waited < Duration::from_secs(12), "{waited:?}");

    // The provider process is gone: its heartbeat no longer moves.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let last_beat = daemon.heartbeat();
    assert!(last_beat.is_some());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(daemon.heartbeat(), last_beat);

    client.say(id, "session").await;
    client.until_reply("resumed=true").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
}

#[tokio::test]
async fn a_stop_with_no_running_turn_changes_nothing() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client
        .send(ClientMessage::StopTurn {
            conversation_id: id,
        })
        .await;
    assert_eq!(client.next_within(Duration::from_millis(400)).await, None);
    let snapshot = client.open(id).await;
    assert!(snapshot.events.is_empty());
    assert!(!snapshot.turn_active);

    client.say(id, "say still here").await;
    client.until_reply("still here").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
}
