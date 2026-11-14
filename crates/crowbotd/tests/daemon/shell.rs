use crowbot_protocol::{
    Appearance, BotPatch, ClientMessage, PROTOCOL_VERSION, Preferences, PreferencesPatch,
    SendShortcut, ServerMessage,
};

use crate::support::{Client, Daemon};

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

async fn until_turn_model(client: &mut Client) -> Option<String> {
    client
        .until(|message| match message {
            ServerMessage::TurnModel { model, .. } => Some(model),
            _ => None,
        })
        .await
}

#[tokio::test]
async fn preferences_start_as_defaults_reach_every_client_and_outlive_the_daemon() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut other = daemon.client().await;
    assert_eq!(preferences(&mut client).await, Preferences::default());

    client
        .send(ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                appearance: Some(Appearance::Light),
                sidebar_width: Some(320),
                ..PreferencesPatch::default()
            },
        })
        .await;
    let expected = Preferences {
        appearance: Appearance::Light,
        sidebar_width: Some(320),
        sidebar_collapsed: false,
        send_shortcut: SendShortcut::Enter,
    };
    assert_eq!(until_preferences(&mut client).await, expected);
    assert_eq!(until_preferences(&mut other).await, expected);

    other
        .send(ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                sidebar_collapsed: Some(true),
                ..PreferencesPatch::default()
            },
        })
        .await;
    let expected = Preferences {
        sidebar_collapsed: true,
        ..expected
    };
    assert_eq!(until_preferences(&mut client).await, expected);

    drop((client, other));
    daemon.kill();
    daemon.spawn();
    let mut later = daemon.client().await;
    assert_eq!(preferences(&mut later).await, expected);
}

#[tokio::test]
async fn a_sidebar_width_out_of_range_is_refused_and_nothing_is_stored() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    client
        .send(ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                appearance: Some(Appearance::Dark),
                sidebar_width: Some(1_000_000),
                ..PreferencesPatch::default()
            },
        })
        .await;
    assert_eq!(client.until_invalid().await, "sidebar_width");
    assert_eq!(preferences(&mut client).await, Preferences::default());
}

#[tokio::test]
async fn a_bot_summary_carries_a_preview_of_its_last_message() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let bot = client.new_bot("Atlas").await;
    assert_eq!(bot.preview, "");
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;

    client.say(conversation, "say caw\ncaw").await;
    let asked = client.until_bot(|bot| !bot.preview.is_empty()).await;
    assert_eq!(asked.preview, "say caw caw");
    client.until_turn_ends().await;
    assert_eq!(client.bot(bot.bot.id).await.preview, "caw caw");
}

#[tokio::test]
async fn a_client_of_another_protocol_version_learns_the_version_of_the_daemon() {
    let daemon = Daemon::start();

    let (mut reader, mut writer) = daemon.raw_connection().await;
    writer
        .write(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION + 1,
        })
        .await
        .unwrap();
    match reader.read::<ServerMessage>().await.unwrap() {
        Some(ServerMessage::Incompatible {
            protocol_version,
            daemon_version,
        }) => {
            assert_eq!(protocol_version, PROTOCOL_VERSION);
            assert_eq!(daemon_version, env!("CARGO_PKG_VERSION"));
        }
        other => panic!("expected a refusal with the versions, got {other:?}"),
    }
    assert!(reader.read::<ServerMessage>().await.unwrap().is_none());

    let (mut reader, mut writer) = daemon.raw_connection().await;
    writer
        .write(&ClientMessage::Hello {
            protocol_version: 3,
        })
        .await
        .unwrap();
    match reader.read::<ServerMessage>().await.unwrap() {
        Some(ServerMessage::Error { message }) => {
            assert_eq!(
                message,
                format!(
                    "this daemon speaks protocol version {PROTOCOL_VERSION}, the client speaks 3"
                )
            );
        }
        other => panic!("expected the refusal an older client reads, got {other:?}"),
    }
}

#[tokio::test]
async fn a_model_changed_during_a_turn_applies_at_the_next_turn() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let bot = client.new_bot("Atlas").await;
    let conversation = bot.current_conversation_id;
    let idle = client.open(conversation).await;
    assert_eq!(idle.turn_model, None);

    client.say(conversation, "stream 40 50").await;
    assert_eq!(until_turn_model(&mut client).await, None);
    client
        .update_bot(
            bot.bot.id,
            BotPatch {
                model: Some(Some("opus".to_owned())),
                ..BotPatch::default()
            },
        )
        .await;
    let changed = client
        .until_bot(|bot| bot.bot.model.as_deref() == Some("opus"))
        .await;
    assert_eq!(changed.bot.id, bot.bot.id);

    let mut other = daemon.client().await;
    let running = other.open(conversation).await;
    assert!(running.turn_active);
    assert_eq!(running.turn_model, Some(None));

    client.until_turn_ends().await;
    client.say(conversation, "say again").await;
    assert_eq!(until_turn_model(&mut client).await.as_deref(), Some("opus"));
    client.until_turn_ends().await;
    assert_eq!(other.open(conversation).await.turn_model, None);
}
