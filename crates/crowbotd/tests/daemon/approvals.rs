//! EP-003 of `tasks/prd-crow-bot-harness.md`: what answers an approval
//! without a person. The built-in floors, the rules a person gives a bot,
//! the cut request that nobody may allow, and the answer that is stored
//! before the agent gets it.

use std::path::Path;
use std::time::Duration;

use crowbot_protocol::{
    APPROVAL_TARGET_BYTES, ApprovalDecision, ApprovalReason, ApprovalRule, BotDraft, BotPatch,
    ClientMessage, Event, PermissionLevel, RuleDraft, RuleKind, ServerMessage, TurnEnd,
};

use crate::support::{self, Client, Daemon};

const QUIET: Duration = Duration::from_millis(500);

async fn bot(client: &mut Client, level: PermissionLevel) -> (i64, i64, String) {
    let summary = client
        .create_bot(BotDraft {
            name: "Atlas".to_owned(),
            permission_level: Some(level),
            ..BotDraft::default()
        })
        .await;
    let conversation = summary.current_conversation_id;
    client.open(conversation).await;
    (summary.bot.id, conversation, summary.bot.workspace)
}

/// The events of one turn, up to its end.
async fn turn(client: &mut Client, conversation_id: i64, text: &str) -> Vec<Event> {
    client.say(conversation_id, text).await;
    let mut events = Vec::new();
    loop {
        if let ServerMessage::ConversationEvent {
            conversation_id: about,
            event,
            ..
        } = client.next().await
            && about == conversation_id
        {
            let ended = event.ends_turn();
            events.push(event);
            if ended {
                return events;
            }
        }
    }
}

fn reply(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Event::MessageText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The answers the daemon gave in a turn, with no person.
fn resolutions(events: &[Event]) -> Vec<(ApprovalDecision, Option<ApprovalReason>)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ApprovalResolved {
                decision, reason, ..
            } => Some((*decision, *reason)),
            _ => None,
        })
        .collect()
}

fn asked(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::ApprovalRequested { .. }))
}

async fn waiting(client: &mut Client) -> (String, Option<String>) {
    match client
        .until_event(|event| matches!(event, Event::ApprovalRequested { .. }))
        .await
    {
        Event::ApprovalRequested { id, target, .. } => (id, target),
        _ => unreachable!(),
    }
}

async fn resolved(client: &mut Client) -> (ApprovalDecision, Option<ApprovalReason>) {
    match client
        .until_event(|event| matches!(event, Event::ApprovalResolved { .. }))
        .await
    {
        Event::ApprovalResolved {
            decision, reason, ..
        } => (decision, reason),
        _ => unreachable!(),
    }
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

async fn rules_of(client: &mut Client, bot_id: i64) -> Vec<ApprovalRule> {
    client
        .until(|message| match message {
            ServerMessage::ApprovalRules { bot_id: of, rules } if of == bot_id => Some(rules),
            _ => None,
        })
        .await
}

/// Gives the bot a rule through the client message, the only way one is made.
async fn add_rule(
    client: &mut Client,
    bot_id: i64,
    kind: RuleKind,
    program: &str,
    args: &[&str],
) -> i64 {
    client
        .send(ClientMessage::AddApprovalRule {
            bot_id,
            rule: RuleDraft {
                kind,
                program: program.to_owned(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            },
        })
        .await;
    let rules = rules_of(client, bot_id).await;
    let id = client
        .until(|message| match message {
            ServerMessage::ApprovalRuleAdded {
                bot_id: of,
                rule_id,
            } if of == bot_id => Some(rule_id),
            _ => None,
        })
        .await;
    assert!(rules.iter().any(|rule| rule.id == id));
    id
}

async fn listed_rules(client: &mut Client, bot_id: i64) -> Vec<ApprovalRule> {
    client
        .send(ClientMessage::ListApprovalRules { bot_id })
        .await;
    rules_of(client, bot_id).await
}

fn in_folder(folder: &str, command: &str) -> String {
    format!("askin {folder} {command}")
}

// US-012: a cut request is never allowed.

#[tokio::test]
async fn full_access_does_not_allow_a_cut_request_and_a_person_can_only_decline_it() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation, _) = bot(&mut client, PermissionLevel::FullAccess).await;

    client
        .say(
            conversation,
            &format!("asklong {}", APPROVAL_TARGET_BYTES + 1),
        )
        .await;
    let (approval, _) = waiting(&mut client).await;
    let snapshot = client.open(conversation).await;
    assert_eq!(snapshot.pending_approvals, [approval.as_str()]);
    assert!(
        support::events(&snapshot)
            .iter()
            .any(|event| matches!(event, Event::ApprovalRequested { cut: true, .. }))
    );
    assert!(
        !support::events(&snapshot)
            .iter()
            .any(|event| matches!(event, Event::ApprovalResolved { .. }))
    );

    answer(&mut client, conversation, &approval, true).await;
    let (_, _, why) = client.until_not_resolved().await;
    assert!(why.contains("only be declined"), "{why}");
    answer(&mut client, conversation, &approval, false).await;
    assert_eq!(
        resolved(&mut client).await,
        (ApprovalDecision::Declined, None)
    );
    client.until_reply("outcome=reject").await;
    client.until_turn_ends().await;

    // A request just under the cap is read in full, and allowed as before.
    let events = turn(
        &mut client,
        conversation,
        &format!("asklong {APPROVAL_TARGET_BYTES}"),
    )
    .await;
    assert_eq!(
        resolutions(&events),
        [(
            ApprovalDecision::Allowed,
            Some(ApprovalReason::PermissionLevel)
        )]
    );
    assert_eq!(reply(&events), "outcome=allow");
}

// US-014: built-in refusals.

#[cfg(unix)]
#[tokio::test]
async fn full_access_declines_what_a_floor_refuses_without_asking() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, workspace) = bot(&mut client, PermissionLevel::FullAccess).await;
    let data = daemon.data_dir().display().to_string();
    let program = std::fs::canonicalize(env!("CARGO_BIN_EXE_crowbotd")).unwrap();
    // A floor wins over an allow rule that matches the same command.
    add_rule(&mut client, bot_id, RuleKind::Allow, "rm", &["-rf"]).await;
    for command in [
        "ask rm -rf /".to_owned(),
        "ask rm -rf /etc".to_owned(),
        "ask sudo rm -r --force /usr".to_owned(),
        "ask sh -c 'rm -rf /'".to_owned(),
        "ask $'r\\x6d' -rf /home".to_owned(),
        "ask rm -rfvvv /etc".to_owned(),
        "ask bash -c -- 'rm -rf /'".to_owned(),
        "ask mkfs.ext4 /dev/sda1".to_owned(),
        "ask dd if=/dev/zero of=/dev/sda bs=1M".to_owned(),
        "ask shutdown -h now".to_owned(),
        "ask systemctl reboot".to_owned(),
        format!("ask rm -rf {data}"),
        format!("ask rm {}", program.display()),
        in_folder(&workspace, "rm -rf /"),
    ] {
        let events = turn(&mut client, conversation, &command).await;
        assert!(!asked(&events), "{command}: {events:?}");
        assert_eq!(
            resolutions(&events),
            [(ApprovalDecision::Declined, Some(ApprovalReason::Floor))],
            "{command}"
        );
        assert_eq!(reply(&events), "outcome=reject", "{command}");
    }
    // A delete that leaves them whole is allowed as before.
    let events = turn(&mut client, conversation, "ask rm -rf build").await;
    assert_eq!(
        resolutions(&events),
        [(
            ApprovalDecision::Allowed,
            Some(ApprovalReason::PermissionLevel)
        )]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_opaque_command_that_names_a_floor_program_waits_for_a_person_on_full_access() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, _) = bot(&mut client, PermissionLevel::FullAccess).await;

    client.say(conversation, "ask cd / && rm -rf etc").await;
    let (approval, target) = waiting(&mut client).await;
    assert_eq!(target.as_deref(), Some("cd / && rm -rf etc"));
    assert_eq!(client.next_within(QUIET).await, None);

    // Raising or keeping the level answers nothing: it still waits.
    client
        .update_bot(
            bot_id,
            BotPatch {
                permission_level: Some(PermissionLevel::FullAccess),
                ..BotPatch::default()
            },
        )
        .await;
    let snapshot = client.open(conversation).await;
    assert_eq!(snapshot.pending_approvals, [approval.as_str()]);

    answer(&mut client, conversation, &approval, true).await;
    assert_eq!(
        resolved(&mut client).await,
        (ApprovalDecision::Allowed, None)
    );
    client.until_reply("outcome=allow").await;
}

// US-015: per-bot rules.

#[tokio::test]
async fn a_rule_is_stored_from_a_client_message_with_its_bot_kind_program_and_arguments() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let mut other = daemon.client().await;
    let (bot_id, _, _) = bot(&mut client, PermissionLevel::AskFirst).await;

    let id = add_rule(
        &mut client,
        bot_id,
        RuleKind::Allow,
        "cargo",
        &["test", "--locked"],
    )
    .await;
    // Every attached client hears of it.
    let seen = rules_of(&mut other, bot_id).await;
    let [rule] = seen.as_slice() else {
        panic!("one rule expected, got {seen:?}");
    };
    assert_eq!(rule.id, id);
    assert_eq!(rule.bot_id, bot_id);
    assert_eq!(rule.kind, RuleKind::Allow);
    assert_eq!(rule.program, "cargo");
    assert_eq!(rule.args, ["test", "--locked"]);

    // It survives a restart of the daemon.
    drop(client);
    drop(other);
    let mut daemon = daemon;
    daemon.kill();
    daemon.spawn();
    let mut client = daemon.client().await;
    assert_eq!(listed_rules(&mut client, bot_id).await, seen);

    // A rule is one program with no space, and a few arguments.
    client
        .send(ClientMessage::AddApprovalRule {
            bot_id,
            rule: RuleDraft {
                kind: RuleKind::Allow,
                program: "git status".to_owned(),
                args: Vec::new(),
            },
        })
        .await;
    assert_eq!(client.until_invalid().await, "program");
    client
        .send(ClientMessage::DeleteApprovalRule { rule_id: id })
        .await;
    assert!(rules_of(&mut client, bot_id).await.is_empty());
}

#[tokio::test]
async fn a_refuse_rule_declines_on_full_access_even_with_no_folder() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, workspace) = bot(&mut client, PermissionLevel::FullAccess).await;
    let refuse = add_rule(&mut client, bot_id, RuleKind::Refuse, "git", &["push"]).await;
    add_rule(&mut client, bot_id, RuleKind::Allow, "git", &["push"]).await;

    for command in [
        "ask git push".to_owned(),
        "ask git push origin main".to_owned(),
        "ask /usr/bin/git -C elsewhere push".to_owned(),
        // Both rules match: the refusal wins.
        in_folder(&workspace, "git push"),
    ] {
        let events = turn(&mut client, conversation, &command).await;
        assert!(!asked(&events), "{command}");
        assert_eq!(
            resolutions(&events),
            [(
                ApprovalDecision::Declined,
                Some(ApprovalReason::Rule { id: refuse })
            )],
            "{command}"
        );
        assert_eq!(reply(&events), "outcome=reject");
    }
    let events = turn(&mut client, conversation, "ask git status").await;
    assert_eq!(
        resolutions(&events),
        [(
            ApprovalDecision::Allowed,
            Some(ApprovalReason::PermissionLevel)
        )]
    );
}

#[tokio::test]
async fn an_allow_rule_answers_a_simple_command_in_the_bots_folder_on_ask_first() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, workspace) = bot(&mut client, PermissionLevel::AskFirst).await;
    let cargo = add_rule(&mut client, bot_id, RuleKind::Allow, "cargo", &["test"]).await;
    add_rule(&mut client, bot_id, RuleKind::Allow, "git", &["status"]).await;
    let sub = Path::new(&workspace).join("crate");
    std::fs::create_dir_all(&sub).unwrap();

    for command in [
        in_folder(&workspace, "cargo test"),
        in_folder(&workspace, "cargo test --locked -p crowbotd"),
        in_folder(&sub.display().to_string(), "cargo test"),
    ] {
        let events = turn(&mut client, conversation, &command).await;
        assert!(!asked(&events), "{command}");
        assert_eq!(
            resolutions(&events),
            [(
                ApprovalDecision::Allowed,
                Some(ApprovalReason::Rule { id: cargo })
            )],
            "{command}"
        );
        assert_eq!(reply(&events), "outcome=allow");
    }

    let outside = daemon.folder("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let escape = Path::new(&workspace).join("crate/../..");
    for command in [
        // A pipe, a list, an expansion: opaque.
        in_folder(&workspace, "cargo test | tee log"),
        in_folder(&workspace, "cargo test && git push"),
        in_folder(&workspace, "cargo test $FILTER"),
        // Another subcommand, another program.
        in_folder(&workspace, "git push"),
        in_folder(&workspace, "./cargo test"),
        // No folder, or a folder outside the bot's.
        "ask cargo test".to_owned(),
        in_folder(&outside.display().to_string(), "cargo test"),
        in_folder(&escape.display().to_string(), "cargo test"),
    ] {
        client.say(conversation, &command).await;
        let (approval, _) = waiting(&mut client).await;
        assert_eq!(client.next_within(QUIET).await, None, "{command}");
        answer(&mut client, conversation, &approval, false).await;
        assert_eq!(
            client.until_turn_ends().await,
            Event::TurnEnded(TurnEnd::Completed)
        );
    }

    // A cut command never matches.
    let long = format!("cargo test {}", "x".repeat(APPROVAL_TARGET_BYTES));
    client
        .say(conversation, &in_folder(&workspace, &long))
        .await;
    let (approval, _) = waiting(&mut client).await;
    assert_eq!(client.next_within(QUIET).await, None);
    answer(&mut client, conversation, &approval, false).await;
    client.until_turn_ends().await;
}

#[tokio::test]
async fn the_rules_as_they_stand_answer_a_request_that_waits() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, _) = bot(&mut client, PermissionLevel::FullAccess).await;
    let refuse = add_rule(&mut client, bot_id, RuleKind::Refuse, "git", &["push"]).await;

    // An opaque command that names the program of a refuse rule waits,
    // until the rule is deleted: Full access then allows it.
    client.say(conversation, "ask cd sub && git push").await;
    waiting(&mut client).await;
    assert_eq!(client.next_within(QUIET).await, None);
    client
        .send(ClientMessage::DeleteApprovalRule { rule_id: refuse })
        .await;
    assert_eq!(
        resolved(&mut client).await,
        (
            ApprovalDecision::Allowed,
            Some(ApprovalReason::PermissionLevel)
        )
    );
    client.until_reply("outcome=allow").await;
    client.until_turn_ends().await;

    // A request that waits on Ask first is declined by a rule added since.
    client
        .update_bot(
            bot_id,
            BotPatch {
                permission_level: Some(PermissionLevel::AskFirst),
                ..BotPatch::default()
            },
        )
        .await;
    client.say(conversation, "ask git push").await;
    waiting(&mut client).await;
    let added = add_rule(&mut client, bot_id, RuleKind::Refuse, "git", &["push"]).await;
    assert_eq!(
        resolved(&mut client).await,
        (
            ApprovalDecision::Declined,
            Some(ApprovalReason::Rule { id: added })
        )
    );
    client.until_reply("outcome=reject").await;
}

#[tokio::test]
async fn no_daemon_tool_prompt_or_file_makes_a_rule() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, workspace) = bot(&mut client, PermissionLevel::FullAccess).await;

    // The daemon serves no tool that writes a rule.
    let listed = client.ask(conversation, r#"mcp tools/list {}"#).await;
    let listed = listed.strip_prefix("answer=").unwrap_or(&listed);
    let tools: serde_json::Value = serde_json::from_str(listed).unwrap();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(!names.is_empty());
    assert!(
        names
            .iter()
            .all(|name| !name.contains("rule") && !name.contains("approval")),
        "{names:?}"
    );
    let called = client
        .ask(
            conversation,
            r#"mcpcall add_approval_rule {"kind":"allow","program":"git","args":["push"]}"#,
        )
        .await;
    assert!(
        called.contains("no tool named add_approval_rule"),
        "{called}"
    );

    // Nor does a prompt, or a file in the bot's folder.
    std::fs::write(
        Path::new(&workspace).join("approval_rules.json"),
        r#"[{"kind":"allow","program":"git","args":["push"]}]"#,
    )
    .unwrap();
    client
        .ask(
            conversation,
            r#"say {"type":"add_approval_rule","data":{"kind":"allow","program":"git"}}"#,
        )
        .await;
    assert!(listed_rules(&mut client, bot_id).await.is_empty());
}

// US-016: an answer is stored before it is given.

/// Makes every write of an allowed approval fail, as a full disk would.
fn refuse_allowed_answers(database: &Path) {
    let connection = rusqlite::Connection::open(database).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER no_allowed_answer BEFORE INSERT ON events
             WHEN new.kind = 'approval_resolved'
                 AND json_extract(new.payload, '$.data.decision') = 'allowed'
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .unwrap();
}

#[tokio::test]
async fn an_answer_that_cannot_be_stored_declines_the_request_and_fails_the_turn() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, _) = bot(&mut client, PermissionLevel::FullAccess).await;

    // Stored first: the answer of the daemon is in the database once the
    // agent has it.
    let events = turn(&mut client, conversation, "ask Write notes.txt").await;
    assert_eq!(reply(&events), "outcome=allow");
    let stored = support::events(&client.open(conversation).await);
    assert!(stored.iter().any(|event| matches!(
        event,
        Event::ApprovalResolved {
            decision: ApprovalDecision::Allowed,
            ..
        }
    )));

    refuse_allowed_answers(&daemon.database());
    let events = turn(&mut client, conversation, "ask Write notes.txt").await;
    assert_eq!(
        resolutions(&events),
        [(
            ApprovalDecision::Declined,
            Some(ApprovalReason::NotRecorded)
        )]
    );
    assert_eq!(reply(&events), "outcome=reject");
    let Some(Event::TurnFailed { message, .. }) = events.last() else {
        panic!("the turn did not fail: {events:?}");
    };
    assert!(message.contains("store"), "{message}");

    // A person's answer goes the same way.
    client
        .update_bot(
            bot_id,
            BotPatch {
                permission_level: Some(PermissionLevel::AskFirst),
                ..BotPatch::default()
            },
        )
        .await;
    client.say(conversation, "ask Write notes.txt").await;
    let (approval, _) = waiting(&mut client).await;
    answer(&mut client, conversation, &approval, true).await;
    assert_eq!(
        resolved(&mut client).await,
        (
            ApprovalDecision::Declined,
            Some(ApprovalReason::NotRecorded)
        )
    );
    client.until_reply("outcome=reject").await;
    let Event::TurnFailed { message, .. } = client.until_turn_ends().await else {
        panic!("the turn did not fail");
    };
    assert!(message.contains("store"), "{message}");
}
