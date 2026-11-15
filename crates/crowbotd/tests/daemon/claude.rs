//! Smoke tests on the real Claude adapter. They spend the user's own Claude
//! usage, and the second one downloads the pinned runtime from its publisher,
//! so they run only on request:
//! `cargo test -p crowbotd --test daemon claude -- --ignored`.

use crowbot_protocol::{BotDraft, Event, ServerMessage};

use crate::support::Daemon;

const MARKER: &str = "CROWBOT-7";

#[tokio::test]
#[ignore = "runs a real Claude turn on the user's own sign-in"]
async fn a_claude_bot_answers_from_its_instructions_on_its_model() {
    let daemon = Daemon::start_with_claude();
    let mut client = daemon.client().await;
    let bot = client
        .create_bot(BotDraft {
            name: "Smoke".to_owned(),
            instructions: format!(
                "You are a test bot. End every answer with the exact word {MARKER}."
            ),
            model: Some("haiku".to_owned()),
            ..BotDraft::default()
        })
        .await;
    assert_eq!(bot.bot.provider, "claude");
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;

    client
        .say(
            conversation,
            "What are your instructions? Answer in one sentence.",
        )
        .await;
    let mut answer = String::new();
    loop {
        if let ServerMessage::ConversationEvent { event, .. } = client.next_within_minutes().await {
            match event {
                Event::MessageText(text) => answer.push_str(&text),
                Event::TurnFailed { message, .. } => panic!("the turn failed: {message}"),
                event if event.ends_turn() => break,
                _ => {}
            }
        }
    }
    println!("Claude answered: {answer}");
    assert!(answer.contains(MARKER), "{answer}");
}

#[tokio::test]
#[ignore = "downloads the pinned Claude runtime and runs a real Claude turn"]
async fn a_claude_bot_answers_from_the_runtime_the_daemon_installs_with_no_bun_and_no_node() {
    let daemon = Daemon::start_with_claude_alone();
    let mut client = daemon.client().await;
    let bot = client
        .create_bot(BotDraft {
            name: "Alone".to_owned(),
            instructions: format!(
                "You are a test bot. End every answer with the exact word {MARKER}."
            ),
            ..BotDraft::default()
        })
        .await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;

    client
        .say(conversation, "Say hello in one short sentence.")
        .await;
    let mut answer = String::new();
    let mut downloaded = 0;
    loop {
        match client.next_within_minutes().await {
            ServerMessage::ProviderChanged(status) if status.id == "claude" => {
                if let Some(install) = status.install {
                    downloaded = downloaded.max(install.received);
                }
            }
            ServerMessage::ConversationEvent { event, .. } => match event {
                Event::MessageText(text) => answer.push_str(&text),
                Event::TurnFailed { message, .. } => panic!("the turn failed: {message}"),
                event if event.ends_turn() => break,
                _ => {}
            },
            _ => {}
        }
    }
    println!("Claude answered after a download of {downloaded} bytes: {answer}");
    assert!(downloaded > 100_000_000, "{downloaded}");
    assert!(answer.contains(MARKER), "{answer}");
    let runtimes = daemon.data_dir().join("runtimes").join("claude");
    assert!(runtimes.is_dir());
}
