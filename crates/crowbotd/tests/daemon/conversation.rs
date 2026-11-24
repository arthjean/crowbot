use std::path::Path;
use std::time::Duration;

use crowbot_protocol::{
    APPROVAL_TARGET_BYTES, Appearance, ApprovalDecision, BotPatch, BotState, ClientMessage, Event,
    FailureCause, MESSAGE_LENGTH, PermissionLevel, Preferences, PreferencesPatch, SendShortcut,
    ServerMessage, TOOL_OUTPUT_BYTES, TimedEvent, ToolKind, ToolStatus, TurnEnd,
};

use crate::support::{self, Client, Daemon, queued_texts, user_texts};

const QUIET: Duration = Duration::from_millis(400);
const PAGE: usize = 200;

async fn turn(client: &mut Client, conversation_id: i64, text: &str) -> Vec<TimedEvent> {
    client.say(conversation_id, text).await;
    let mut events = Vec::new();
    loop {
        if let ServerMessage::ConversationEvent {
            conversation_id: about,
            event,
            at_ms,
        } = client.next().await
            && about == conversation_id
        {
            let ended = event.ends_turn();
            events.push(TimedEvent {
                id: 0,
                at_ms,
                event,
            });
            if ended {
                return events;
            }
        }
    }
}

fn untimed(events: &[TimedEvent]) -> Vec<Event> {
    events.iter().map(|timed| timed.event.clone()).collect()
}

fn outputs(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ToolCallUpdated {
                output: Some(output),
                ..
            } => Some(output.as_str()),
            _ => None,
        })
        .collect()
}

async fn older_page(
    client: &mut Client,
    conversation_id: i64,
    before: i64,
) -> Result<(Vec<TimedEvent>, Option<i64>), String> {
    client
        .send(ClientMessage::LoadOlderEvents {
            conversation_id,
            before,
        })
        .await;
    client
        .until(|message| match message {
            ServerMessage::OlderEvents {
                conversation_id: about,
                events,
                older,
            } if about == conversation_id => Some(Ok((events, older))),
            ServerMessage::OlderEventsFailed {
                conversation_id: about,
                message,
            } if about == conversation_id => Some(Err(message)),
            _ => None,
        })
        .await
}

async fn requested(client: &mut Client) -> Event {
    client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
}

async fn answer(client: &mut Client, conversation_id: i64, approval: &str, allow: bool) {
    client
        .send(ClientMessage::ResolveApproval {
            conversation_id,
            approval_id: approval.to_owned(),
            decision: if allow {
                ApprovalDecision::Allowed
            } else {
                ApprovalDecision::Declined
            },
        })
        .await;
}

async fn stop(client: &mut Client, conversation_id: i64) {
    client
        .send(ClientMessage::StopTurn { conversation_id })
        .await;
}

async fn set_draft(client: &mut Client, conversation_id: i64, text: &str) {
    client
        .send(ClientMessage::SetDraft {
            conversation_id,
            text: text.to_owned(),
        })
        .await;
}

async fn preferences(client: &mut Client) -> Preferences {
    client.send(ClientMessage::GetPreferences).await;
    until_preferences(client).await
}

async fn until_preferences(client: &mut Client) -> Preferences {
    client
        .until(|message| match message {
            ServerMessage::Preferences(preferences) => Some(preferences),
            _ => None,
        })
        .await
}

#[tokio::test]
async fn a_tool_call_is_stored_with_its_kind_input_output_and_times() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let bot = client.new_bot("Atlas").await;
    let id = bot.current_conversation_id;
    client.open(id).await;

    let live = turn(&mut client, id, "work").await;
    let expected = vec![
        Event::user_text("work".to_owned()),
        Event::ThoughtText("Looking at the files.".to_owned()),
        Event::ToolCallStarted {
            id: "call-1000".to_owned(),
            title: "Read notes.txt".to_owned(),
            status: Some(ToolStatus::Pending),
            kind: Some(ToolKind::Read),
            input: Some(format!("{}/notes.txt", bot.bot.workspace)),
            effect: None,
        },
        Event::ToolCallUpdated {
            id: "call-1000".to_owned(),
            title: None,
            status: Some(ToolStatus::InProgress),
            kind: None,
            input: None,
            output: None,
        },
        Event::ToolCallUpdated {
            id: "call-1000".to_owned(),
            title: None,
            status: Some(ToolStatus::Completed),
            kind: None,
            input: None,
            output: Some("line one\nline two".to_owned()),
        },
        Event::ToolCallStarted {
            id: "call-1001".to_owned(),
            title: "Run tests".to_owned(),
            status: Some(ToolStatus::InProgress),
            kind: Some(ToolKind::Execute),
            input: Some("cargo test".to_owned()),
            effect: None,
        },
        Event::ToolCallUpdated {
            id: "call-1001".to_owned(),
            title: None,
            status: Some(ToolStatus::Failed),
            kind: None,
            input: None,
            output: Some("error: 1 test failed".to_owned()),
        },
        Event::MessageText("Done with **two** steps.".to_owned()),
        Event::TurnEnded(TurnEnd::Completed),
    ];
    assert_eq!(untimed(&live), expected);
    assert!(live.iter().all(|timed| timed.at_ms > 0));

    let snapshot = client.open(id).await;
    assert_eq!(support::events(&snapshot), expected);
    assert_eq!(snapshot.older, None);
    let (started, ended) = (snapshot.events[2].at_ms, snapshot.events[4].at_ms);
    assert!(started > 0);
    assert!(ended >= started, "{ended} < {started}");
    let (started, ended) = (snapshot.events[5].at_ms, snapshot.events[6].at_ms);
    assert!(started > 0);
    assert!(ended >= started, "{ended} < {started}");
    assert!(
        snapshot
            .events
            .windows(2)
            .all(|pair| pair[0].at_ms <= pair[1].at_ms)
    );
}

#[tokio::test]
async fn a_tool_output_over_the_cap_is_cut_to_the_cap() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;

    let over = format!("bigout {}", TOOL_OUTPUT_BYTES + 5_000);
    let live = untimed(&turn(&mut client, id, &over).await);
    let [cut] = outputs(&live)[..] else {
        panic!("expected one output, got {live:?}");
    };
    assert_eq!(cut.len(), TOOL_OUTPUT_BYTES);
    assert!(cut.bytes().all(|byte| byte == b'x'));

    let exact = format!("bigout {TOOL_OUTPUT_BYTES}");
    let live = untimed(&turn(&mut client, id, &exact).await);
    assert_eq!(outputs(&live), ["x".repeat(TOOL_OUTPUT_BYTES)]);
    let live = untimed(&turn(&mut client, id, "bigout 10").await);
    assert_eq!(outputs(&live), ["xxxxxxxxxx"]);

    let snapshot = client.open(id).await;
    let stored = support::events(&snapshot);
    let lengths: Vec<usize> = outputs(&stored).iter().map(|output| output.len()).collect();
    assert_eq!(lengths, [TOOL_OUTPUT_BYTES, TOOL_OUTPUT_BYTES, 10]);
}

const FOURTH_SCHEMA: &str = "
CREATE TABLE conversations (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    command TEXT,
    cwd TEXT NOT NULL,
    provider_session_id TEXT,
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX events_by_conversation ON events(conversation_id, id);
ALTER TABLE conversations ADD COLUMN queue_paused INTEGER NOT NULL DEFAULT 0;
CREATE TABLE queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    position INTEGER NOT NULL DEFAULT 0,
    text TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX queue_by_conversation ON queue(conversation_id, position, id);
CREATE TABLE bots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    instructions TEXT NOT NULL DEFAULT '',
    avatar_color TEXT NOT NULL,
    avatar_glyph TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT,
    permission_level TEXT NOT NULL DEFAULT 'ask_first',
    workspace TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0,
    current_conversation_id INTEGER,
    session_revision INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL
);
ALTER TABLE conversations ADD COLUMN bot_id INTEGER REFERENCES bots(id);
ALTER TABLE conversations ADD COLUMN title TEXT;
ALTER TABLE conversations ADD COLUMN session_revision INTEGER;
ALTER TABLE conversations ADD COLUMN read_event_id INTEGER NOT NULL DEFAULT 0;
CREATE INDEX conversations_by_bot ON conversations(bot_id);
CREATE TABLE preferences (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
PRAGMA user_version = 4;
";

const OLD_EVENTS: [(&str, &str); 6] = [
    (
        "user_text",
        r#"{"type":"user_text","data":"Plan the trip"}"#,
    ),
    (
        "tool_call_started",
        r#"{"type":"tool_call_started","data":{"id":"t1","title":"Read","status":"pending"}}"#,
    ),
    (
        "tool_call_updated",
        r#"{"type":"tool_call_updated","data":{"id":"t1","title":null,"status":"completed"}}"#,
    ),
    (
        "approval_requested",
        r#"{"type":"approval_requested","data":{"id":"a1","tool_call_id":"t1","title":"Write"}}"#,
    ),
    (
        "approval_resolved",
        r#"{"type":"approval_resolved","data":{"id":"a1","tool_call_id":"t1","title":"Write","decision":"allowed","reason":null}}"#,
    ),
    (
        "turn_failed",
        r#"{"type":"turn_failed","data":{"message":"gone"}}"#,
    ),
];

pub fn write_fourth_database(path: &Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch(FOURTH_SCHEMA).unwrap();
    let folder = path.parent().unwrap().to_string_lossy().into_owned();
    connection
        .execute(
            "INSERT INTO bots (name, avatar_color, avatar_glyph, provider, workspace, created_at_ms)
             VALUES ('Atlas', '#3b82f6', 'A', 'custom', ?1, 5)",
            rusqlite::params![folder],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO conversations (bot_id, provider, cwd, title, created_at_ms)
             VALUES (1, 'custom', ?1, 'Plan the trip', 6)",
            rusqlite::params![folder],
        )
        .unwrap();
    connection
        .execute_batch(
            "UPDATE bots SET current_conversation_id = 1;
             INSERT INTO preferences (key, value) VALUES ('appearance', 'dark');",
        )
        .unwrap();
    for (index, (kind, payload)) in OLD_EVENTS.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO events (conversation_id, kind, payload, created_at_ms)
                 VALUES (1, ?1, ?2, ?3)",
                rusqlite::params![kind, payload, 11 + i64::try_from(index).unwrap()],
            )
            .unwrap();
    }
}

#[tokio::test]
async fn a_version_4_database_migrates_and_its_older_events_still_load() {
    let daemon = Daemon::start_on(write_fourth_database);
    let mut client = daemon.client().await;

    let bots = client.bots().await;
    let [owner] = bots.as_slice() else {
        panic!("expected one bot, got {bots:?}");
    };
    assert_eq!(owner.bot.name, "Atlas");
    assert_eq!(owner.current_conversation_id, 1);
    assert_eq!(preferences(&mut client).await.appearance, Appearance::Dark);

    let snapshot = client.open(1).await;
    assert_eq!(
        snapshot.conversation.title.as_deref(),
        Some("Plan the trip")
    );
    assert_eq!(snapshot.draft, "");
    assert_eq!(snapshot.older, None);
    let times: Vec<i64> = snapshot.events.iter().map(|timed| timed.at_ms).collect();
    assert_eq!(times, [11, 12, 13, 14, 15, 16]);
    assert_eq!(
        support::events(&snapshot),
        [
            Event::user_text("Plan the trip".to_owned()),
            Event::ToolCallStarted {
                id: "t1".to_owned(),
                title: "Read".to_owned(),
                status: Some(ToolStatus::Pending),
                kind: None,
                input: None,
                effect: None,
            },
            Event::ToolCallUpdated {
                id: "t1".to_owned(),
                title: None,
                status: Some(ToolStatus::Completed),
                kind: None,
                input: None,
                output: None,
            },
            Event::ApprovalRequested {
                id: "a1".to_owned(),
                tool_call_id: "t1".to_owned(),
                title: Some("Write".to_owned()),
                target: None,
                folder: None,
                cut: false,
            },
            Event::ApprovalResolved {
                id: Some("a1".to_owned()),
                tool_call_id: "t1".to_owned(),
                title: Some("Write".to_owned()),
                decision: ApprovalDecision::Allowed,
                reason: None,
            },
            Event::TurnFailed {
                message: "gone".to_owned(),
                cause: None,
                provider: None,
            },
        ]
    );

    let database = rusqlite::Connection::open(daemon.database()).unwrap();
    let count = |sql: &str| -> i64 { database.query_row(sql, [], |row| row.get(0)).unwrap() };
    assert_eq!(count("PRAGMA user_version"), 14);
    assert_eq!(count("SELECT COUNT(*) FROM approval_rules"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM memory"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM skills"), 0);
    for table in [
        "harness_settings",
        "turn_refusals",
        "reviews",
        "skill_turns",
        "skill_uses",
    ] {
        assert_eq!(
            count(&format!("SELECT COUNT(*) FROM {table}")),
            0,
            "{table}"
        );
    }
    assert_eq!(count("SELECT COUNT(*) FROM skill_versions"), 0);
    // The tool call stored before the index held tool calls is found.
    assert_eq!(
        count("SELECT COUNT(*) FROM event_text WHERE event_text MATCH '\"Read\"'"),
        1
    );
    assert_eq!(count("SELECT COUNT(*) FROM events"), 6);
    assert_eq!(count("SELECT COUNT(*) FROM conversations"), 1);
    drop(database);

    set_draft(&mut client, 1, "kept after the migration").await;
    assert_eq!(client.open(1).await.draft, "kept after the migration");
    assert_eq!(client.ask(1, "say still here").await, "still here");
    let snapshot = client.open(1).await;
    assert_eq!(snapshot.events.len(), 9);
    assert_eq!(snapshot.draft, "");
}

#[tokio::test]
async fn a_long_conversation_opens_on_its_newest_page_and_older_pages_load_in_order() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    let turns = 135;
    let mut expected = Vec::new();
    for index in 0..turns {
        assert_eq!(
            client.ask(id, &format!("say n{index}")).await,
            format!("n{index}")
        );
        expected.extend([
            Event::user_text(format!("say n{index}")),
            Event::MessageText(format!("n{index}")),
            Event::TurnEnded(TurnEnd::Completed),
        ]);
    }
    assert_eq!(expected.len(), 405);

    let snapshot = client.open(id).await;
    assert_eq!(snapshot.events.len(), PAGE);
    assert_eq!(
        support::events(&snapshot),
        expected[expected.len() - PAGE..]
    );
    let mut cursor = snapshot.older;
    assert!(cursor.is_some());

    let mut pages = vec![snapshot.events];
    while let Some(before) = cursor {
        let (events, older) = older_page(&mut client, id, before).await.unwrap();
        assert!(!events.is_empty());
        pages.insert(0, events);
        cursor = older;
    }
    let sizes: Vec<usize> = pages.iter().map(Vec::len).collect();
    assert_eq!(sizes, [5, PAGE, PAGE]);
    let all = pages.concat();
    assert_eq!(untimed(&all), expected);
    assert!(all.iter().all(|timed| timed.at_ms > 0));
    assert!(all.windows(2).all(|pair| pair[0].at_ms <= pair[1].at_ms));

    let database = rusqlite::Connection::open(daemon.database()).unwrap();
    let rows: Vec<i64> = database
        .prepare("SELECT id FROM events WHERE conversation_id = ?1 ORDER BY id")
        .unwrap()
        .query_map([id], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    drop(database);
    assert_eq!(rows.len(), expected.len());
    assert_eq!(snapshot.older, Some(rows[rows.len() - PAGE]));
    let (again, older) = older_page(&mut client, id, rows[5]).await.unwrap();
    assert_eq!(untimed(&again), expected[..5]);
    assert_eq!(older, None);
    let (nothing, older) = older_page(&mut client, id, rows[0]).await.unwrap();
    assert!(nothing.is_empty());
    assert_eq!(older, None);
}

#[tokio::test]
async fn a_short_conversation_has_no_older_page() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    let empty = client.open(id).await;
    assert!(empty.events.is_empty());
    assert_eq!(empty.older, None);

    client.ask(id, "say one").await;
    let snapshot = client.open(id).await;
    assert_eq!(snapshot.events.len(), 3);
    assert_eq!(snapshot.older, None);

    let (all, older) = older_page(&mut client, id, i64::MAX).await.unwrap();
    assert_eq!(all, snapshot.events);
    assert_eq!(older, None);

    let failed = older_page(&mut client, id + 100, 1).await.unwrap_err();
    assert!(failed.contains("does not exist"), "{failed}");
}

#[tokio::test]
async fn a_draft_is_kept_until_a_message_is_accepted() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    assert_eq!(client.open(id).await.draft, "");

    let draft = "half a thought\nand a second line";
    set_draft(&mut client, id, draft).await;
    assert_eq!(client.next_within(QUIET).await, None);
    client
        .send(ClientMessage::CloseConversation {
            conversation_id: id,
        })
        .await;
    assert_eq!(client.open(id).await.draft, draft);
    let mut other = daemon.client().await;
    assert_eq!(other.open(id).await.draft, draft);

    drop((client, other));
    daemon.kill();
    daemon.spawn();
    let mut client = daemon.client().await;
    assert_eq!(client.open(id).await.draft, draft);

    client.say(id, "   ").await;
    client.until_refused().await;
    assert_eq!(client.open(id).await.draft, draft);

    client.say(id, "stream 300 20").await;
    assert_eq!(client.until_accepted().await, id);
    assert_eq!(client.open(id).await.draft, "");

    set_draft(&mut client, id, "written during the turn").await;
    assert_eq!(client.open(id).await.draft, "written during the turn");
    client.say(id, "say later").await;
    assert_eq!(client.until_accepted().await, id);
    let queued = client.open(id).await;
    assert_eq!(queued.draft, "");
    assert_eq!(queued_texts(&queued.queue), ["say later"]);

    set_draft(&mut client, id + 100, "nowhere").await;
    assert!(client.until_error().await.contains("does not exist"));
}

#[tokio::test]
async fn a_message_is_accepted_when_it_starts_a_turn_and_when_it_is_queued() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut other = daemon.client().await;
    let id = client.new_conversation().await;
    other.open(id).await;

    client.say(id, "stream 30 20").await;
    assert_eq!(client.until_accepted().await, id);
    client.until_reply("w0").await;
    client.say(id, "say two").await;
    assert_eq!(client.until_accepted().await, id);
    let queue = client.until_queue(|queue| queue.messages.len() == 1).await;
    assert_eq!(queued_texts(&queue), ["say two"]);

    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
    client.until_reply("two").await;
    client.until_turn_ends().await;
    assert_eq!(
        user_texts(&client.open(id).await.events),
        ["stream 30 20", "say two"]
    );

    let mut answers = 0;
    while let Some(message) = other.next_within(QUIET).await {
        if matches!(
            message,
            ServerMessage::MessageAccepted { .. } | ServerMessage::MessageRefused { .. }
        ) {
            answers += 1;
        }
    }
    assert_eq!(answers, 0);
}

#[tokio::test]
async fn a_message_that_is_empty_or_too_long_is_refused_and_records_nothing() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;

    let too_long = "x".repeat(MESSAGE_LENGTH + 1);
    for (text, why) in [
        ("", "A message needs some text."),
        (" \n\t ", "A message needs some text."),
        (
            too_long.as_str(),
            "A message can hold 100,000 characters at most.",
        ),
    ] {
        client.say(id, text).await;
        assert_eq!(client.until_refused().await, (id, why.to_owned()));
    }
    assert_eq!(client.next_within(QUIET).await, None);
    let snapshot = client.open(id).await;
    assert!(snapshot.events.is_empty());
    assert!(!snapshot.turn_active);
    assert!(snapshot.queue.messages.is_empty());
    assert_eq!(snapshot.conversation.title, None);

    client.say(id + 100, "say nowhere").await;
    let (about, why) = client.until_refused().await;
    assert_eq!(about, id + 100);
    assert!(why.contains("does not exist"), "{why}");

    let longest = format!("say {}", "é".repeat(MESSAGE_LENGTH - 4));
    assert_eq!(longest.chars().count(), MESSAGE_LENGTH);
    client.say(id, &longest).await;
    assert_eq!(client.until_accepted().await, id);
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
    let snapshot = client.open(id).await;
    assert_eq!(user_texts(&snapshot.events), [longest.as_str()]);
}

#[tokio::test]
async fn an_approval_request_carries_its_title_its_target_and_the_working_folder() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let bot = client.new_bot("Atlas").await;
    let id = bot.current_conversation_id;
    client.open(id).await;

    client.say(id, "ask cargo build --release").await;
    let command = requested(&mut client).await;
    let Event::ApprovalRequested { id: first, .. } = &command else {
        unreachable!()
    };
    assert_eq!(
        command,
        Event::ApprovalRequested {
            id: first.clone(),
            tool_call_id: "tool-1000".to_owned(),
            title: Some("cargo build --release".to_owned()),
            target: Some("cargo build --release".to_owned()),
            folder: Some(bot.bot.workspace.clone()),
            cut: false,
        }
    );
    answer(&mut client, id, first, false).await;
    client.until_reply("outcome=reject").await;
    client.until_turn_ends().await;

    client.say(id, "askfile src/main.rs").await;
    let file = requested(&mut client).await;
    let Event::ApprovalRequested { id: second, .. } = &file else {
        unreachable!()
    };
    assert_ne!(first, second);
    assert_eq!(
        file,
        Event::ApprovalRequested {
            id: second.clone(),
            tool_call_id: "tool-1001".to_owned(),
            title: Some("Edit src/main.rs".to_owned()),
            target: Some("src/main.rs".to_owned()),
            folder: Some(bot.bot.workspace.clone()),
            cut: false,
        }
    );
    let waiting = client.open(id).await;
    assert_eq!(waiting.pending_approvals, [second.as_str()]);
    answer(&mut client, id, second, true).await;
    client.until_reply("outcome=allow").await;
    client.until_turn_ends().await;

    let stored = support::events(&client.open(id).await);
    assert!(stored.contains(&command));
    assert!(stored.contains(&file));
}

#[tokio::test]
async fn a_cut_request_still_waits_when_the_bot_moves_to_full_access() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let bot = client.new_bot("Atlas").await;
    let id = bot.current_conversation_id;
    client.open(id).await;
    client
        .say(id, &format!("asklong {}", APPROVAL_TARGET_BYTES + 100))
        .await;
    let Event::ApprovalRequested {
        id: approval, cut, ..
    } = requested(&mut client).await
    else {
        panic!("no request");
    };
    assert!(cut);

    client
        .update_bot(
            bot.bot.id,
            BotPatch {
                permission_level: Some(PermissionLevel::FullAccess),
                ..BotPatch::default()
            },
        )
        .await;
    while let Some(message) = client.next_within(QUIET).await {
        assert!(
            !matches!(
                message,
                ServerMessage::ConversationEvent {
                    event: Event::ApprovalResolved { .. },
                    ..
                }
            ),
            "{message:?}"
        );
    }
    let waiting = client.open(id).await;
    assert!(waiting.turn_active);
    assert_eq!(waiting.pending_approvals, [approval.as_str()]);

    answer(&mut client, id, &approval, true).await;
    let (_, _, why) = client.until_not_resolved().await;
    assert!(why.contains("only be declined"), "{why}");
    answer(&mut client, id, &approval, false).await;
    client.until_reply("outcome=reject").await;
}

#[tokio::test]
async fn a_request_cut_by_the_size_cap_can_only_be_declined() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let bot = client.new_bot("Atlas").await;
    let id = bot.current_conversation_id;
    client.open(id).await;

    client
        .say(id, &format!("asklong {}", APPROVAL_TARGET_BYTES + 100))
        .await;
    let Event::ApprovalRequested {
        id: approval,
        title,
        target: Some(target),
        folder,
        cut,
        ..
    } = requested(&mut client).await
    else {
        panic!("the request has no target");
    };
    assert!(cut);
    assert_eq!(target.len(), APPROVAL_TARGET_BYTES);
    assert!(target.bytes().all(|byte| byte == b'x'));
    assert_eq!(title.as_deref(), Some("Run a long command"));
    assert_eq!(folder.as_deref(), Some(bot.bot.workspace.as_str()));

    answer(&mut client, id, &approval, true).await;
    let (about, refused, why) = client.until_not_resolved().await;
    assert_eq!((about, refused.as_str()), (id, approval.as_str()));
    assert!(why.contains("only be declined"), "{why}");
    assert_eq!(client.next_within(QUIET).await, None);
    let waiting = client.open(id).await;
    assert!(waiting.turn_active);
    assert_eq!(waiting.pending_approvals, [approval.as_str()]);
    assert_eq!(client.bot(bot.bot.id).await.state, BotState::Waiting);

    answer(&mut client, id, &approval, false).await;
    let resolved = client
        .until_event(|event| matches!(event, Event::ApprovalResolved { .. }))
        .await;
    assert_eq!(
        resolved,
        Event::ApprovalResolved {
            id: Some(approval),
            tool_call_id: "tool-1000".to_owned(),
            title,
            decision: ApprovalDecision::Declined,
            reason: None,
        }
    );
    client.until_reply("outcome=reject").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );

    client
        .say(id, &format!("asklong {APPROVAL_TARGET_BYTES}"))
        .await;
    let Event::ApprovalRequested {
        id: whole,
        target: Some(target),
        cut,
        ..
    } = requested(&mut client).await
    else {
        panic!("the request has no target");
    };
    assert!(!cut);
    assert_eq!(target.len(), APPROVAL_TARGET_BYTES);
    answer(&mut client, id, &whole, true).await;
    client.until_reply("outcome=allow").await;
}

#[tokio::test]
async fn an_answer_to_an_unknown_or_already_answered_approval_is_refused() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;

    answer(&mut client, id, "approval-0", true).await;
    let (about, refused, why) = client.until_not_resolved().await;
    assert_eq!((about, refused.as_str()), (id, "approval-0"));
    assert!(why.contains("no longer waiting"), "{why}");

    client.say(id, "ask Write notes.txt").await;
    let Event::ApprovalRequested { id: approval, .. } = requested(&mut client).await else {
        unreachable!()
    };
    answer(&mut client, id, "approval-0", false).await;
    let (_, refused, _) = client.until_not_resolved().await;
    assert_eq!(refused, "approval-0");
    assert_eq!(client.open(id).await.pending_approvals, [approval.as_str()]);

    answer(&mut client, id, &approval, true).await;
    client.until_reply("outcome=allow").await;
    client.until_turn_ends().await;
    for allow in [true, false] {
        answer(&mut client, id, &approval, allow).await;
        let (about, refused, why) = client.until_not_resolved().await;
        assert_eq!((about, refused.as_str()), (id, approval.as_str()));
        assert!(why.contains("no longer waiting"), "{why}");
    }

    answer(&mut client, id + 100, &approval, true).await;
    let (about, refused, why) = client.until_not_resolved().await;
    assert_eq!((about, refused.as_str()), (id + 100, approval.as_str()));
    assert!(why.contains("does not exist"), "{why}");

    let stored = support::events(&client.open(id).await);
    let resolved = stored
        .iter()
        .filter(|event| matches!(event, Event::ApprovalResolved { .. }))
        .count();
    assert_eq!(resolved, 1);
}

#[tokio::test]
async fn a_failed_turn_tells_its_cause_and_its_provider() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let bot = client.new_bot("Atlas").await;
    assert_eq!(bot.bot.provider, "custom");
    let id = bot.current_conversation_id;
    client.open(id).await;

    let mut failures = Vec::new();
    for (command, message, cause) in [
        (
            "fail auth",
            "Authentication required",
            Some(FailureCause::SignInRequired),
        ),
        (
            "fail limit",
            "Internal error",
            Some(FailureCause::UsageLimit),
        ),
        ("fail other", "Internal error", None),
    ] {
        client.say(id, command).await;
        let failed = client.until_turn_ends().await;
        assert_eq!(
            failed,
            Event::TurnFailed {
                message: message.to_owned(),
                cause,
                provider: Some("custom".to_owned()),
            },
            "{command}"
        );
        failures.push(failed);
    }

    client.say(id, "crash").await;
    let Event::TurnFailed {
        cause, provider, ..
    } = client.until_turn_ends().await
    else {
        panic!("the turn did not fail");
    };
    assert_eq!(cause, None);
    assert_eq!(provider.as_deref(), Some("custom"));

    let stored = support::events(&client.open(id).await);
    for failed in &failures {
        assert!(stored.contains(failed), "{failed:?}");
    }
    assert_eq!(client.ask(id, "say back").await, "back");
}

#[tokio::test]
async fn a_tool_call_stopped_before_it_ends_has_no_final_status() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;

    client.say(id, "hang sleep 60").await;
    let started = client
        .until_event(|event| matches!(event, Event::ToolCallStarted { .. }))
        .await;
    assert_eq!(
        started,
        Event::ToolCallStarted {
            id: "call-1000".to_owned(),
            title: "sleep 60".to_owned(),
            status: Some(ToolStatus::InProgress),
            kind: Some(ToolKind::Execute),
            input: Some("sleep 60".to_owned()),
            effect: None,
        }
    );
    assert_eq!(client.next_within(QUIET).await, None);

    stop(&mut client, id).await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Cancelled)
    );
    let snapshot = client.open(id).await;
    assert!(!snapshot.turn_active);
    assert_eq!(
        support::events(&snapshot),
        [
            Event::user_text("hang sleep 60".to_owned()),
            started,
            Event::TurnEnded(TurnEnd::Cancelled),
        ]
    );
}

#[tokio::test]
async fn a_turn_stopped_while_thinking_keeps_the_thought_and_has_no_message() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;

    client.say(id, "ponder Weighing the options.").await;
    let thought = client
        .until_event(|event| matches!(event, Event::ThoughtText(_)))
        .await;
    assert_eq!(
        thought,
        Event::ThoughtText("Weighing the options.".to_owned())
    );
    stop(&mut client, id).await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Cancelled)
    );
    assert_eq!(
        support::events(&client.open(id).await),
        [
            Event::user_text("ponder Weighing the options.".to_owned()),
            thought,
            Event::TurnEnded(TurnEnd::Cancelled),
        ]
    );
}

#[tokio::test]
async fn the_send_shortcut_reaches_every_client_and_outlives_the_daemon() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut other = daemon.client().await;
    assert_eq!(
        preferences(&mut client).await.send_shortcut,
        SendShortcut::Enter
    );

    client
        .send(ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                send_shortcut: Some(SendShortcut::ModEnter),
                ..PreferencesPatch::default()
            },
        })
        .await;
    let expected = Preferences {
        send_shortcut: SendShortcut::ModEnter,
        ..Preferences::default()
    };
    assert_eq!(until_preferences(&mut client).await, expected);
    assert_eq!(until_preferences(&mut other).await, expected);
    assert_eq!(preferences(&mut other).await, expected);

    drop((client, other));
    daemon.kill();
    daemon.spawn();
    let mut later = daemon.client().await;
    assert_eq!(preferences(&mut later).await, expected);

    later
        .send(ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                send_shortcut: Some(SendShortcut::Enter),
                ..PreferencesPatch::default()
            },
        })
        .await;
    assert_eq!(until_preferences(&mut later).await, Preferences::default());
}

#[tokio::test]
async fn the_sample_answer_streams_every_kind_of_block_and_stops_on_a_cancel() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;

    let sample = client.ask(id, "sample 0").await;
    for block in [
        "## Release notes\n\n",
        "**bold**",
        "*italic*",
        "~~struck~~",
        "`inline code`",
        "[link](https://example.com/docs)",
        "- First item\n- Second item\n- Third item\n",
        "1. Step one\n2. Step two\n",
        "- [x] Done task\n- [ ] Open task\n",
        "> A quote that spans\n> two lines.\n",
        "| Name | Kind | Count |\n| --- | --- | ---: |\n| alpha | read | 1 |\n| beta | edit | 22 |\n",
        "\n---\n",
        "```rust\nfn main() {\n    let words = [\"caw\", \"caw\"];\n    for word in words {\n        println!(\"{word}\");\n    }\n}\n```\n",
    ] {
        assert!(sample.contains(block), "{block:?} is missing in {sample:?}");
    }
    assert!(sample.ends_with("Done."));

    client.say(id, "sample 50").await;
    client.until_reply("Release").await;
    stop(&mut client, id).await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Cancelled)
    );
    let snapshot = client.open(id).await;
    let stopped: String = support::events(&snapshot)
        .iter()
        .rev()
        .take_while(|event| !matches!(event, Event::UserText(_)))
        .filter_map(|event| match event {
            Event::MessageText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(sample.starts_with(&stopped), "{stopped:?}");
    assert!(stopped.len() < sample.len());
}
