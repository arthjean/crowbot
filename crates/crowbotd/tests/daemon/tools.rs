//! The daemon's tools, served to an agent through the bridge (EP-001 of
//! `tasks/prd-crow-bot-harness.md`). The scripted agent starts the bridge it
//! is given as its MCP server, as a real agent does, and the daemon's test
//! tools (`test_harness.rs`) stand for one tool per effect.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use crowbot_protocol::{
    ApprovalClosure, ApprovalDecision, ApprovalReason, BotDraft, BotPatch, ClientMessage, Event,
    PermissionLevel, ServerMessage, TOOL_OUTPUT_BYTES, ToolEffect,
};
use serde_json::{Value, json};

use crate::support::{self, Client, Daemon};

const QUIET: Duration = Duration::from_millis(400);

/// The JSON a reply of the scripted agent reports after `prefix=`.
fn reported(reply: &str, prefix: &str) -> Value {
    let text = reply
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("no {prefix} in {reply}"));
    serde_json::from_str(text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

async fn mcp(client: &mut Client, conversation: i64, method: &str, params: &Value) -> Value {
    let reply = client
        .ask(conversation, &format!("mcp {method} {params}"))
        .await;
    reported(&reply, "answer=")
}

/// The text of a tool's result, and whether it failed.
fn tool_text(answer: &Value) -> (&str, bool) {
    (
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no result: {answer}")),
        answer["result"]["isError"] == true,
    )
}

async fn bot(client: &mut Client, name: &str) -> (i64, i64) {
    let bot = client.new_bot(name).await;
    let conversation = bot.current_conversation_id;
    client.open(conversation).await;
    (bot.bot.id, conversation)
}

async fn full_access(client: &mut Client, bot_id: i64) {
    client
        .update_bot(
            bot_id,
            BotPatch {
                permission_level: Some(PermissionLevel::FullAccess),
                ..BotPatch::default()
            },
        )
        .await;
    client
        .until_bot(|summary| {
            summary.bot.id == bot_id && summary.bot.permission_level == PermissionLevel::FullAccess
        })
        .await;
}

/// The events of the next turn, up to its end.
async fn turn_events(client: &mut Client, conversation: i64, text: &str) -> Vec<Event> {
    client.say(conversation, text).await;
    let mut events = Vec::new();
    loop {
        if let ServerMessage::ConversationEvent {
            conversation_id,
            event,
            ..
        } = client.next().await
            && conversation_id == conversation
        {
            let ended = event.ends_turn();
            events.push(event);
            if ended {
                return events;
            }
        }
    }
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

fn the_server(config: &Value) -> &Value {
    let servers = config.as_array().unwrap();
    assert_eq!(servers.len(), 1, "{config}");
    &servers[0]
}

/// Starts the bridge as an agent does, with the arguments a session gave.
fn bridge(server: &Value) -> std::process::Child {
    let args: Vec<&str> = server["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    Command::new(server["command"].as_str().unwrap())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Sends one request to a bridge and reads its answer, off the runtime.
async fn ask_bridge(child: &mut std::process::Child, request: Value) -> Option<Value> {
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (stdin, stdout, answer) = tokio::task::spawn_blocking(move || {
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let read = reader.read_line(&mut line).unwrap_or(0);
        let answer = (read > 0).then(|| serde_json::from_str::<Value>(&line).unwrap());
        (stdin, reader.into_inner(), answer)
    })
    .await
    .unwrap();
    child.stdin = Some(stdin);
    child.stdout = Some(stdout);
    answer
}

// US-002: the bridge and the per-session endpoint.

#[tokio::test]
async fn every_session_carries_the_bridge_and_the_agent_reaches_the_daemon_through_it() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation) = bot(&mut client, "Tools").await;

    let reply = client.ask(conversation, "mcpconfig").await;
    let config = reported(&reply, "mcp=");
    let server = the_server(&config);
    assert_eq!(server["name"], "crowbot");
    let program = server["command"].as_str().unwrap();
    assert!(std::path::Path::new(program).is_absolute(), "{program}");
    assert_eq!(server["args"][0], "--mcp-bridge", "{server}");

    let initialized = mcp(
        &mut client,
        conversation,
        "initialize",
        &json!({ "protocolVersion": "2025-06-18" }),
    )
    .await;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "crowbot");
    let listed = mcp(&mut client, conversation, "tools/list", &json!({})).await;
    assert!(
        listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "probe_read"),
        "{listed}"
    );
    let called = mcp(
        &mut client,
        conversation,
        "tools/call",
        &json!({ "name": "probe_read", "arguments": {} }),
    )
    .await;
    assert_eq!(
        tool_text(&called),
        (
            format!("bot={bot_id} conversation={conversation}").as_str(),
            false
        )
    );

    // A session the daemon resumes carries the bridge too, to an endpoint of
    // its own.
    drop(client);
    daemon.kill();
    daemon.spawn();
    let mut client = daemon.client().await;
    client.open(conversation).await;
    let reply = client.ask(conversation, "session").await;
    assert!(reply.contains("resumed=true"), "{reply}");
    let resumed = reported(&client.ask(conversation, "mcpconfig").await, "mcp=");
    let resumed_server = the_server(&resumed);
    assert_eq!(resumed_server["args"][0], "--mcp-bridge");
    assert_ne!(resumed_server["args"][1], server["args"][1]);
    let called = mcp(
        &mut client,
        conversation,
        "tools/call",
        &json!({ "name": "probe_read" }),
    )
    .await;
    assert!(tool_text(&called).0.starts_with("bot="), "{called}");
}

#[tokio::test]
async fn the_calls_of_two_bots_are_each_attributed_to_its_own_bot_and_conversation() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (first_bot, first) = bot(&mut client, "First").await;
    let (second_bot, second) = bot(&mut client, "Second").await;
    let call = json!({ "name": "probe_read", "arguments": {} });
    let one = mcp(&mut client, first, "tools/call", &call).await;
    let two = mcp(&mut client, second, "tools/call", &call).await;
    assert_eq!(
        tool_text(&one).0,
        format!("bot={first_bot} conversation={first}")
    );
    assert_eq!(
        tool_text(&two).0,
        format!("bot={second_bot} conversation={second}")
    );
}

#[tokio::test]
async fn a_bridge_of_a_session_that_ended_gets_an_error_and_no_tool_runs() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation) = bot(&mut client, "Ended").await;
    let old = reported(&client.ask(conversation, "mcpconfig").await, "mcp=");
    let old_server = the_server(&old).clone();

    // New instructions close the session at the next message.
    client
        .update_bot(
            bot_id,
            BotPatch {
                instructions: Some("Be brief.".to_owned()),
                ..BotPatch::default()
            },
        )
        .await;
    client
        .until_bot(|summary| summary.bot.id == bot_id && summary.bot.instructions == "Be brief.")
        .await;
    let new = reported(&client.ask(conversation, "mcpconfig").await, "mcp=");
    assert_ne!(the_server(&new)["args"][1], old_server["args"][1]);

    let mut stale = bridge(&old_server);
    let answer = ask_bridge(
        &mut stale,
        json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "probe_state", "arguments": { "text": "late" } } }),
    )
    .await
    .expect("the bridge answers");
    assert_eq!(answer["id"], 7);
    assert!(answer.get("error").is_some(), "{answer}");
    assert!(!daemon.harness_file("state.log").exists());
    let ended = tokio::task::spawn_blocking(move || stale.wait().unwrap())
        .await
        .unwrap();
    assert!(!ended.success());
}

#[tokio::test]
async fn a_bridge_exits_when_the_daemon_is_killed() {
    let mut daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Killed").await;
    let config = reported(&client.ask(conversation, "mcpconfig").await, "mcp=");
    // A bridge of the test's own, outside the agent's process group: only
    // the end of its connection can stop it.
    let mut relay = bridge(the_server(&config));
    let answer = ask_bridge(
        &mut relay,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
    )
    .await
    .unwrap();
    assert_eq!(answer["result"], json!({}));
    drop(client);
    daemon.kill();
    let exited = support::eventually(Duration::from_secs(10), || {
        relay.try_wait().unwrap().is_some()
    });
    assert!(exited, "the bridge outlived the daemon");
}

// US-003: the tool registry and the effect of a tool.

#[tokio::test]
async fn the_list_holds_named_described_tools_with_a_schema_and_only_those_the_session_may_call() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Listed").await;
    let listed = mcp(&mut client, conversation, "tools/list", &json!({})).await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    for tool in tools {
        assert!(tool["name"].as_str().is_some_and(|name| !name.is_empty()));
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }
    let state = tools
        .iter()
        .find(|tool| tool["name"] == "probe_state")
        .unwrap();
    assert_eq!(state["inputSchema"]["properties"]["text"]["type"], "string");
    assert_eq!(state["inputSchema"]["required"], json!(["text"]));
    assert!(tools.iter().any(|tool| tool["name"] == "read_output"));
    // A tool only a review may call is not offered to a conversation.
    assert!(!tools.iter().any(|tool| tool["name"] == "probe_review"));

    let refused = mcp(
        &mut client,
        conversation,
        "tools/call",
        &json!({ "name": "probe_review" }),
    )
    .await;
    assert!(refused.get("error").is_some(), "{refused}");
    let snapshot = client.open(conversation).await;
    assert!(
        support::events(&snapshot).iter().any(|event| matches!(
            event,
            Event::ToolRefused { tool, .. } if tool == "probe_review"
        )),
        "the refusal is not recorded"
    );
}

#[tokio::test]
async fn arguments_that_do_not_fit_the_schema_name_the_first_wrong_field_and_run_nothing() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Schema").await;
    let answer = mcp(
        &mut client,
        conversation,
        "tools/call",
        &json!({ "name": "probe_state", "arguments": { "text": 3 } }),
    )
    .await;
    let (text, failed) = tool_text(&answer);
    assert!(failed);
    assert!(text.contains("`text`"), "{text}");
    let missing = mcp(
        &mut client,
        conversation,
        "tools/call",
        &json!({ "name": "probe_state", "arguments": {} }),
    )
    .await;
    assert!(tool_text(&missing).0.contains("`text`"), "{missing}");
    assert!(!daemon.harness_file("state.log").exists());
}

#[tokio::test]
async fn a_call_to_a_daemon_tool_is_stored_with_the_tools_effect() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Effect").await;
    let events = turn_events(&mut client, conversation, "mcpcall probe_read {}").await;
    let effect = events.iter().find_map(|event| match event {
        Event::ToolCallStarted { title, effect, .. } if title.contains("probe_read") => {
            Some(*effect)
        }
        _ => None,
    });
    assert_eq!(effect, Some(Some(ToolEffect::Read)), "{events:?}");
    let events = turn_events(
        &mut client,
        conversation,
        r#"mcpcall probe_state {"text":"kept"}"#,
    )
    .await;
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ToolCallStarted {
            effect: Some(ToolEffect::BotState),
            ..
        }
    )));
    // The stored event keeps it.
    let snapshot = client.open(conversation).await;
    assert!(support::events(&snapshot).iter().any(|event| matches!(
        event,
        Event::ToolCallStarted {
            effect: Some(ToolEffect::Read),
            ..
        }
    )));
    // A shell command is no daemon tool.
    let events = turn_events(&mut client, conversation, "work").await;
    assert!(events.iter().all(|event| !matches!(
        event,
        Event::ToolCallStarted {
            effect: Some(_),
            ..
        }
    )));
}

#[tokio::test]
async fn a_long_result_comes_as_a_preview_with_a_cursor_to_the_rest() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Long").await;
    let bytes = TOOL_OUTPUT_BYTES * 2;
    let first = mcp(
        &mut client,
        conversation,
        "tools/call",
        &json!({ "name": "probe_big", "arguments": { "bytes": bytes } }),
    )
    .await;
    let (preview, failed) = tool_text(&first);
    assert!(!failed);
    assert!(preview.len() <= TOOL_OUTPUT_BYTES, "{}", preview.len());
    let mut read = preview.split("\n\n[").next().unwrap().len();
    let mut page = preview.to_owned();
    while let Some(cursor) = page.split("cursor \"").nth(1) {
        let cursor = cursor.split('"').next().unwrap().to_owned();
        let next = mcp(
            &mut client,
            conversation,
            "tools/call",
            &json!({ "name": "read_output", "arguments": { "cursor": cursor } }),
        )
        .await;
        page = tool_text(&next).0.to_owned();
        assert!(page.len() <= TOOL_OUTPUT_BYTES);
        read += page.split("\n\n[").next().unwrap().len();
    }
    assert_eq!(read, bytes);
}

// US-004: approvals of daemon tools.

#[tokio::test]
async fn a_request_that_names_the_daemons_server_is_allowed_as_a_daemon_tool() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Named").await;
    let events = turn_events(&mut client, conversation, "mcpcall probe_read {}").await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::ApprovalResolved {
                decision: ApprovalDecision::Allowed,
                reason: Some(ApprovalReason::DaemonTool),
                ..
            }
        )),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ApprovalRequested { .. }))
    );
    assert!(reply_of(&events).starts_with("call=bot="), "{events:?}");
}

#[tokio::test]
async fn read_and_bot_state_tools_run_with_no_approval_request() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Quiet").await;
    for text in [
        "mcpcall probe_read {}",
        r#"mcpcall probe_state {"text":"remembered"}"#,
    ] {
        let events = turn_events(&mut client, conversation, text).await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::ApprovalRequested { .. })),
            "{events:?}"
        );
        assert!(reply_of(&events).contains("failed=false"), "{events:?}");
    }
    let state = std::fs::read_to_string(daemon.harness_file("state.log")).unwrap();
    assert!(state.contains("remembered"), "{state}");
}

#[tokio::test]
async fn an_external_tool_on_ask_first_waits_for_a_person_and_runs_once_allowed() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Asks").await;
    client
        .say(conversation, r#"mcpcall probe_external {"text":"hello"}"#)
        .await;
    let Event::ApprovalRequested {
        id, title, target, ..
    } = client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
    else {
        unreachable!()
    };
    assert!(title.unwrap().contains("probe_external"));
    assert!(target.unwrap().contains("hello"));
    // The call does not return before a client answers.
    while let Some(message) = client.next_within(QUIET).await {
        assert!(
            !matches!(
                &message,
                ServerMessage::ConversationEvent {
                    event: Event::MessageText(_),
                    ..
                }
            ),
            "{message:?}"
        );
    }
    assert!(!daemon.harness_file("external.log").exists());
    client
        .send(ClientMessage::ResolveApproval {
            conversation_id: conversation,
            approval_id: id,
            decision: ApprovalDecision::Allowed,
        })
        .await;
    client.until_reply("call=sent failed=false").await;
    let sent = std::fs::read_to_string(daemon.harness_file("external.log")).unwrap();
    assert!(sent.contains("hello"), "{sent}");
}

#[tokio::test]
async fn an_external_tool_on_full_access_runs_and_says_why() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation) = bot(&mut client, "Full").await;
    full_access(&mut client, bot_id).await;
    let events = turn_events(
        &mut client,
        conversation,
        r#"mcpcall probe_external {"text":"go"}"#,
    )
    .await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::ApprovalResolved {
                decision: ApprovalDecision::Allowed,
                reason: Some(ApprovalReason::PermissionLevel),
                title: Some(title),
                ..
            } if title.contains("probe_external")
        )),
        "{events:?}"
    );
    assert!(reply_of(&events).contains("call=sent"), "{events:?}");
    assert!(daemon.harness_file("external.log").exists());
}

#[tokio::test]
async fn a_declined_external_tool_tells_the_agent_a_person_declined_and_changes_nothing() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Declined").await;
    client
        .say(conversation, r#"mcpcall probe_external {"text":"no"}"#)
        .await;
    let Event::ApprovalRequested { id, .. } = client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
    else {
        unreachable!()
    };
    client
        .send(ClientMessage::ResolveApproval {
            conversation_id: conversation,
            approval_id: id,
            decision: ApprovalDecision::Declined,
        })
        .await;
    let reply = client
        .until_event(|event| matches!(event, Event::MessageText(text) if text.starts_with("call=")))
        .await;
    let Event::MessageText(reply) = reply else {
        unreachable!()
    };
    assert!(reply.contains("A person declined"), "{reply}");
    assert!(reply.ends_with("failed=true"), "{reply}");
    assert!(!daemon.harness_file("external.log").exists());
}

#[tokio::test]
async fn a_stop_while_an_external_tool_waits_cancels_it_and_the_tool_does_not_run() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Stopped").await;
    client
        .say(conversation, r#"mcpcall probe_external {"text":"stop"}"#)
        .await;
    let Event::ApprovalRequested { id, .. } = client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
    else {
        unreachable!()
    };
    client
        .send(ClientMessage::StopTurn {
            conversation_id: conversation,
        })
        .await;
    let closed = client
        .until_event(|event| matches!(event, Event::ApprovalClosed { .. }))
        .await;
    assert!(
        matches!(&closed, Event::ApprovalClosed { id: closed, reason: ApprovalClosure::Cancelled, .. } if *closed == id),
        "{closed:?}"
    );
    client.until_turn_ends().await;
    assert!(!daemon.harness_file("external.log").exists());
}

#[tokio::test]
async fn a_request_that_names_the_server_only_in_its_title_waits_for_a_person() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Anonymous").await;
    client.say(conversation, "mcpanon probe_read {}").await;
    let Event::ApprovalRequested { id, title, .. } = client
        .until_event(|event| {
            matches!(
                event,
                Event::ApprovalRequested { .. } | Event::ApprovalResolved { .. }
            )
        })
        .await
    else {
        panic!("the request was answered without a person");
    };
    assert_eq!(title.as_deref(), Some("mcp__crowbot__probe_read"));
    client
        .send(ClientMessage::ResolveApproval {
            conversation_id: conversation,
            approval_id: id,
            decision: ApprovalDecision::Allowed,
        })
        .await;
    client.until_turn_ends().await;
}

/// A name in the daemon's form for a tool the daemon does not serve belongs
/// to another server, whose name starts like the daemon's: it asks.
#[tokio::test]
async fn a_request_for_a_tool_the_daemon_does_not_serve_waits_for_a_person() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Unserved").await;
    client.say(conversation, "mcpcall nothing__here {}").await;
    let event = client
        .until_event(|event| {
            matches!(
                event,
                Event::ApprovalRequested { .. } | Event::ApprovalResolved { .. }
            )
        })
        .await;
    let Event::ApprovalRequested { id, .. } = event else {
        panic!("the request was answered without a person: {event:?}");
    };
    client
        .send(ClientMessage::ResolveApproval {
            conversation_id: conversation,
            approval_id: id,
            decision: ApprovalDecision::Declined,
        })
        .await;
    client.until_turn_ends().await;
}

// The turn gate: no daemon tool runs between turns, and no change of one is
// applied once a stop is asked (`tools.rs`).

/// The daemon's server of the session, read in a turn that has ended.
async fn server_after_a_turn(client: &mut Client, conversation: i64) -> Value {
    let events = turn_events(client, conversation, "mcpconfig").await;
    the_server(&reported(&reply_of(&events), "mcp=")).clone()
}

fn tools_call(id: u64, name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
}

#[tokio::test]
async fn a_daemon_tool_called_between_turns_does_not_run_and_the_refusal_is_recorded() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Between").await;
    let server = server_after_a_turn(&mut client, conversation).await;
    let mut relay = bridge(&server);
    let calls = [
        tools_call(1, "probe_state", json!({ "text": "between" })),
        tools_call(2, "probe_read", json!({})),
    ];
    for call in calls {
        let answer = ask_bridge(&mut relay, call).await.unwrap();
        assert!(answer.get("error").is_some(), "{answer}");
    }
    assert!(!daemon.harness_file("state.log").exists());
    let snapshot = client.open(conversation).await;
    assert!(
        support::events(&snapshot).iter().any(|event| matches!(
            event,
            Event::ToolRefused { tool, .. } if tool == "probe_state"
        )),
        "the refusal is not recorded"
    );
    relay.kill().ok();
}

#[tokio::test]
async fn a_change_whose_turn_is_stopped_before_it_is_applied_changes_nothing() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Stopping").await;
    let server = server_after_a_turn(&mut client, conversation).await;
    // A turn whose agent ignores the cancel, so that the stop waits.
    client.say(conversation, "deaf").await;
    client.until_reply("deaf").await;
    let mut relay = bridge(&server);

    // In the turn, a change is applied, and names the turn it was made in.
    let kept = ask_bridge(
        &mut relay,
        tools_call(1, "probe_state", json!({ "text": "inside" })),
    )
    .await
    .unwrap();
    assert_eq!(tool_text(&kept), ("kept", false));

    // A change made before the stop and handed over after it.
    let late = tools_call(
        2,
        "probe_state",
        json!({ "text": "late", "delay_ms": 1500 }),
    );
    let pending = tokio::spawn(async move {
        let answer = ask_bridge(&mut relay, late).await;
        (relay, answer)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    client
        .send(ClientMessage::StopTurn {
            conversation_id: conversation,
        })
        .await;
    let (mut relay, answer) = pending.await.unwrap();
    let answer = answer.unwrap();
    let (text, failed) = tool_text(&answer);
    assert!(failed, "{answer}");
    assert!(text.contains("stopped"), "{text}");
    let state = std::fs::read_to_string(daemon.harness_file("state.log")).unwrap();
    assert!(state.contains("turn=2 inside"), "{state}");
    assert!(!state.contains("late"), "{state}");

    // While the stop waits for the agent, no tool starts.
    let refused = ask_bridge(&mut relay, tools_call(3, "probe_read", json!({})))
        .await
        .unwrap();
    assert!(refused.get("error").is_some(), "{refused}");
    relay.kill().ok();
}

/// Calls race a stop on bridges of their own: a change the agent was told
/// is kept is in the bot's state, one it was told is not is absent, every
/// change names the turn that ran, and none handed over well after the stop
/// is applied. `scripts/stress-tests.sh` runs it many times at once.
#[tokio::test]
async fn changes_that_race_a_stop_are_applied_exactly_when_the_agent_is_told_so() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Race").await;
    let server = server_after_a_turn(&mut client, conversation).await;
    client.say(conversation, "deaf").await;
    client.until_reply("deaf").await;
    let delays: Vec<u64> = (0..8).map(|index| index * 150).collect();
    let mut calls: Vec<_> = delays
        .iter()
        .map(|&delay| {
            let mut relay = bridge(&server);
            let call = tools_call(
                delay,
                "probe_state",
                json!({ "text": format!("delay-{delay}"), "delay_ms": delay }),
            );
            tokio::spawn(async move {
                let answer = ask_bridge(&mut relay, call).await;
                relay.kill().ok();
                (delay, answer)
            })
        })
        .collect();
    // The change made at once is answered in the turn, however slow the
    // computer; the stop comes after it, while the others still work.
    let mut answers = vec![calls.remove(0).await.unwrap()];
    tokio::time::sleep(Duration::from_millis(300)).await;
    client
        .send(ClientMessage::StopTurn {
            conversation_id: conversation,
        })
        .await;
    for call in calls {
        answers.push(call.await.unwrap());
    }
    let state = std::fs::read_to_string(daemon.harness_file("state.log")).unwrap_or_default();
    for (delay, answer) in answers {
        let answer = answer.unwrap();
        let kept = answer["result"]["isError"] == false;
        let line = format!("turn=2 delay-{delay}\n");
        assert_eq!(
            kept,
            state.contains(&line),
            "delay {delay}: {answer} / {state}"
        );
        if delay == 0 {
            assert!(kept, "a change made at once in the turn is kept: {answer}");
        }
        if delay == delays[delays.len() - 1] {
            assert!(
                !kept,
                "a change handed over after the stop is not kept: {answer}"
            );
        }
    }
    assert!(
        state.lines().all(|line| line.contains(" turn=2 ")),
        "{state}"
    );
}

#[tokio::test]
async fn a_request_for_a_daemon_tool_while_the_turn_stops_is_cancelled() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation) = bot(&mut client, "Late").await;
    client
        .say(conversation, r#"mcplate probe_state {"text":"late"}"#)
        .await;
    client.until_reply("waiting").await;
    client
        .send(ClientMessage::StopTurn {
            conversation_id: conversation,
        })
        .await;
    let mut events = Vec::new();
    loop {
        if let ServerMessage::ConversationEvent {
            conversation_id,
            event,
            ..
        } = client.next().await
            && conversation_id == conversation
        {
            let ended = event.ends_turn();
            events.push(event);
            if ended {
                break;
            }
        }
    }
    assert!(
        !events.iter().any(|event| matches!(
            event,
            Event::ApprovalResolved {
                reason: Some(ApprovalReason::DaemonTool),
                ..
            }
        )),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::ApprovalClosed {
                reason: ApprovalClosure::Cancelled,
                ..
            }
        )),
        "{events:?}"
    );
    assert!(
        reply_of(&events).contains("permission=cancelled"),
        "{events:?}"
    );
    assert!(!daemon.harness_file("state.log").exists());
}

/// A real turn on each signed-in provider calls a daemon tool through the
/// bridge (`docs/harness-providers.md`). It spends the user's own usage, so
/// it runs only on request, with `CROWBOT_REAL_PROVIDERS` naming the data
/// folder of the user's daemon, and `CROWBOT_CLAUDE_EXE` the Claude agent
/// when that folder holds no Claude runtime:
/// `cargo test -p crowbotd --test daemon real_providers -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "runs a real turn on each signed-in provider, with the user's own sign-ins"]
async fn real_providers_call_a_daemon_tool_through_the_bridge() {
    if std::env::var_os(support::REAL_PROVIDERS).is_none() {
        eprintln!("skipped: {} is not set", support::REAL_PROVIDERS);
        return;
    }
    let daemon = Daemon::start_with_real_providers();
    let mut client = daemon.client().await;
    if let Ok(claude) = std::env::var("CROWBOT_CLAUDE_EXE") {
        client
            .send(ClientMessage::SetProviderExecutable {
                provider: "claude".to_owned(),
                path: Some(claude),
            })
            .await;
    }
    // Grok wants its sign-in named before each session, which the daemon
    // does once a sign-in it started ended well: with the sign-in its home
    // holds, that one ends at once.
    client
        .send(ClientMessage::CheckProvider {
            provider: "grok".to_owned(),
        })
        .await;
    while let Some(message) = client.next_within(Duration::from_secs(120)).await {
        if let ServerMessage::ProviderChanged(status) = message
            && status.id == "grok"
            && !status.checking
            && status.state != crowbot_protocol::ProviderState::Unknown
        {
            println!("grok checked: {:?}", status.state);
            break;
        }
    }
    client
        .send(ClientMessage::SignIn {
            provider: "grok".to_owned(),
            method: None,
        })
        .await;
    let mut started = false;
    while let Some(message) = client.next_within(Duration::from_secs(120)).await {
        if let ServerMessage::ProviderChanged(status) = &message
            && status.id == "grok"
        {
            println!(
                "grok: {:?} signing_in={} checking={} page={}",
                status.state,
                status.signing_in,
                status.checking,
                status.sign_in_url.is_some()
            );
            started |= status.signing_in;
            if started && !status.signing_in && !status.checking {
                break;
            }
        } else if let ServerMessage::Error { message } = &message {
            println!("grok: the sign-in was refused: {message}");
            break;
        }
    }
    let mut ran = Vec::new();
    let only = std::env::var("CROWBOT_REAL_PROVIDER").ok();
    for provider in ["claude", "codex", "grok"] {
        if only.as_deref().is_some_and(|only| only != provider) {
            continue;
        }
        let bot = client
            .create_bot(BotDraft {
                name: format!("Real {provider}"),
                provider: Some(provider.to_owned()),
                ..BotDraft::default()
            })
            .await;
        let conversation = bot.current_conversation_id;
        client.open(conversation).await;
        client
            .say(
                conversation,
                "Call the tool probe_read of the MCP server named crowbot, then tell me exactly what it returned.",
            )
            .await;
        let mut events = Vec::new();
        loop {
            match client.next_within_minutes().await {
                ServerMessage::ConversationEvent {
                    conversation_id,
                    event,
                    ..
                } if conversation_id == conversation => {
                    let ended = event.ends_turn();
                    events.push(event);
                    if ended {
                        break;
                    }
                }
                _ => {}
            }
        }
        let effect = events.iter().find_map(|event| match event {
            Event::ToolCallStarted {
                effect: Some(effect),
                ..
            } => Some(*effect),
            _ => None,
        });
        let daemon_tool = events.iter().any(|event| {
            matches!(
                event,
                Event::ApprovalResolved {
                    reason: Some(ApprovalReason::DaemonTool),
                    ..
                }
            )
        });
        let asked = events
            .iter()
            .any(|event| matches!(event, Event::ApprovalRequested { .. }));
        let answer = reply_of(&events);
        let end = events.last().cloned();
        println!(
            "{provider}: effect={effect:?} daemon_tool_approval={daemon_tool} person_asked={asked} end={end:?} answer={answer:?}"
        );
        if let Some(Event::TurnFailed { message, .. }) = &end {
            println!("{provider}: not observed, the turn failed: {message}");
            continue;
        }
        assert_eq!(effect, Some(ToolEffect::Read), "{provider}: {events:?}");
        assert!(!asked, "{provider} asked a person for a Read tool");
        assert!(
            answer.contains(&format!("bot={}", bot.bot.id)),
            "{provider}: {answer}"
        );
        ran.push(provider);
    }
    println!("ran through the bridge: {ran:?}");
    assert!(!ran.is_empty());
}
