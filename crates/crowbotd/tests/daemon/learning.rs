//! EP-006 of the harness PRD: learning from use, through the real daemon. A
//! person sets what the daemon may start on its own for a bot; the daemon
//! refuses a turn whose switch is off or whose daily limit is reached, asks
//! for a summary after a turn over budget, reviews the bot's work in a
//! conversation of its own, and counts the turns that loaded a skill. The
//! scripted agent plays the review with the command of the file `review`
//! (`FAKE_AGENT_REVIEW`).

use std::time::{Duration, Instant};

use crowbot_protocol::{
    ApprovalReason, BotPatch, BotSkillFigures, ClientMessage, Consumption, Event, Figure,
    HarnessSettings, Interruption, MemoryAuthor, OriginConsumption, ServerMessage, SkillAuthor,
    SkillChangeKind, SkillFigures, TurnEnd, TurnOrigin, TurnRefusal, TurnSwitches, UserMessage,
};
use serde_json::json;

use crate::support::{self, Client, Daemon};

#[path = "../../src/review/limits.rs"]
mod limits;

use limits::{REVIEW_OPENING, REVIEW_TOOL_CALLS, REVIEW_USER_TURNS};

/// A daemon whose guards wait a hundred times less: a retry waits 150 ms.
const FAST: u32 = 100;
/// How long a test waits for something that must not happen.
const QUIET: Duration = Duration::from_secs(3);

async fn bot(client: &mut Client, name: &str) -> (i64, i64) {
    let bot = client.new_bot(name).await;
    client.open(bot.current_conversation_id).await;
    (bot.bot.id, bot.current_conversation_id)
}

async fn settings(client: &mut Client, bot_id: i64) -> HarnessSettings {
    client
        .send(ClientMessage::GetHarnessSettings { bot_id })
        .await;
    until_settings(client, bot_id).await
}

async fn until_settings(client: &mut Client, bot_id: i64) -> HarnessSettings {
    client
        .until(|message| match message {
            ServerMessage::HarnessSettings {
                bot_id: about,
                settings,
            } if about == bot_id => Some(settings),
            ServerMessage::Error { message } => panic!("refused: {message}"),
            _ => None,
        })
        .await
}

/// Sets the settings and waits until the daemon told the client.
async fn allow(client: &mut Client, bot_id: i64, settings: HarnessSettings) {
    client
        .send(ClientMessage::SetHarnessSettings { bot_id, settings })
        .await;
    assert_eq!(until_settings(client, bot_id).await, settings);
}

/// The switches of a bot with `origins` on, and retries too.
fn with(origins: &[TurnOrigin], daily_turn_limit: u32) -> HarnessSettings {
    let mut switches = TurnSwitches::default();
    for origin in origins {
        match origin {
            TurnOrigin::Routine => switches.routine = true,
            TurnOrigin::Review => switches.review = true,
            TurnOrigin::Goal => switches.goal = true,
            TurnOrigin::Delegation => switches.delegation = true,
            TurnOrigin::Budget => switches.budget = true,
            TurnOrigin::Retry | TurnOrigin::User => {}
        }
    }
    HarnessSettings {
        switches,
        daily_turn_limit: Some(daily_turn_limit),
        ..HarnessSettings::default()
    }
}

async fn consumption(client: &mut Client, bot_id: i64) -> Consumption {
    client.send(ClientMessage::GetConsumption { bot_id }).await;
    client
        .until(|message| match message {
            ServerMessage::Consumption(consumption) if consumption.bot_id == bot_id => {
                Some(consumption)
            }
            ServerMessage::Error { message } => panic!("refused: {message}"),
            _ => None,
        })
        .await
}

fn of(consumption: &Consumption, origin: TurnOrigin) -> OriginConsumption {
    consumption
        .origins
        .iter()
        .find(|entry| entry.origin == origin)
        .cloned()
        .unwrap()
}

/// The refusals the database holds for a bot: origin and cause.
fn refusals(daemon: &Daemon, bot_id: i64) -> Vec<(String, String)> {
    let database = rusqlite::Connection::open(daemon.database()).unwrap();
    let mut statement = database
        .prepare("SELECT origin, cause FROM turn_refusals WHERE bot_id = ?1 ORDER BY id")
        .unwrap();
    statement
        .query_map([bot_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Whether an event of the conversation that matches reaches the client
/// within `patience`.
async fn arrives_within(
    client: &mut Client,
    conversation: i64,
    patience: Duration,
    matches: impl Fn(&Event) -> bool,
) -> Option<Event> {
    let deadline = Instant::now() + patience;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match client.next_within(left).await {
            Some(ServerMessage::ConversationEvent {
                conversation_id,
                event,
                ..
            }) if conversation_id == conversation && matches(&event) => return Some(event),
            Some(_) => {}
            None => return None,
        }
    }
    None
}

fn origin_of(event: &Event) -> Option<TurnOrigin> {
    match event {
        Event::UserText(UserMessage { origin, .. }) => Some(*origin),
        _ => None,
    }
}

/// `count` tool calls that all differ, so that no guard stops them.
fn calls(count: u32) -> String {
    let outputs: Vec<String> = (1..=count).map(|index| index.to_string()).collect();
    format!("calls {count} a,b,c,d,e {}", outputs.join(","))
}

/// The conversation of a review the daemon started for the bot.
async fn until_review_starts(client: &mut Client, bot_id: i64, known: &[i64]) -> i64 {
    client
        .until(|message| match message {
            ServerMessage::ConversationChanged(summary)
                if summary.bot_id == bot_id && !known.contains(&summary.id) =>
            {
                Some(summary.id)
            }
            _ => None,
        })
        .await
}

/// The events of a conversation up to the end of its first turn: those
/// stored when the client opens it, then those that follow.
async fn first_turn(client: &mut Client, conversation: i64) -> Vec<Event> {
    let mut events = support::events(&client.open(conversation).await);
    while !events.iter().any(Event::ends_turn) {
        if let ServerMessage::ConversationEvent {
            conversation_id,
            event,
            ..
        } = client.next().await
            && conversation_id == conversation
        {
            events.push(event);
        }
    }
    events
}

fn reply_of(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Event::MessageText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn review_script(daemon: &Daemon, command: &str) {
    std::fs::write(daemon.harness_file("review"), command).unwrap();
}

fn skill_document(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: How to {name} the site.\n---\n# {name}\n\n1. Run make {name}.\n"
    )
}

// US-026: switches and limits for daemon-started turns.

#[tokio::test]
async fn a_new_bot_retries_only_and_a_switch_waits_for_a_daily_limit() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut watcher = daemon.client().await;
    let (bot_id, _) = bot(&mut client, "Careful").await;

    let read = settings(&mut client, bot_id).await;
    assert_eq!(read, HarnessSettings::default());
    assert!(read.switches.retry);
    for origin in [
        TurnOrigin::Routine,
        TurnOrigin::Review,
        TurnOrigin::Goal,
        TurnOrigin::Delegation,
        TurnOrigin::Budget,
    ] {
        assert!(!read.switches.allows(origin), "{origin:?}");
    }
    assert!(!read.verification_reminder);

    // A switch other than retries waits for a daily limit of turns.
    let mut review = with(&[TurnOrigin::Review], 3);
    review.daily_turn_limit = None;
    client
        .send(ClientMessage::SetHarnessSettings {
            bot_id,
            settings: review,
        })
        .await;
    // The answer names the limit it asks for.
    assert_eq!(client.until_invalid().await, "daily_turn_limit");
    assert_eq!(
        settings(&mut client, bot_id).await,
        HarnessSettings::default()
    );

    // A limit of zero is no limit a turn could fit in.
    let mut zero = with(&[], 3);
    zero.daily_turn_limit = Some(0);
    client
        .send(ClientMessage::SetHarnessSettings {
            bot_id,
            settings: zero,
        })
        .await;
    assert_eq!(client.until_invalid().await, "daily_turn_limit");

    // Every attached client receives the settings that were set.
    let set = HarnessSettings {
        daily_token_limit: Some(50_000),
        verification_reminder: true,
        ..with(&[TurnOrigin::Review, TurnOrigin::Budget], 3)
    };
    allow(&mut client, bot_id, set).await;
    assert_eq!(until_settings(&mut watcher, bot_id).await, set);
    assert_eq!(settings(&mut client, bot_id).await, set);
}

#[tokio::test]
async fn a_turn_over_budget_asks_for_a_summary_only_when_the_switch_is_on() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, id) = bot(&mut client, "Summer").await;
    client
        .update_bot(
            bot_id,
            BotPatch {
                tool_call_limit: Some(Some(2)),
                ..BotPatch::default()
            },
        )
        .await;
    client
        .until_bot(|summary| summary.bot.tool_call_limit == Some(2))
        .await;

    // With the switch off, the turn ends and nothing follows.
    client.say(id, &calls(4)).await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnInterrupted {
            cause: Interruption::BudgetExhausted
        }
    );
    assert!(
        arrives_within(&mut client, id, QUIET, |event| origin_of(event).is_some())
            .await
            .is_none(),
        "a message followed a turn over budget with the switch off"
    );
    assert_eq!(
        refusals(&daemon, bot_id),
        [("budget".to_owned(), "switch_off".to_owned())]
    );

    // With the switch on, one message of origin `Budget` asks for a summary.
    allow(&mut client, bot_id, with(&[TurnOrigin::Budget], 5)).await;
    client.say(id, &calls(4)).await;
    let asked = client
        .until_event(|event| origin_of(event) == Some(TurnOrigin::Budget))
        .await;
    let Event::UserText(asked) = asked else {
        unreachable!()
    };
    assert!(
        asked.text.contains("Summarize the work done"),
        "{}",
        asked.text
    );
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
    let budget = of(&consumption(&mut client, bot_id).await, TurnOrigin::Budget);
    assert_eq!((budget.turns, budget.refused), (1, 1));
}

#[tokio::test]
async fn the_daily_limits_stop_the_daemon_s_turns_and_never_a_person_s() {
    let daemon = Daemon::start_scaled(FAST);
    let mut client = daemon.client().await;
    let (bot_id, id) = bot(&mut client, "Thrifty").await;
    let counter = |name: &str| daemon.folder("markers").join(name).display().to_string();

    // A turn of an origin that is off does not start, and the record names
    // the origin and the cause.
    let mut off = HarnessSettings::default();
    off.switches.retry = false;
    allow(&mut client, bot_id, off).await;
    client
        .say(id, &format!("failfirst 1 {} say again", counter("off")))
        .await;
    assert!(matches!(
        client.until_turn_ends().await,
        Event::TurnFailed { .. }
    ));
    assert!(
        arrives_within(&mut client, id, QUIET, |event| origin_of(event).is_some())
            .await
            .is_none(),
        "a retry started with its switch off"
    );
    assert_eq!(
        refusals(&daemon, bot_id),
        [("retry".to_owned(), "switch_off".to_owned())]
    );

    // A token limit counts the tokens the agents of daemon turns reported;
    // a turn whose agent reports none counts toward the turns alone.
    let limited = HarnessSettings {
        daily_token_limit: Some(300),
        ..with(&[], 3)
    };
    allow(&mut client, bot_id, limited).await;
    client
        .say(
            id,
            &format!("failfirst 1 {} say no tokens", counter("plain")),
        )
        .await;
    client
        .until_event(|event| origin_of(event) == Some(TurnOrigin::Retry))
        .await;
    client.until_reply("no tokens").await;
    client
        .say(
            id,
            &format!("failfirst 1 {} spend - - - 100 200", counter("spent")),
        )
        .await;
    client
        .until_event(|event| origin_of(event) == Some(TurnOrigin::Retry))
        .await;
    client.until_reply("spent").await;
    client.until_turn_ends().await;
    client
        .say(id, &format!("failfirst 1 {} say over", counter("tokens")))
        .await;
    assert_eq!(
        client
            .until_event(|event| matches!(event, Event::TurnNotStarted { .. }))
            .await,
        Event::TurnNotStarted {
            origin: TurnOrigin::Retry,
            cause: TurnRefusal::TokenLimit
        }
    );
    let retry = of(&consumption(&mut client, bot_id).await, TurnOrigin::Retry);
    assert_eq!(
        (retry.turns, retry.turns_with_tokens, retry.total_tokens),
        (2, 1, 300)
    );

    // A limit of turns stops the next daemon turn of the day.
    allow(&mut client, bot_id, with(&[], 2)).await;
    client
        .say(id, &format!("failfirst 1 {} say over", counter("turns")))
        .await;
    assert_eq!(
        client
            .until_event(|event| matches!(event, Event::TurnNotStarted { .. }))
            .await,
        Event::TurnNotStarted {
            origin: TurnOrigin::Retry,
            cause: TurnRefusal::TurnLimit
        }
    );

    // A person's turn starts whatever the switches and the limits.
    assert_eq!(client.ask(id, "say mine").await, "mine");
    let day = consumption(&mut client, bot_id).await;
    let user = of(&day, TurnOrigin::User);
    assert_eq!(user.turns, 6);
    assert_eq!(of(&day, TurnOrigin::Retry).refused, 3);
    assert_eq!(day.day.len(), "2026-11-24".len());
    assert_eq!(
        refusals(&daemon, bot_id)[1..],
        [
            ("retry".to_owned(), "token_limit".to_owned()),
            ("retry".to_owned(), "turn_limit".to_owned())
        ]
    );
}

/// Every name the JSON schema `schema` gives a property, at any depth.
fn property_names(schema: &serde_json::Value, names: &mut Vec<String>) {
    match schema {
        serde_json::Value::Object(fields) => {
            if let Some(serde_json::Value::Object(properties)) = fields.get("properties") {
                names.extend(properties.keys().cloned());
            }
            for value in fields.values() {
                property_names(value, names);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                property_names(value, names);
            }
        }
        _ => {}
    }
}

fn listed_tools(answer: &str) -> Vec<serde_json::Value> {
    let answer = answer.strip_prefix("answer=").unwrap_or(answer);
    let listed: serde_json::Value = serde_json::from_str(answer).unwrap();
    listed["result"]["tools"].as_array().unwrap().clone()
}

#[tokio::test]
async fn no_daemon_tool_takes_a_switch_or_a_limit() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, id) = bot(&mut client, "Lister").await;
    let tools = listed_tools(&client.ask(id, "mcp tools/list {}").await);
    assert!(!tools.is_empty());
    let mut names = Vec::new();
    for tool in &tools {
        property_names(&tool["inputSchema"], &mut names);
    }
    let settings = serde_json::to_value(HarnessSettings::default()).unwrap();
    let mut forbidden: Vec<String> = settings.as_object().unwrap().keys().cloned().collect();
    forbidden.extend(settings["switches"].as_object().unwrap().keys().cloned());
    forbidden.extend(["origin", "tool_call_limit", "permission_level"].map(str::to_owned));
    for name in &names {
        assert!(!forbidden.contains(name), "a tool takes {name}");
    }
}

// US-027: the review conversation.

#[tokio::test]
async fn a_review_after_ten_tool_calls_writes_a_skill_in_the_open() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, work) = bot(&mut client, "Learner").await;
    let operations = json!({ "operations": [
        { "op": "create", "name": "deploy", "content": skill_document("deploy") }
    ] });
    review_script(&daemon, &format!("mcpcall skill_manage {operations}"));
    allow(&mut client, bot_id, with(&[TurnOrigin::Review], 5)).await;

    // Nine calls are not enough.
    assert_eq!(
        client.ask(work, &calls(REVIEW_TOOL_CALLS - 1)).await,
        "calls done"
    );
    assert!(
        arrives_within(&mut client, work, QUIET, |event| matches!(
            event,
            Event::ReviewApplied { .. }
        ))
        .await
        .is_none()
    );
    assert_eq!(client.conversations(bot_id).await.len(), 1);

    // The tenth starts a review once the turn ended.
    assert_eq!(client.ask(work, &calls(1)).await, "calls done");
    let review = until_review_starts(&mut client, bot_id, &[work]).await;
    let applied = client
        .until_event(|event| matches!(event, Event::ReviewApplied { .. }))
        .await;
    let Event::ReviewApplied {
        review_conversation_id,
        memory,
        skills,
    } = applied
    else {
        unreachable!()
    };
    assert_eq!(review_conversation_id, review);
    assert!(memory.is_empty());
    assert_eq!(skills.len(), 1);
    assert_eq!(
        (skills[0].name.as_str(), skills[0].kind),
        ("deploy", SkillChangeKind::Created)
    );

    // The review is a conversation a person reads: its turn has the origin
    // `Review`, and its change is the review's.
    let events = support::events(&client.open(review).await);
    let Some(Event::UserText(asked)) = events.first() else {
        panic!("{events:?}")
    };
    assert_eq!(asked.origin, TurnOrigin::Review);
    assert!(asked.text.starts_with(REVIEW_OPENING), "{}", asked.text);
    assert!(asked.text.contains("Skills:"), "{}", asked.text);
    assert!(asked.text.contains("(used a tool: Read)"), "{}", asked.text);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::SkillChanged {
            author: SkillAuthor::Review,
            ..
        }
    )));
    assert_eq!(events.last(), Some(&Event::TurnEnded(TurnEnd::Completed)));
    // It is not the conversation the bot opens on, and it leaves the bot read.
    let summary = client.bot(bot_id).await;
    assert_eq!(summary.current_conversation_id, work);

    // A person cannot write in a review.
    client.say(review, "say hello").await;
    let (_, refused) = client.until_refused().await;
    assert!(refused.contains("review"), "{refused}");

    // The counter starts again after the review.
    assert_eq!(
        client.ask(work, &calls(REVIEW_TOOL_CALLS - 1)).await,
        "calls done"
    );
    assert!(
        arrives_within(&mut client, work, QUIET, |event| matches!(
            event,
            Event::ReviewApplied { .. }
        ))
        .await
        .is_none()
    );
    assert_eq!(
        of(&consumption(&mut client, bot_id).await, TurnOrigin::Review).turns,
        1
    );
}

#[tokio::test]
async fn a_bot_with_reviews_off_runs_none_and_the_refusal_is_recorded() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, work) = bot(&mut client, "Quiet").await;
    assert_eq!(
        client.ask(work, &calls(REVIEW_TOOL_CALLS + 2)).await,
        "calls done"
    );
    assert_eq!(client.ask(work, &calls(2)).await, "calls done");
    tokio::time::sleep(QUIET).await;
    assert_eq!(client.conversations(bot_id).await.len(), 1);
    // A review that comes due at the end of every turn is recorded once a day.
    assert_eq!(
        refusals(&daemon, bot_id),
        [("review".to_owned(), "switch_off".to_owned())]
    );
}

#[tokio::test]
async fn a_review_holds_the_daemon_s_tools_alone_and_declines_any_other() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, work) = bot(&mut client, "Auditor").await;
    allow(&mut client, bot_id, with(&[TurnOrigin::Review], 5)).await;

    review_script(&daemon, "mcp tools/list {}");
    client.ask(work, &calls(REVIEW_TOOL_CALLS)).await;
    let first = until_review_starts(&mut client, bot_id, &[work]).await;
    let tools = listed_tools(&reply_of(&first_turn(&mut client, first).await));
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for name in [
        "memory",
        "skills_list",
        "skill_view",
        "skill_manage",
        "session_search",
    ] {
        assert!(names.contains(&name), "{name} is missing from {names:?}");
    }
    // `probe_external`, a test tool of every kind, reaches outside the bot.
    assert!(!names.contains(&"probe_external"), "{names:?}");
    assert!(names.contains(&"probe_review"), "{names:?}");
    for tool in &tools {
        assert_eq!(tool["annotations"]["openWorldHint"], false, "{tool}");
    }

    // An agent's own tool is declined, with the reason.
    review_script(&daemon, "ask touch reviewed.txt");
    client.ask(work, &calls(REVIEW_TOOL_CALLS)).await;
    let second = until_review_starts(&mut client, bot_id, &[work, first]).await;
    let events = first_turn(&mut client, second).await;
    let declined: Vec<&Event> = events
        .iter()
        .filter(|event| matches!(event, Event::ApprovalResolved { .. }))
        .collect();
    assert!(
        matches!(
            declined[..],
            [Event::ApprovalResolved {
                reason: Some(ApprovalReason::ReviewSession),
                ..
            }]
        ),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ApprovalRequested { .. }))
    );
}

#[tokio::test]
async fn a_person_s_message_stops_a_review_and_their_turn_starts() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, work) = bot(&mut client, "Polite").await;
    allow(&mut client, bot_id, with(&[TurnOrigin::Review], 5)).await;
    review_script(&daemon, "ponder what to keep");
    client.ask(work, &calls(REVIEW_TOOL_CALLS)).await;
    let review = until_review_starts(&mut client, bot_id, &[work]).await;
    let opened = support::events(&client.open(review).await);
    if !opened
        .iter()
        .any(|event| matches!(event, Event::ThoughtText(_)))
    {
        client
            .until_event(|event| matches!(event, Event::ThoughtText(_)))
            .await;
    }
    // The review ponders; the next one answers at once.
    review_script(&daemon, "say Nothing to keep.");

    client.say(work, "say I am here").await;
    let mut review_ended = None;
    let mut reply = String::new();
    while review_ended.is_none() || !reply.contains("I am here") {
        let message = client.next().await;
        if let ServerMessage::ConversationEvent {
            conversation_id,
            event,
            ..
        } = message
        {
            if conversation_id == review && event.ends_turn() {
                review_ended = Some(event);
            } else if conversation_id == work
                && let Event::MessageText(text) = event
            {
                reply.push_str(&text);
            }
        }
    }
    assert_eq!(review_ended, Some(Event::TurnEnded(TurnEnd::Cancelled)));

    // The stopped review read nothing: the next one, once no turn of the
    // bot runs, is given the same work.
    let known = [work, review];
    let second = match new_conversation_within(&mut client, bot_id, &known, QUIET).await {
        Some(second) => second,
        None => {
            client.ask(work, "say once more").await;
            until_review_starts(&mut client, bot_id, &known).await
        }
    };
    let events = first_turn(&mut client, second).await;
    let Some(Event::UserText(asked)) = events.first() else {
        panic!("{events:?}")
    };
    assert_eq!(asked.origin, TurnOrigin::Review);
    assert!(asked.text.contains("(used a tool: "), "{}", asked.text);
    assert_eq!(events.last(), Some(&Event::TurnEnded(TurnEnd::Completed)));
}

/// The conversation the daemon creates for the bot within `patience`.
async fn new_conversation_within(
    client: &mut Client,
    bot_id: i64,
    known: &[i64],
    patience: Duration,
) -> Option<i64> {
    let deadline = Instant::now() + patience;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match client.next_within(left).await {
            Some(ServerMessage::ConversationChanged(summary))
                if summary.bot_id == bot_id && !known.contains(&summary.id) =>
            {
                return Some(summary.id);
            }
            Some(_) => {}
            None => return None,
        }
    }
    None
}

#[tokio::test]
async fn a_review_waits_for_a_failed_turn_to_be_sent_again() {
    let daemon = Daemon::start_scaled(FAST);
    let mut client = daemon.client().await;
    let (bot_id, work) = bot(&mut client, "Patient").await;
    client.ask(work, &calls(REVIEW_TOOL_CALLS)).await;
    allow(&mut client, bot_id, with(&[TurnOrigin::Review], 5)).await;
    let marker = daemon.folder("markers").join("retry");

    // The turn fails before it said anything, so it is sent again: no
    // review starts while it waits, only once it ended.
    client
        .say(work, &format!("failfirst 1 {} say again", marker.display()))
        .await;
    let mut retried = false;
    let mut retry_ended = false;
    loop {
        match client.next().await {
            ServerMessage::ConversationChanged(summary)
                if summary.bot_id == bot_id && summary.id != work =>
            {
                assert!(retry_ended, "a review started while a retry waited");
                break;
            }
            ServerMessage::ConversationEvent {
                conversation_id,
                event,
                ..
            } if conversation_id == work => {
                if origin_of(&event) == Some(TurnOrigin::Retry) {
                    retried = true;
                } else if retried && event.ends_turn() {
                    assert_eq!(event, Event::TurnEnded(TurnEnd::Completed));
                    retry_ended = true;
                }
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn a_review_over_budget_asks_for_no_summary() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, work) = bot(&mut client, "Brief").await;
    // The work runs before the bot has a limit of tool calls.
    client.ask(work, &calls(REVIEW_TOOL_CALLS)).await;
    client
        .update_bot(
            bot_id,
            BotPatch {
                tool_call_limit: Some(Some(2)),
                ..BotPatch::default()
            },
        )
        .await;
    client
        .until_bot(|summary| summary.bot.tool_call_limit == Some(2))
        .await;
    review_script(&daemon, &calls(4));
    allow(
        &mut client,
        bot_id,
        with(&[TurnOrigin::Review, TurnOrigin::Budget], 5),
    )
    .await;

    client.ask(work, "say go").await;
    let review = until_review_starts(&mut client, bot_id, &[work]).await;
    let events = first_turn(&mut client, review).await;
    assert_eq!(
        events.last(),
        Some(&Event::TurnInterrupted {
            cause: Interruption::BudgetExhausted
        })
    );
    // A review ends where its budget ends: no summary follows in it.
    assert!(
        arrives_within(&mut client, review, QUIET, |event| origin_of(event)
            .is_some())
        .await
        .is_none(),
        "a message followed a review over budget"
    );
    assert_eq!(
        of(&consumption(&mut client, bot_id).await, TurnOrigin::Budget),
        OriginConsumption {
            origin: TurnOrigin::Budget,
            turns: 0,
            refused: 0,
            turns_with_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            costs: Vec::new(),
        }
    );
}

#[tokio::test]
async fn ten_turns_of_a_person_start_a_review_that_may_write_memory() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, work) = bot(&mut client, "Rememberer").await;
    allow(&mut client, bot_id, with(&[TurnOrigin::Review], 5)).await;
    let operations = json!({ "operations": [
        { "op": "add", "scope": "bot", "text": "The user deploys on Fridays.", "core": false }
    ] });
    review_script(&daemon, &format!("mcpcall memory {operations}"));
    for turn in 0..REVIEW_USER_TURNS {
        assert_eq!(
            client.ask(work, &format!("say {turn}")).await,
            turn.to_string()
        );
    }
    let review = until_review_starts(&mut client, bot_id, &[work]).await;
    let applied = client
        .until_event(|event| matches!(event, Event::ReviewApplied { .. }))
        .await;
    let Event::ReviewApplied {
        review_conversation_id,
        memory,
        skills,
    } = applied
    else {
        unreachable!()
    };
    assert_eq!(review_conversation_id, review);
    assert!(skills.is_empty());
    assert_eq!(memory[0].text, "The user deploys on Fridays.");
    let events = support::events(&client.open(review).await);
    let Some(Event::UserText(asked)) = events.first() else {
        panic!("{events:?}")
    };
    assert!(asked.text.contains("Memory:"), "{}", asked.text);
    assert!(!asked.text.contains("Skills:"), "{}", asked.text);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::MemoryChanged {
            author: MemoryAuthor::Review,
            ..
        }
    )));
}

// US-029: what skills change.

async fn skill_figures(
    client: &mut Client,
    bot_id: i64,
    name: &str,
) -> Result<SkillFigures, String> {
    client
        .send(ClientMessage::GetSkillFigures {
            bot_id,
            name: name.to_owned(),
        })
        .await;
    client
        .until(|message| match message {
            ServerMessage::SkillFigures(figures) => Some(Ok(figures)),
            ServerMessage::Error { message } => Some(Err(message)),
            _ => None,
        })
        .await
}

async fn bot_figures(client: &mut Client, bot_id: i64) -> BotSkillFigures {
    client
        .send(ClientMessage::GetBotSkillFigures { bot_id })
        .await;
    client
        .until(|message| match message {
            ServerMessage::BotSkillFigures(figures) => Some(figures),
            ServerMessage::Error { message } => panic!("refused: {message}"),
            _ => None,
        })
        .await
}

#[tokio::test]
async fn the_turns_that_load_a_skill_are_counted_per_version() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, work) = bot(&mut client, "Measured").await;
    for name in ["deploy", "tidy"] {
        let operations = json!({ "operations": [
            { "op": "create", "name": name, "content": skill_document(name) }
        ] });
        let created = client
            .ask(work, &format!("mcpcall skill_manage {operations}"))
            .await;
        assert!(created.contains("failed=false"), "{created}");
    }
    // A turn that loads a skill, and one that loads none while the index
    // lists two.
    let viewed = client
        .ask(work, r#"mcpcall skill_view {"name":"deploy"}"#)
        .await;
    assert!(viewed.contains("failed=false"), "{viewed}");
    client.ask(work, "say nothing loaded").await;

    let figures = skill_figures(&mut client, bot_id, "deploy").await.unwrap();
    assert_eq!(figures.versions.len(), 1);
    let version = &figures.versions[0];
    assert_eq!(version.turns, 1);
    assert_eq!(version.completed, Some(Figure(1.0)));
    // The scripted agent reports no usage for these turns.
    assert_eq!(version.mean_tokens, None);

    // A skill never loaded has no turn and no share.
    let never = skill_figures(&mut client, bot_id, "tidy").await.unwrap();
    assert_eq!(never.versions[0].turns, 0);
    assert_eq!(never.versions[0].completed, None);

    // The second skill was created while the first was listed: three turns
    // ended with a skill in the index, one of them loaded it.
    let bot = bot_figures(&mut client, bot_id).await;
    assert_eq!((bot.turns, bot.loaded), (3, 1));
    assert_eq!(bot.share, Some(Figure(1.0 / 3.0)));

    let missing = skill_figures(&mut client, bot_id, "nothing")
        .await
        .unwrap_err();
    assert!(missing.contains("no skill named nothing"), "{missing}");
}
