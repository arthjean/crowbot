//! EP-002 of the harness PRD: the daemon guards a turn. It stores what a turn
//! consumed, records who started it, stops a turn that makes no progress, a
//! turn of its own that went silent, and a turn over its bot's tool call
//! limit, sends again a turn that failed before it said anything, and starts
//! no turn of its own while it is paused.

use std::time::{Duration, Instant};

use crowbot_protocol::{
    ApprovalDecision, BotPatch, ClientMessage, Cost, Event, Interruption, ServerMessage,
    TimedEvent, TurnEnd, TurnOrigin, TurnUsage, UserMessage,
};

use crate::support::{self, Client, Daemon};

#[path = "../../src/guard/limits.rs"]
mod limits;

use limits::{INACTIVITY_LIMIT, NO_PROGRESS_CYCLE, NO_PROGRESS_REPEATS, RETRY_DELAYS};

/// A daemon whose guards wait a hundred times less: a retry waits 150 ms
/// and a silent turn of its own is stopped after 6 seconds.
const FAST: u32 = 100;
/// A daemon whose first retry waits 1.5 seconds: long enough for a person
/// to act before it.
const SLOW: u32 = 10;

/// Whether an event that matches reaches the client within `patience`.
async fn arrives_within(
    client: &mut Client,
    patience: Duration,
    matches: impl Fn(&Event) -> bool,
) -> bool {
    let deadline = Instant::now() + patience;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match client.next_within(left).await {
            Some(ServerMessage::ConversationEvent { event, .. }) if matches(&event) => {
                return true;
            }
            Some(_) => {}
            None => return false,
        }
    }
    false
}

fn message(event: &Event) -> Option<&UserMessage> {
    match event {
        Event::UserText(message) => Some(message),
        _ => None,
    }
}

fn is_retry(event: &Event) -> bool {
    message(event).is_some_and(|message| message.origin == TurnOrigin::Retry)
}

fn stored(daemon: &Daemon, id: i64) -> Vec<TimedEvent> {
    let database = rusqlite::Connection::open(daemon.database()).unwrap();
    let mut statement = database
        .prepare(
            "SELECT id, payload, created_at_ms FROM events WHERE conversation_id = ?1 ORDER BY id",
        )
        .unwrap();
    statement
        .query_map([id], |row| {
            Ok(TimedEvent {
                id: row.get(0)?,
                event: serde_json::from_str(&row.get::<_, String>(1)?).unwrap(),
                at_ms: row.get(2)?,
            })
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn marker(daemon: &Daemon, name: &str) -> String {
    daemon.folder("markers").join(name).display().to_string()
}

// US-006: the usage an agent reports.

/// A row of `turn_usage`: the turn, the conversation, the provider, the
/// context used and its size, the session's cost and the turn's, and the
/// input, output, and total tokens.
type UsageRow = (
    i64,
    i64,
    String,
    i64,
    i64,
    f64,
    f64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

#[tokio::test]
async fn the_usage_of_each_turn_is_stored_and_a_snapshot_carries_the_last_one() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;

    assert_eq!(client.ask(id, "spend 100 1000 0.5 10 20").await, "spent");
    assert_eq!(client.ask(id, "spend 300 1000 1.25 - -").await, "spent");

    let usd = |amount| Cost {
        amount,
        currency: "USD".to_owned(),
    };
    let last = TurnUsage {
        provider: "custom".to_owned(),
        context_used: Some(300),
        context_size: Some(1000),
        session_cost: Some(usd(1.25)),
        turn_cost: Some(usd(0.75)),
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
    };
    assert_eq!(client.open(id).await.usage, Some(last));

    let turns: Vec<i64> = stored(&daemon, id)
        .into_iter()
        .filter(|timed| message(&timed.event).is_some())
        .map(|timed| timed.id)
        .collect();
    let database = rusqlite::Connection::open(daemon.database()).unwrap();
    let rows: Vec<UsageRow> = database
        .prepare(
            "SELECT turn_event_id, conversation_id, provider, context_used, context_size,
                    session_cost, turn_cost, input_tokens, output_tokens, total_tokens
                 FROM turn_usage ORDER BY turn_event_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [
            (
                turns[0],
                id,
                "custom".to_owned(),
                100,
                1000,
                0.5,
                0.5,
                Some(10),
                Some(20),
                Some(30)
            ),
            (
                turns[1],
                id,
                "custom".to_owned(),
                300,
                1000,
                1.25,
                0.75,
                None,
                None,
                None
            ),
        ]
    );
}

#[tokio::test]
async fn a_turn_whose_agent_reports_no_usage_has_no_row_and_an_unknown_usage() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;
    assert_eq!(client.ask(id, "spend 5 50 - - -").await, "spent");
    assert!(client.open(id).await.usage.is_some());

    assert_eq!(client.ask(id, "say plain").await, "plain");
    assert_eq!(client.open(id).await.usage, None);
    let database = rusqlite::Connection::open(daemon.database()).unwrap();
    let rows: i64 = database
        .query_row("SELECT COUNT(*) FROM turn_usage", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

// US-007: the origin of a turn.

#[tokio::test]
async fn a_message_a_client_sends_is_a_person_s_whatever_origin_it_names() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    let (mut reader, mut writer) = daemon.raw_connection().await;
    writer
        .write(&serde_json::json!({
            "type": "hello",
            "data": { "protocol_version": crowbot_protocol::PROTOCOL_VERSION }
        }))
        .await
        .unwrap();
    writer
        .write(&serde_json::json!({
            "type": "send_message",
            "data": { "conversation_id": id, "text": "say mine", "origin": "routine" }
        }))
        .await
        .unwrap();
    loop {
        let read: ServerMessage = reader.read().await.unwrap().unwrap();
        if matches!(read, ServerMessage::MessageAccepted { .. }) {
            break;
        }
    }
    let snapshot = client.open(id).await;
    let origins: Vec<_> = support::events(&snapshot)
        .iter()
        .filter_map(|event| message(event).map(|message| message.origin))
        .collect();
    assert_eq!(origins, [TurnOrigin::User]);
}

// US-008: a turn that makes no progress.

#[tokio::test]
async fn a_turn_that_repeats_the_same_calls_is_stopped_and_its_queue_paused() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;

    client.say(id, "calls 20 notes.txt same").await;
    client.say(id, "say after").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnInterrupted {
            cause: Interruption::NoProgress
        }
    );
    let queue = client.until_queue(|queue| queue.paused).await;
    assert_eq!(support::queued_texts(&queue), ["say after"]);
    let calls = stored(&daemon, id)
        .iter()
        .filter(|timed| matches!(timed.event, Event::ToolCallStarted { .. }))
        .count();
    assert!((NO_PROGRESS_REPEATS..20).contains(&calls), "{calls} calls");
}

#[tokio::test]
async fn a_repeated_cycle_or_calls_with_no_output_are_stopped_and_new_outputs_are_not() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;
    let stopped = Event::TurnInterrupted {
        cause: Interruption::NoProgress,
    };

    for period in 2..=NO_PROGRESS_CYCLE {
        let inputs = ["a", "b", "c", "d"][..period].join(",");
        let count = period * (NO_PROGRESS_REPEATS + 3);
        client
            .say(id, &format!("calls {count} {inputs} same"))
            .await;
        assert_eq!(client.until_turn_ends().await, stopped, "period {period}");
    }

    client.say(id, "calls 10 ls _").await;
    assert_eq!(client.until_turn_ends().await, stopped);

    let outputs: Vec<String> = (0..10).map(|index| index.to_string()).collect();
    let reply = client
        .ask(id, &format!("calls 10 ls {}", outputs.join(",")))
        .await;
    assert_eq!(reply, "calls done");
}

// US-009: the limits of a turn.

#[tokio::test]
async fn a_silent_turn_of_the_daemon_is_stopped_and_a_person_s_is_not() {
    let daemon = Daemon::start_scaled(FAST);
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    let other = client.new_conversation().await;
    client.open(id).await;
    client.open(other).await;

    // A person's turn that goes silent runs on.
    client.say(other, "ponder quietly").await;
    client
        .until_event(|event| matches!(event, Event::ThoughtText(_)))
        .await;

    let failing = format!("failfirst 1 {} ponder hmm", marker(&daemon, "silent"));
    client.say(id, &failing).await;
    client.until_event(is_retry).await;
    let retried = Instant::now();
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnInterrupted {
            cause: Interruption::Inactive
        }
    );
    assert!(retried.elapsed() >= daemon.scaled(INACTIVITY_LIMIT) - Duration::from_millis(100));

    let snapshot = client.open(other).await;
    assert!(snapshot.turn_active, "the person's turn was stopped");
    client
        .send(ClientMessage::StopTurn {
            conversation_id: other,
        })
        .await;
}

#[tokio::test]
async fn a_turn_of_the_daemon_that_waits_for_an_approval_is_not_stopped() {
    let daemon = Daemon::start_scaled(FAST);
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;

    let asking = format!("failfirst 1 {} ask Write notes.txt", marker(&daemon, "ask"));
    client.say(id, &asking).await;
    client.until_event(is_retry).await;
    let Event::ApprovalRequested { id: approval, .. } = client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
    else {
        unreachable!();
    };
    let wait = daemon.scaled(INACTIVITY_LIMIT) + Duration::from_secs(1);
    assert!(!arrives_within(&mut client, wait, Event::ends_turn).await);

    client
        .send(ClientMessage::ResolveApproval {
            conversation_id: id,
            approval_id: approval,
            decision: ApprovalDecision::Declined,
        })
        .await;
    client.until_reply("outcome=").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
}

#[tokio::test]
async fn a_turn_over_its_bot_s_tool_call_limit_is_stopped() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut watcher = daemon.client().await;
    let bot = client.new_bot("Counter").await;
    let (bot_id, id) = (bot.bot.id, bot.current_conversation_id);
    assert_eq!(bot.bot.tool_call_limit, None);
    client.open(id).await;

    // With no limit, a turn makes as many calls as it wants.
    assert_eq!(
        client.ask(id, "calls 4 a,b,c,d 1,2,3,4").await,
        "calls done"
    );

    let limit = |limit| BotPatch {
        tool_call_limit: Some(limit),
        ..BotPatch::default()
    };
    client.update_bot(bot_id, limit(Some(0))).await;
    client.until_invalid().await;
    client.update_bot(bot_id, limit(Some(2))).await;
    watcher
        .until_bot(|summary| summary.bot.id == bot_id && summary.bot.tool_call_limit == Some(2))
        .await;

    client.say(id, "calls 4 a,b,c,d 1,2,3,4").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnInterrupted {
            cause: Interruption::BudgetExhausted
        }
    );
    let calls = stored(&daemon, id)
        .iter()
        .rev()
        .take_while(|timed| message(&timed.event).is_none())
        .filter(|timed| matches!(timed.event, Event::ToolCallStarted { .. }))
        .count();
    // The turn stops at the call over its limit; the agent may start one
    // more before the cancel reaches it.
    assert!((3..=4).contains(&calls), "{calls} calls");

    client.update_bot(bot_id, limit(None)).await;
    watcher
        .until_bot(|summary| summary.bot.id == bot_id && summary.bot.tool_call_limit.is_none())
        .await;
    assert_eq!(
        client.ask(id, "calls 4 a,b,c,d 1,2,3,4").await,
        "calls done"
    );
}

// US-010: a retry of a turn that failed before it said anything.

#[tokio::test]
async fn a_turn_that_failed_before_it_said_anything_is_sent_again_on_a_ladder() {
    let daemon = Daemon::start_scaled(FAST);
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;

    // Once: the retry succeeds.
    let once = format!("failfirst 1 {} say back", marker(&daemon, "once"));
    client.say(id, &once).await;
    assert!(matches!(
        client.until_turn_ends().await,
        Event::TurnFailed { .. }
    ));
    let retried = client.until_event(is_retry).await;
    assert_eq!(message(&retried).unwrap().text, once);
    client.until_reply("back").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );

    // Always: three retries, each after its wait, then none.
    let other = client.new_conversation().await;
    client.open(other).await;
    let always = format!("failfirst 99 {} say never", marker(&daemon, "always"));
    client.say(other, &always).await;
    for _ in 0..=RETRY_DELAYS.len() {
        assert!(matches!(
            client.until_turn_ends().await,
            Event::TurnFailed { .. }
        ));
    }
    let last = *RETRY_DELAYS.last().unwrap();
    assert!(!arrives_within(&mut client, daemon.scaled(last) * 3, is_retry).await);

    let events = stored(&daemon, other);
    let starts: Vec<&TimedEvent> = events
        .iter()
        .filter(|timed| message(&timed.event).is_some())
        .collect();
    let origins: Vec<TurnOrigin> = starts
        .iter()
        .map(|timed| message(&timed.event).unwrap().origin)
        .collect();
    assert_eq!(
        origins,
        [
            TurnOrigin::User,
            TurnOrigin::Retry,
            TurnOrigin::Retry,
            TurnOrigin::Retry
        ]
    );
    let failures: Vec<i64> = events
        .iter()
        .filter(|timed| matches!(timed.event, Event::TurnFailed { .. }))
        .map(|timed| timed.at_ms)
        .collect();
    for (index, wait) in RETRY_DELAYS.iter().enumerate() {
        let waited = starts[index + 1].at_ms - failures[index];
        let wanted = i64::try_from(daemon.scaled(*wait).as_millis()).unwrap();
        assert!(waited >= wanted - 5, "retry {index} waited {waited} ms");
    }
}

#[tokio::test]
async fn a_turn_that_said_something_or_needs_a_person_is_not_sent_again() {
    let daemon = Daemon::start_scaled(FAST);
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;
    let patience = daemon.scaled(RETRY_DELAYS[0]) * 4;

    for prompt in ["sayfail partly", "toolfail", "fail auth", "fail limit"] {
        client.say(id, prompt).await;
        assert!(
            matches!(client.until_turn_ends().await, Event::TurnFailed { .. }),
            "{prompt}"
        );
        assert!(
            !arrives_within(&mut client, patience, is_retry).await,
            "{prompt} was sent again"
        );
    }
}

#[tokio::test]
async fn a_retry_that_waits_is_dropped_by_a_stop_or_a_new_message() {
    let daemon = Daemon::start_scaled(SLOW);
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;
    let patience = daemon.scaled(RETRY_DELAYS[0]) * 2;

    client
        .say(
            id,
            &format!("failfirst 1 {} say a", marker(&daemon, "stop")),
        )
        .await;
    assert!(matches!(
        client.until_turn_ends().await,
        Event::TurnFailed { .. }
    ));
    client
        .send(ClientMessage::StopTurn {
            conversation_id: id,
        })
        .await;
    assert!(!arrives_within(&mut client, patience, is_retry).await);

    client
        .say(
            id,
            &format!("failfirst 1 {} say b", marker(&daemon, "write")),
        )
        .await;
    assert!(matches!(
        client.until_turn_ends().await,
        Event::TurnFailed { .. }
    ));
    assert_eq!(client.ask(id, "say instead").await, "instead");
    assert!(!arrives_within(&mut client, patience, is_retry).await);
}

// US-011: a pause of all new work.

fn paused(message: ServerMessage) -> Option<bool> {
    match message {
        ServerMessage::DaemonInfo(info) => Some(info.paused),
        _ => None,
    }
}

#[tokio::test]
async fn a_paused_daemon_starts_no_turn_of_its_own_and_lets_running_ones_end() {
    let mut daemon = Daemon::start_scaled(FAST);
    let mut client = daemon.client().await;
    let mut watcher = daemon.client().await;
    let id = client.new_conversation().await;
    client.open(id).await;
    let flag = daemon.data_dir().join("PAUSED");

    client.say(id, "stream 30 20").await;
    client.until_reply("w0").await;
    client.send(ClientMessage::PauseDaemon).await;
    assert!(watcher.until(paused).await);
    assert!(flag.exists());
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );

    client
        .say(
            id,
            &format!("failfirst 1 {} say a", marker(&daemon, "paused")),
        )
        .await;
    assert!(matches!(
        client.until_turn_ends().await,
        Event::TurnFailed { .. }
    ));
    let patience = daemon.scaled(RETRY_DELAYS[0]) * 4;
    assert!(!arrives_within(&mut client, patience, is_retry).await);
    // A person's message still starts a turn.
    assert_eq!(client.ask(id, "say mine").await, "mine");

    daemon.kill();
    daemon.spawn();
    let mut client = daemon.client().await;
    client.send(ClientMessage::GetDaemonInfo).await;
    assert!(client.until(paused).await);

    let mut watcher = daemon.client().await;
    client.send(ClientMessage::ResumeDaemon).await;
    assert!(!watcher.until(paused).await);
    assert!(!flag.exists());
}

#[tokio::test]
async fn a_paused_file_pauses_the_daemon_and_one_it_cannot_remove_keeps_it_paused() {
    let daemon = Daemon::start_on(|database| {
        let flag = database.parent().unwrap().join("PAUSED");
        std::fs::create_dir_all(flag.join("held")).unwrap();
    });
    let mut client = daemon.client().await;
    client.send(ClientMessage::GetDaemonInfo).await;
    assert!(client.until(paused).await);

    client.send(ClientMessage::ResumeDaemon).await;
    let error = client.until_error().await;
    let flag = daemon.data_dir().join("PAUSED");
    assert!(error.contains(&flag.display().to_string()), "{error}");
    client.send(ClientMessage::GetDaemonInfo).await;
    assert!(client.until(paused).await);
}
