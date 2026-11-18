//! Setup and settings: the answer to a saved field, the deletion of a bot's
//! history, the state and the sign-in of a provider, what the daemon tells
//! about itself, and the search.

use std::path::Path;
use std::time::{Duration, Instant};

use crowbot_protocol::{
    BotPatch, ClientMessage, Event, MessageAuthor, PROTOCOL_VERSION, ProviderState, ProviderStatus,
    SearchResults, ServerMessage, TimedEvent,
};

use crate::conversation::write_fourth_database;
use crate::support::{self, Client, Daemon, user_texts};

const QUIET: Duration = Duration::from_millis(400);
const PROVIDER: &str = "custom";

async fn search(client: &mut Client, request: u64, query: &str) -> SearchResults {
    client
        .send(ClientMessage::Search {
            request,
            query: query.to_owned(),
        })
        .await;
    client
        .until(|message| match message {
            ServerMessage::SearchResults(results) if results.request == request => Some(results),
            ServerMessage::Error { message } => panic!("the search failed: {message}"),
            _ => None,
        })
        .await
}

async fn until_provider(
    client: &mut Client,
    matches: impl Fn(&ProviderStatus) -> bool,
) -> ProviderStatus {
    client
        .until(|message| match message {
            ServerMessage::ProviderChanged(status) if matches(&status) => Some(status),
            ServerMessage::Error { message } => panic!("the daemon refused: {message}"),
            _ => None,
        })
        .await
}

fn settled(status: &ProviderStatus) -> bool {
    !status.checking && !status.signing_in
}

/// Asks for the providers and waits for the state of the test agent.
async fn provider(client: &mut Client) -> ProviderStatus {
    client.send(ClientMessage::ListProviders).await;
    let listed = client
        .until(|message| match message {
            ServerMessage::Providers(providers) => Some(providers),
            _ => None,
        })
        .await;
    let [status] = listed.as_slice() else {
        panic!("expected one provider, got {listed:?}");
    };
    assert_eq!(
        (status.id.as_str(), status.name.as_str()),
        (PROVIDER, "Custom agent")
    );
    if settled(status) {
        return status.clone();
    }
    until_provider(client, settled).await
}

fn counter(file: &Path) -> Option<u64> {
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// Waits until the counter in the file grows, which tells that the process
/// that rewrites it is alive.
async fn grows(file: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    let first = counter(file);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(60)).await;
        if counter(file).is_some() && counter(file) != first {
            return true;
        }
    }
    false
}

// US-027: a saved field is confirmed to the client that sent it.

#[tokio::test]
async fn a_saved_field_is_confirmed_to_the_client_that_sent_it_and_shown_to_all() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut other = daemon.client().await;
    let bot = client.new_bot("Atlas").await;

    let started = Instant::now();
    client
        .update_bot(
            bot.bot.id,
            BotPatch {
                instructions: Some("Answer in one line.".to_owned()),
                model: Some(Some("smart".to_owned())),
                ..BotPatch::default()
            },
        )
        .await;
    let mut changed = None;
    let confirmed = client
        .until(|message| match message {
            ServerMessage::BotChanged(summary) => {
                changed = Some(summary);
                None
            }
            ServerMessage::BotUpdated { bot_id } => Some(bot_id),
            ServerMessage::InvalidField { field, message } => panic!("{field}: {message}"),
            _ => None,
        })
        .await;
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(confirmed, bot.bot.id);
    let changed = changed.expect("the change comes before its confirmation");
    assert_eq!(changed.bot.instructions, "Answer in one line.");
    assert_eq!(changed.bot.model.as_deref(), Some("smart"));

    let seen = other
        .until_bot(|summary| summary.bot.model.as_deref() == Some("smart"))
        .await;
    assert_eq!(seen.bot.instructions, "Answer in one line.");
    while let Some(message) = other.next_within(QUIET).await {
        assert!(
            !matches!(message, ServerMessage::BotUpdated { .. }),
            "only the client that saved gets the confirmation"
        );
    }

    // A refused field gets no confirmation, and the stored value stays.
    client
        .update_bot(
            bot.bot.id,
            BotPatch {
                instructions: Some("x".repeat(20_001)),
                ..BotPatch::default()
            },
        )
        .await;
    let refused = client
        .until(|message| match message {
            ServerMessage::InvalidField { field, message } => Some((field, message)),
            ServerMessage::BotUpdated { .. } => panic!("a refused field was confirmed"),
            _ => None,
        })
        .await;
    assert_eq!(refused.0, "instructions");
    assert!(refused.1.contains("20000"), "{}", refused.1);
    assert_eq!(
        client.bot(bot.bot.id).await.bot.instructions,
        "Answer in one line."
    );
}

// US-027: Delete history.

#[tokio::test]
async fn deleting_a_history_stops_the_turn_and_leaves_one_empty_conversation() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut other = daemon.client().await;
    let atlas = client.new_bot("Atlas").await;
    let wren = client.new_bot("Wren").await;
    let first = atlas.current_conversation_id;
    client.open(first).await;
    assert_eq!(
        client.ask(first, "say remember the lighthouse").await,
        "remember the lighthouse"
    );
    client.open(wren.current_conversation_id).await;
    client
        .ask(wren.current_conversation_id, "say the lighthouse keeper")
        .await;

    client
        .send(ClientMessage::NewConversation {
            bot_id: atlas.bot.id,
        })
        .await;
    let second = client
        .until(|message| match message {
            ServerMessage::ConversationCreated(summary) => Some(summary.id),
            _ => None,
        })
        .await;
    client.open(second).await;
    client.say(second, "stream 600 20").await;
    client
        .until_event(|event| matches!(event, Event::MessageText(_)))
        .await;
    assert_eq!(search(&mut client, 1, "lighthouse").await.messages.len(), 4);

    client
        .send(ClientMessage::DeleteHistory {
            bot_id: atlas.bot.id,
        })
        .await;
    let deleted = |message| match message {
        ServerMessage::HistoryDeleted {
            bot_id,
            current_conversation_id,
        } => Some((bot_id, current_conversation_id)),
        ServerMessage::Error { message } => panic!("the daemon refused: {message}"),
        _ => None,
    };
    let (bot_id, current) = client.until(deleted).await;
    assert_eq!(bot_id, atlas.bot.id);
    assert!(current != first && current != second);
    assert_eq!(other.until(deleted).await, (atlas.bot.id, current));

    let history = client.conversations(atlas.bot.id).await;
    assert_eq!(
        history.iter().map(|summary| summary.id).collect::<Vec<_>>(),
        [current]
    );
    let summary = client.bot(atlas.bot.id).await;
    assert_eq!(summary.current_conversation_id, current);
    assert_eq!((summary.unread, summary.preview.as_str()), (0, ""));
    assert_eq!(summary.bot.name, "Atlas");
    assert!(client.open(current).await.events.is_empty());

    // The messages of the bot left the search; the other bot keeps its own.
    let left = search(&mut client, 2, "lighthouse").await;
    assert_eq!(left.messages.len(), 2);
    assert!(left.messages.iter().all(|hit| hit.bot_id == wren.bot.id));
    assert_eq!(
        user_texts(&client.open(wren.current_conversation_id).await.events),
        ["say the lighthouse keeper"]
    );

    // A removed conversation is gone, and the bot goes on in the new one.
    client.say(first, "say again").await;
    let (refused, _) = client.until_refused().await;
    assert_eq!(refused, first);
    assert_eq!(client.ask(current, "say fresh start").await, "fresh start");

    client
        .send(ClientMessage::DeleteHistory { bot_id: 999 })
        .await;
    assert_eq!(client.until_error().await, "bot 999 does not exist");
}

// US-028: the state of a provider and its sign-in.

#[tokio::test]
async fn a_signed_out_provider_signs_in_through_its_own_program() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut other = daemon.client().await;

    let status = provider(&mut client).await;
    assert_eq!(status.state, ProviderState::SignedOut);
    assert!(status.can_sign_in);
    assert_eq!(status.account, None);
    provider(&mut other).await;

    client
        .send(ClientMessage::SignIn {
            provider: PROVIDER.to_owned(),
            method: None,
        })
        .await;
    let started = Instant::now();
    let signing_in = until_provider(&mut client, |status| status.signing_in).await;
    assert_eq!(signing_in.state, ProviderState::SignedOut);
    let ready = until_provider(&mut client, |status| {
        settled(status) && status.state == ProviderState::Ready
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(ready.account.as_deref(), Some("Fake Plan"));
    assert_eq!(ready.detail, None);
    let sent = serde_json::to_string(&ready).unwrap();
    assert!(
        !sent.contains("person@example.com") && !sent.contains("Example Org"),
        "{sent}"
    );

    // Every attached client learns the new state.
    let seen = until_provider(&mut other, |status| {
        settled(status) && status.state == ProviderState::Ready
    })
    .await;
    assert_eq!(seen.account.as_deref(), Some("Fake Plan"));

    // A sign-out made outside the daemon shows at the next check.
    std::fs::remove_file(daemon.auth_file("")).unwrap();
    client
        .send(ClientMessage::CheckProvider {
            provider: PROVIDER.to_owned(),
        })
        .await;
    let checking = until_provider(&mut client, |status| status.checking).await;
    assert_eq!(checking.state, ProviderState::Ready);
    let signed_out = until_provider(&mut client, settled).await;
    assert_eq!(signed_out.state, ProviderState::SignedOut);
    assert_eq!(signed_out.account, None);

    // The state is kept: a client that attaches later gets it with no new check.
    let mut late = daemon.client().await;
    late.send(ClientMessage::ListProviders).await;
    let listed = late
        .until(|message| match message {
            ServerMessage::Providers(providers) => Some(providers),
            _ => None,
        })
        .await;
    assert_eq!(listed, [signed_out]);
}

#[tokio::test]
async fn a_cancelled_sign_in_ends_its_process_and_the_processes_it_started() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    provider(&mut client).await;
    std::fs::write(daemon.auth_file("mode"), "wait").unwrap();
    let (own, child) = (daemon.auth_file("beat"), daemon.auth_file("child"));

    let sign_in = ClientMessage::SignIn {
        provider: PROVIDER.to_owned(),
        method: None,
    };
    client.send(sign_in.clone()).await;
    until_provider(&mut client, |status| status.signing_in).await;
    assert!(grows(&own).await, "the sign-in process does not run");
    assert!(
        grows(&child).await,
        "the process started by the sign-in does not run"
    );

    // A second request while one runs starts nothing.
    client.send(sign_in).await;
    assert!(client.next_within(QUIET).await.is_none());

    client
        .send(ClientMessage::CancelSignIn {
            provider: PROVIDER.to_owned(),
        })
        .await;
    let ended = until_provider(&mut client, settled).await;
    assert_eq!(ended.state, ProviderState::SignedOut);
    assert_eq!(ended.detail, None);
    assert!(!grows(&own).await, "the sign-in process still runs");
    assert!(
        !grows(&child).await,
        "the process started by the sign-in still runs"
    );
    assert!(!daemon.auth_file("").exists());
}

/// On Linux the page a sign-in opens, with `xdg-open` or with the program
/// `BROWSER` names, reaches the clients: no browser starts in the process
/// group that a cancel kills, and a daemon with no display variable still
/// shows the page.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_page_a_sign_in_opens_reaches_the_clients_in_place_of_a_browser() {
    for opener in ["xdg-open", "browser"] {
        let daemon = Daemon::start();
        let mut client = daemon.client().await;
        provider(&mut client).await;
        std::fs::write(daemon.auth_file("mode"), opener).unwrap();
        client
            .send(ClientMessage::SignIn {
                provider: PROVIDER.to_owned(),
                method: None,
            })
            .await;
        let named = until_provider(&mut client, |status| {
            status.signing_in && status.sign_in_url.is_some()
        })
        .await;
        assert_eq!(
            named.sign_in_url.as_deref(),
            Some("https://login.example/oauth/authorize?state=FAKE"),
            "{opener}"
        );
        // The opener told the sign-in that the page is open: it waits.
        assert!(grows(&daemon.auth_file("beat")).await, "{opener}");

        client
            .send(ClientMessage::CancelSignIn {
                provider: PROVIDER.to_owned(),
            })
            .await;
        let ended = until_provider(&mut client, settled).await;
        assert_eq!(ended.sign_in_url, None, "{opener}");
        let caught = daemon
            .data_dir()
            .join("providers")
            .join(PROVIDER)
            .join("sign-in");
        // The folder of the sign-in went with it.
        assert!(
            std::fs::read_dir(&caught).unwrap().next().is_none(),
            "{opener}"
        );
    }
}

#[tokio::test]
async fn a_finished_sign_in_leaves_the_processes_it_started_alone() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    provider(&mut client).await;
    std::fs::write(daemon.auth_file("mode"), "guest").unwrap();
    client
        .send(ClientMessage::SignIn {
            provider: PROVIDER.to_owned(),
            method: None,
        })
        .await;
    until_provider(&mut client, |status| {
        settled(status) && status.state == ProviderState::Ready
    })
    .await;
    // The sign-in opened something that is the user's, such as a browser: it
    // goes on after the sign-in ended.
    assert!(
        grows(&daemon.auth_file("child")).await,
        "the process started by the sign-in ended with it"
    );
}

#[tokio::test]
async fn a_sign_in_that_fails_says_so_and_leaves_the_provider_signed_out() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    provider(&mut client).await;
    std::fs::write(daemon.auth_file("mode"), "fail").unwrap();
    client
        .send(ClientMessage::SignIn {
            provider: PROVIDER.to_owned(),
            method: None,
        })
        .await;
    until_provider(&mut client, |status| status.signing_in).await;
    let ended = until_provider(&mut client, |status| {
        settled(status) && status.detail.is_some()
    })
    .await;
    assert_eq!(ended.state, ProviderState::SignedOut);
    assert_eq!(ended.detail.as_deref(), Some("The sign-in did not finish."));

    // The note goes away once the provider is ready.
    std::fs::write(daemon.auth_file("mode"), "0").unwrap();
    client
        .send(ClientMessage::SignIn {
            provider: PROVIDER.to_owned(),
            method: None,
        })
        .await;
    let ready = until_provider(&mut client, |status| {
        settled(status) && status.state == ProviderState::Ready
    })
    .await;
    assert_eq!(ready.detail, None);
}

#[tokio::test]
async fn a_provider_whose_program_is_missing_says_which_one() {
    let daemon = Daemon::start_with_missing_agent();
    let mut client = daemon.client().await;
    let status = provider(&mut client).await;
    assert_eq!(status.state, ProviderState::RuntimeMissing);
    assert!(!status.can_sign_in);
    let detail = status.detail.unwrap();
    assert!(
        detail.contains("no-such-agent") && detail.contains("was not found"),
        "{detail}"
    );

    // A bot can still be created: the turn is where the missing program stops it.
    let bot = client.new_bot("Atlas").await;
    assert_eq!(bot.bot.provider, PROVIDER);
}

#[tokio::test]
async fn a_provider_that_tells_nothing_about_its_sign_in_is_unknown() {
    let daemon = Daemon::start_with_silent_agent();
    let mut client = daemon.client().await;
    let status = provider(&mut client).await;
    assert_eq!(status.state, ProviderState::Unknown);
    assert!(!status.can_sign_in);
    assert_eq!(
        status.detail.as_deref(),
        Some("The agent does not tell whether it is signed in.")
    );

    client
        .send(ClientMessage::SignIn {
            provider: PROVIDER.to_owned(),
            method: None,
        })
        .await;
    let refused = client.until_error().await;
    assert!(refused.contains("offers no sign-in"), "{refused}");

    client
        .send(ClientMessage::CheckProvider {
            provider: "nope".to_owned(),
        })
        .await;
    assert_eq!(client.until_error().await, "unknown provider: nope");

    // An unknown state stops nothing: the turn is the final check.
    let id = client.new_conversation().await;
    assert_eq!(client.ask(id, "say caw").await, "caw");
}

#[tokio::test]
#[ignore = "downloads the pinned Claude runtime and reads the user's own sign-in state"]
async fn the_claude_adapter_tells_its_sign_in_state() {
    let daemon = Daemon::start_with_claude();
    let mut client = daemon.client().await;
    // A fresh daemon has no runtime yet: it says so, and installs it on request.
    client.send(ClientMessage::ListProviders).await;
    let status = client
        .until(|message| match message {
            ServerMessage::ProviderChanged(status) if settled(&status) => Some(status),
            _ => None,
        })
        .await;
    assert_eq!(status.state, ProviderState::NotInstalled, "{status:?}");
    client
        .send(ClientMessage::InstallRuntime {
            provider: "claude".to_owned(),
        })
        .await;
    let status = loop {
        if let ServerMessage::ProviderChanged(status) = client.next_within_minutes().await
            && settled(&status)
            && status.install.is_none()
            && status.state != ProviderState::NotInstalled
        {
            break status;
        }
    };
    println!(
        "Claude: {:?}, can sign in: {}, detail: {:?}",
        status.state, status.can_sign_in, status.detail
    );
    assert_eq!(
        (status.id.as_str(), status.name.as_str()),
        ("claude", "Claude")
    );
    assert!(matches!(
        status.state,
        ProviderState::Ready | ProviderState::SignedOut
    ));
    assert!(status.can_sign_in);
    // The adapter ran from its folder and wrote nothing there: a second check
    // finds the files it was installed with.
    client
        .send(ClientMessage::CheckProvider {
            provider: "claude".to_owned(),
        })
        .await;
    let checked = loop {
        if let ServerMessage::ProviderChanged(checked) = client.next_within_minutes().await
            && settled(&checked)
        {
            break checked;
        }
    };
    assert_eq!(checked.state, status.state, "{checked:?}");
}

// US-028: About.

#[tokio::test]
async fn the_daemon_tells_its_versions_and_where_its_database_is() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    client.send(ClientMessage::GetDaemonInfo).await;
    let info = client
        .until(|message| match message {
            ServerMessage::DaemonInfo(info) => Some(info),
            _ => None,
        })
        .await;
    assert_eq!(info.daemon_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert_eq!(Path::new(&info.database), daemon.database());
}

// US-029: the search.

#[tokio::test]
async fn a_search_finds_bots_conversations_by_title_and_messages() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let atlas = client.new_bot("Atlas").await;
    let harbor = client.new_bot("Harbor master").await;
    let chat = atlas.current_conversation_id;
    client.open(chat).await;
    client.ask(chat, "say The harbor opens at dawn.").await;
    client.ask(chat, "say Nothing else to report.").await;

    let started = Instant::now();
    let results = search(&mut client, 7, "harbor").await;
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(results.query, "harbor");
    assert_eq!(results.bots, [harbor.bot.id]);
    let titles: Vec<(i64, &str)> = results
        .conversations
        .iter()
        .map(|hit| (hit.conversation_id, hit.title.as_str()))
        .collect();
    assert_eq!(titles, [(chat, "say The harbor opens at dawn.")]);
    assert!(results.messages_searched);
    let found: Vec<(MessageAuthor, &str, i64, i64)> = results
        .messages
        .iter()
        .map(|hit| {
            (
                hit.author,
                hit.snippet.as_str(),
                hit.conversation_id,
                hit.bot_id,
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                MessageAuthor::Bot,
                "The harbor opens at dawn.",
                chat,
                atlas.bot.id
            ),
            (
                MessageAuthor::User,
                "say The harbor opens at dawn.",
                chat,
                atlas.bot.id
            ),
        ]
    );

    // Nothing matches: every group is empty, and the messages were searched.
    let none = search(&mut client, 8, "zeppelin").await;
    assert!(none.bots.is_empty() && none.conversations.is_empty() && none.messages.is_empty());
    assert!(none.messages_searched);

    // Operators and quotes that do not parse are plain text, with no error.
    client.ask(chat, "say use \"quotes AND (parens").await;
    for (request, query, expected) in [
        (10, "\"quotes AND (parens", 2),
        (11, "(parens", 2),
        (12, "harbor OR zeppelin", 0),
        (13, "harbor NOT dawn", 0),
        (14, "harb*", 0),
        (15, "NEAR(harbor dawn)", 0),
        (16, "\"", 0),
        (17, "\"\"\"", 0),
        (18, "report. -Nothing", 0),
        (19, "'; DROP TABLE events; --", 0),
    ] {
        let results = search(&mut client, request, query).await;
        assert_eq!(results.messages.len(), expected, "{query}");
    }
    assert_eq!(search(&mut client, 20, "harbor").await.messages.len(), 2);
}

#[tokio::test]
async fn events_stored_by_an_older_daemon_are_searchable() {
    let daemon = Daemon::start_on(write_fourth_database);
    let mut client = daemon.client().await;
    let results = search(&mut client, 1, "the trip").await;
    assert_eq!(results.conversations.len(), 1);
    assert_eq!(results.conversations[0].title, "Plan the trip");
    let found: Vec<(i64, i64, &str)> = results
        .messages
        .iter()
        .map(|hit| (hit.event_id, hit.conversation_id, hit.snippet.as_str()))
        .collect();
    assert_eq!(found, [(1, 1, "Plan the trip")]);
    assert_eq!(support::events(&client.open(1).await).len(), 6);
}

fn ids(events: &[TimedEvent]) -> Vec<i64> {
    events.iter().map(|timed| timed.id).collect()
}

#[tokio::test]
async fn a_hit_outside_the_loaded_page_comes_with_its_turn_up_to_the_page() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let chat = client.new_conversation().await;
    client.ask(chat, "say the albatross landed").await;
    for turn in 0..90 {
        client.ask(chat, &format!("say filler {turn}")).await;
    }

    let snapshot = client.open(chat).await;
    let loaded = ids(&snapshot.events);
    assert_eq!(loaded.len(), 200);
    assert!(loaded.windows(2).all(|pair| pair[0] < pair[1]), "ids grow");
    let before = snapshot.older.expect("older events exist");
    assert_eq!(before, loaded[0]);

    let results = search(&mut client, 1, "albatross landed").await;
    let hit = results
        .messages
        .iter()
        .find(|hit| hit.author == MessageAuthor::Bot)
        .expect("the answer is found");
    assert!(hit.event_id < before);

    client
        .send(ClientMessage::RevealEvent {
            conversation_id: chat,
            before,
            event_id: hit.event_id,
        })
        .await;
    let (events, older) = client
        .until(|message| match message {
            ServerMessage::OlderEvents { events, older, .. } => Some((events, older)),
            ServerMessage::OlderEventsFailed { message, .. } => panic!("{message}"),
            _ => None,
        })
        .await;
    assert_eq!(older, None, "the hit is in the first turn");
    assert_eq!(user_texts(&events)[0], "say the albatross landed");
    let revealed = ids(&events);
    assert!(revealed.contains(&hit.event_id));
    assert!(revealed.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(revealed.len() + loaded.len(), 91 * 3);
    assert!(revealed.last().unwrap() < &before);
}

#[tokio::test]
async fn a_hit_far_from_the_loaded_page_comes_as_a_page_that_leads_to_it() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let chat = client.new_conversation().await;
    client.ask(chat, "say the albatross landed").await;
    for turn in 0..420 {
        client.ask(chat, &format!("say filler {turn}")).await;
    }
    let snapshot = client.open(chat).await;
    let loaded = ids(&snapshot.events);
    let before = snapshot.older.expect("older events exist");
    let results = search(&mut client, 1, "albatross landed").await;
    let hit = results
        .messages
        .iter()
        .find(|hit| hit.author == MessageAuthor::Bot)
        .expect("the answer is found");

    client
        .send(ClientMessage::RevealEvent {
            conversation_id: chat,
            before,
            event_id: hit.event_id,
        })
        .await;
    let (events, older, newer) = client
        .until(|message| match message {
            ServerMessage::EventsWindow {
                events,
                older,
                newer,
                ..
            } => Some((events, older, newer)),
            ServerMessage::OlderEvents { events, .. } => {
                panic!("{} events came in one answer", events.len())
            }
            ServerMessage::OlderEventsFailed { message, .. } => panic!("{message}"),
            _ => None,
        })
        .await;
    assert_eq!(events.len(), 200, "one page, not the 1,063 events between");
    assert_eq!(older, None, "the hit is in the first turn");
    assert_eq!(user_texts(&events)[0], "say the albatross landed");
    let mut revealed = ids(&events);
    assert!(revealed.contains(&hit.event_id));
    assert_eq!(newer, revealed.last().copied());

    // The pages after it end where the events of the client start.
    let mut newer = newer;
    let mut pages = 0;
    while let Some(after) = newer {
        client
            .send(ClientMessage::LoadNewerEvents {
                conversation_id: chat,
                after,
                until: before,
            })
            .await;
        let (events, next) = client
            .until(|message| match message {
                ServerMessage::NewerEvents { events, newer, .. } => Some((events, newer)),
                ServerMessage::NewerEventsFailed { message, .. } => panic!("{message}"),
                _ => None,
            })
            .await;
        assert!(events.len() <= 200);
        revealed.extend(ids(&events));
        newer = next;
        pages += 1;
    }
    assert_eq!(pages, 5);
    assert!(revealed.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(revealed.last().unwrap() < &before);
    assert_eq!(revealed.len() + loaded.len(), 421 * 3);
}
