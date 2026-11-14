use crowbot_protocol::transport::Listener;
use crowbot_protocol::{ClientMessage, Event, PROTOCOL_VERSION, ServerMessage, TurnEnd};

use crate::support::{self, Daemon};

#[tokio::test]
async fn a_client_of_the_same_user_exchanges_hello_and_welcome() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    client.send(ClientMessage::ListBots).await;
    assert_eq!(client.next().await, ServerMessage::Bots(Vec::new()));
}

#[tokio::test]
async fn a_turn_keeps_running_after_its_client_disconnects() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let id = client.new_conversation().await;
    client.say(id, "stream 100 20").await;
    client.until_reply("w0").await;
    drop(client);

    let mut later = daemon.client().await;
    assert!(later.open(id).await.turn_active);
    later.until_reply("w99").await;
    assert_eq!(
        later.until_turn_ends().await,
        Event::TurnEnded(TurnEnd::Completed)
    );
}

#[tokio::test]
async fn a_client_that_speaks_another_protocol_version_is_refused() {
    let dir = support::test_dir();
    let endpoint = support::test_endpoint(&dir);
    let mut daemon = std::process::Command::new(env!("CARGO_BIN_EXE_crowbotd"))
        .arg("--db")
        .arg(dir.join("crowbot.db"))
        .args(["--endpoint", endpoint.address()])
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let (mut reader, mut writer) = support::connect(&endpoint).await;
    writer
        .write(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION - 1,
        })
        .await
        .unwrap();
    let refusal = reader.read::<ServerMessage>().await.unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    std::fs::remove_dir_all(&dir).ok();
    let Some(ServerMessage::Error { message }) = refusal else {
        panic!("expected a refusal, got {refusal:?}");
    };
    assert!(
        message.contains(&format!("version {PROTOCOL_VERSION}")),
        "{message}"
    );
    assert!(
        message.contains(&format!("speaks {}", PROTOCOL_VERSION - 1)),
        "{message}"
    );
}

#[tokio::test]
async fn a_daemon_does_not_serve_an_endpoint_that_another_process_holds() {
    let dir = support::test_dir();
    let endpoint = support::test_endpoint(&dir);
    let _holder = Listener::bind(&endpoint).unwrap();

    let output = support::run_daemon(&dir, &endpoint);
    std::fs::remove_dir_all(&dir).ok();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("another program already holds the endpoint"),
        "{stderr}"
    );
    assert!(stderr.contains(endpoint.address()), "{stderr}");
    assert!(!stderr.contains("listening on"), "{stderr}");
}
