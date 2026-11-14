//! A smoke test on the real Claude adapter. It spends the user's own Claude
//! usage, so it runs only on request:
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
