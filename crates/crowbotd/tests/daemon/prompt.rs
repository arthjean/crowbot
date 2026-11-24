//! The system prompt in tiers and the daemon's note (US-005 of
//! `tasks/prd-crow-bot-harness.md`). The daemon's two test sections
//! (`test_harness.rs`) read the files `section-a.txt` (rank 10) and
//! `section-b.txt` (rank 20) of the test folder, and are registered in the
//! other order.

use crowbot_protocol::{BotDraft, Event, UserMessage};

use crate::support::{self, Daemon};

fn write_sections(daemon: &Daemon, a: &str, b: &str) {
    std::fs::write(daemon.harness_file("section-a.txt"), a).unwrap();
    std::fs::write(daemon.harness_file("section-b.txt"), b).unwrap();
}

fn with_instructions(instructions: &str) -> BotDraft {
    BotDraft {
        name: "Tiers".to_owned(),
        instructions: instructions.to_owned(),
        ..BotDraft::default()
    }
}

/// The stored sections of each provider session of a conversation.
fn stored_sections(daemon: &Daemon, conversation: i64) -> Vec<(String, serde_json::Value)> {
    let connection = rusqlite::Connection::open(daemon.database()).unwrap();
    let mut statement = connection
        .prepare("SELECT fingerprint, sections FROM provider_sessions WHERE conversation_id = ?1")
        .unwrap();
    statement
        .query_map([conversation], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|row| {
            let (fingerprint, sections) = row.unwrap();
            (fingerprint, serde_json::from_str(&sections).unwrap())
        })
        .collect()
}

fn notes(events: &[Event]) -> Vec<Option<String>> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::UserText(UserMessage { note, .. }) => Some(note.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_instructions_come_first_and_the_sections_follow_by_rank() {
    let daemon = Daemon::start_with_system_prompt();
    write_sections(&daemon, "Alpha section.", "Beta section.");
    let mut client = daemon.client().await;
    let bot = client.create_bot(with_instructions("Be brief.")).await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;
    let config = client.ask(conversation, "config").await;
    assert!(
        config.starts_with("system=[Be brief.\n\nAlpha section.\n\nBeta section.] preface=[]"),
        "{config}"
    );
    // The versions the session saw are stored with it.
    let stored = stored_sections(&daemon, conversation);
    assert_eq!(stored.len(), 1);
    let sections = &stored[0].1;
    assert!(sections["test-a"].is_string(), "{sections}");
    assert!(sections["test-b"].is_string(), "{sections}");
}

#[tokio::test]
async fn an_agent_with_no_system_prompt_reads_the_sections_after_the_instructions_first() {
    let daemon = Daemon::start();
    write_sections(&daemon, "Alpha.", "Beta.");
    let mut client = daemon.client().await;
    let bot = client.create_bot(with_instructions("Be brief.")).await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;
    let first = client.ask(conversation, "config").await;
    assert!(
        first.starts_with("system=[] preface=[Be brief.\n\nAlpha.\n\nBeta.]"),
        "{first}"
    );
    let second = client.ask(conversation, "config").await;
    assert!(second.starts_with("system=[] preface=[]"), "{second}");
}

#[tokio::test]
async fn a_changed_section_travels_in_one_note_and_keeps_the_session() {
    let mut daemon = Daemon::start_with_system_prompt();
    write_sections(&daemon, "Alpha one.", "Beta one.");
    let mut client = daemon.client().await;
    let bot = client.create_bot(with_instructions("Be brief.")).await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;
    let session = client.ask(conversation, "session").await;
    let fingerprint = stored_sections(&daemon, conversation)[0].0.clone();

    std::fs::write(daemon.harness_file("section-a.txt"), "Alpha two.").unwrap();
    let config = client.ask(conversation, "config").await;
    // The system prompt stays the one the session opened with, and the note
    // comes before the person's text.
    assert!(
        config.starts_with("system=[Be brief.\n\nAlpha one.\n\nBeta one.] preface=["),
        "{config}"
    );
    let preface = config
        .split("preface=[")
        .nth(1)
        .unwrap()
        .split("] model=")
        .next()
        .unwrap();
    assert!(
        preface.contains("Section test-a now reads: Alpha two."),
        "{preface}"
    );
    assert!(!preface.contains("test-b"), "{preface}");
    // The same session, and the same fingerprint.
    assert_eq!(client.ask(conversation, "session").await, session);
    assert_eq!(stored_sections(&daemon, conversation)[0].0, fingerprint);

    // The note is stored with the message, and only once.
    let snapshot = client.open(conversation).await;
    let notes = notes(&support::events(&snapshot));
    assert_eq!(notes.len(), 3, "{notes:?}");
    assert_eq!(notes[0], None);
    assert!(
        notes[1]
            .as_deref()
            .is_some_and(|note| note.contains("Alpha two.")),
        "{notes:?}"
    );
    assert_eq!(notes[2], None);

    // A resumed session gets the change it has not seen in a note too.
    std::fs::write(daemon.harness_file("section-b.txt"), "Beta two.").unwrap();
    drop(client);
    daemon.kill();
    daemon.spawn();
    let mut client = daemon.client().await;
    client.open(conversation).await;
    let config = client.ask(conversation, "config").await;
    assert!(
        config.contains("Section test-b now reads: Beta two."),
        "{config}"
    );
    assert!(!config.contains("Section test-a now reads"), "{config}");
    let resumed = client.ask(conversation, "session").await;
    assert!(resumed.ends_with("resumed=true"), "{resumed}");
}

#[tokio::test]
async fn a_queued_message_keeps_its_note() {
    let daemon = Daemon::start_with_system_prompt();
    write_sections(&daemon, "Alpha.", "Beta.");
    let mut client = daemon.client().await;
    let bot = client.create_bot(with_instructions("Be brief.")).await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;
    client.ask(conversation, "say ready").await;
    client.say(conversation, "stream 3 100").await;
    client.until_accepted().await;
    std::fs::write(daemon.harness_file("section-a.txt"), "Alpha later.").unwrap();
    client.say(conversation, "config").await;
    client.until_reply("Alpha later.").await;
    let snapshot = client.open(conversation).await;
    let notes = notes(&support::events(&snapshot));
    assert!(
        notes
            .last()
            .unwrap()
            .as_deref()
            .is_some_and(|note| note.contains("Alpha later.")),
        "{notes:?}"
    );
}
