use std::time::{Duration, Instant};

use crowbot_protocol::{ClientMessage, Event, ServerMessage, TurnEnd};

use crate::support::{Daemon, queued_texts, user_texts};

const START_BOUND: Duration = Duration::from_millis(500);

fn user_text(text: &'static str) -> impl Fn(&Event) -> bool {
    move |event| matches!(event, Event::UserText(sent) if sent.text == text)
}

#[tokio::test]
async fn messages_sent_during_a_turn_wait_and_then_run_in_the_order_sent() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut other = daemon.client().await;
    let id = client.new_conversation().await;
    other.open(id).await;

    client.say(id, "stream 30 20").await;
    client.until_reply("w0").await;
    client.say(id, "say two").await;
    client.say(id, "say three").await;

    // Every attached client learns about the queue, not only the one that sent.
    for attached in [&mut client, &mut other] {
        let queue = attached
            .until_queue(|queue| queue.messages.len() == 2)
            .await;
        assert_eq!(queued_texts(&queue), ["say two", "say three"]);
        assert!(!queue.paused);
    }

    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
    let first_turn_ended = Instant::now();
    client.until_event(user_text("say two")).await;
    assert!(first_turn_ended.elapsed() < START_BOUND);
    client.until_reply("two").await;
    client.until_turn_ends().await;
    let second_turn_ended = Instant::now();
    client.until_event(user_text("say three")).await;
    assert!(second_turn_ended.elapsed() < START_BOUND);
    client.until_reply("three").await;
    client.until_turn_ends().await;

    let snapshot = other.open(id).await;
    assert_eq!(
        user_texts(&snapshot.events),
        ["stream 30 20", "say two", "say three"]
    );
    assert!(snapshot.queue.messages.is_empty());
}

#[tokio::test]
async fn an_edited_message_runs_with_its_new_text_and_a_deleted_one_never_runs() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "stream 60 20").await;
    client.until_reply("w0").await;
    client.say(id, "say draft").await;
    client.say(id, "say removed").await;
    let queue = client.until_queue(|queue| queue.messages.len() == 2).await;
    let (draft, removed) = (queue.messages[0].id, queue.messages[1].id);

    client
        .send(ClientMessage::EditQueuedMessage {
            conversation_id: id,
            queue_id: draft,
            text: "say final".to_owned(),
        })
        .await;
    client
        .send(ClientMessage::DeleteQueuedMessage {
            conversation_id: id,
            queue_id: removed,
        })
        .await;
    let queue = client.until_queue(|queue| queue.messages.len() == 1).await;
    assert_eq!(queued_texts(&queue), ["say final"]);

    client
        .send(ClientMessage::EditQueuedMessage {
            conversation_id: id,
            queue_id: removed,
            text: "say again".to_owned(),
        })
        .await;
    assert!(
        client
            .until_error()
            .await
            .contains("no longer in the queue")
    );

    client.until_reply("final").await;
    client.until_turn_ends().await;
    assert_eq!(client.next_within(START_BOUND).await, None);
    let snapshot = client.open(id).await;
    assert_eq!(user_texts(&snapshot.events), ["stream 60 20", "say final"]);
    assert!(snapshot.queue.messages.is_empty());
}

#[tokio::test]
async fn send_now_stops_the_running_turn_and_starts_that_message_next() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "stream 3000 20").await;
    client.until_reply("w0").await;
    client.say(id, "say later").await;
    client.say(id, "say urgent").await;
    let queue = client.until_queue(|queue| queue.messages.len() == 2).await;

    client
        .send(ClientMessage::SendQueuedMessageNow {
            conversation_id: id,
            queue_id: queue.messages[1].id,
        })
        .await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Cancelled)
    );
    client.until_event(user_text("say urgent")).await;
    client.until_reply("urgent").await;
    client.until_turn_ends().await;
    client.until_event(user_text("say later")).await;
    client.until_turn_ends().await;

    let snapshot = client.open(id).await;
    assert_eq!(
        user_texts(&snapshot.events),
        ["stream 3000 20", "say urgent", "say later"]
    );
}

#[tokio::test]
async fn a_failed_turn_pauses_the_queue_until_it_is_resumed() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "stream 20 20").await;
    client.until_reply("w0").await;
    client.say(id, "crash").await;
    client.say(id, "say after").await;

    client.until_event(user_text("crash")).await;
    assert!(matches!(
        client.until_turn_ends().await,
        Event::TurnFailed { .. }
    ));
    let queue = client.until_queue(|queue| queue.paused).await;
    assert_eq!(queued_texts(&queue), ["say after"]);
    assert_eq!(client.next_within(START_BOUND).await, None);
    assert!(client.open(id).await.queue.paused);

    client
        .send(ClientMessage::ResumeQueue {
            conversation_id: id,
        })
        .await;
    let queue = client.until_queue(|queue| !queue.paused).await;
    assert_eq!(queued_texts(&queue), ["say after"]);
    client.until_event(user_text("say after")).await;
    client.until_reply("after").await;
    assert_eq!(
        client.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
    assert!(client.open(id).await.queue.messages.is_empty());
}

#[tokio::test]
async fn a_restart_keeps_the_queue_and_starts_nothing_twice() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "stream 3000 20").await;
    client.until_reply("w0").await;
    client.say(id, "say one").await;
    client.say(id, "say two").await;
    client.until_queue(|queue| queue.messages.len() == 2).await;
    daemon.kill();

    daemon.spawn();
    let mut client = daemon.client().await;
    let snapshot = client.open(id).await;
    assert_eq!(queued_texts(&snapshot.queue), ["say one", "say two"]);
    assert!(snapshot.queue.paused);
    assert_eq!(user_texts(&snapshot.events), ["stream 3000 20"]);
    assert_eq!(client.next_within(START_BOUND).await, None);

    client
        .send(ClientMessage::ResumeQueue {
            conversation_id: id,
        })
        .await;
    client.until_reply("one").await;
    client.until_turn_ends().await;
    client.until_reply("two").await;
    client.until_turn_ends().await;
    assert_eq!(client.next_within(START_BOUND).await, None);

    let snapshot = client.open(id).await;
    assert_eq!(
        user_texts(&snapshot.events),
        ["stream 3000 20", "say one", "say two"]
    );
    assert!(snapshot.queue.messages.is_empty());
    assert!(matches!(
        client.next_within(Duration::from_millis(100)).await,
        None | Some(ServerMessage::QueueChanged { .. })
    ));
}
