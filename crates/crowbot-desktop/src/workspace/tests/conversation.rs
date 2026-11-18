use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crowbot_protocol::{
    ApprovalClosure, ApprovalDecision, ApprovalReason, FailureCause, Interruption, MESSAGE_LENGTH,
    PermissionLevel, QueuedMessage, SendShortcut, ToolStatus,
};
use gpui::{MouseDownEvent, MouseUpEvent, ScrollDelta, ScrollEvent};

use super::*;
use crate::theme::duration;
use crate::transcript::tests::{
    DAY_MS, at, message, request, thought, tool_ended, tool_started, user,
};
use crate::transcript::{Row, RowKind, StepState};

const CHAT: i64 = 10;

#[derive(Clone)]
struct Clock(Rc<Cell<Instant>>);

impl Clock {
    fn install(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Self {
        let clock = Self(Rc::new(Cell::new(Instant::now())));
        let time = clock.0.clone();
        workspace.update(cx, |this, _| this.clock = Rc::new(move || time.get()));
        clock
    }

    fn advance(&self, workspace: &Entity<Workspace>, cx: &mut VisualTestContext, millis: u64) {
        self.0.set(self.0.get() + Duration::from_millis(millis));
        workspace.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
}

fn start(
    cx: &mut TestAppContext,
    snapshot: ConversationSnapshot,
) -> (Entity<Workspace>, Daemon, Clock, &mut VisualTestContext) {
    let (workspace, mut daemon, cx) = open(cx);
    let mut atlas = summary(1, "Atlas", BotState::Idle);
    atlas.bot.title = "Research assistant".to_owned();
    daemon.connect(vec![atlas, summary(2, "Wren", BotState::Idle)], cx);
    daemon.sent();
    let clock = Clock::install(&workspace, cx);
    daemon.tell(ServerMessage::ConversationOpened(snapshot), cx);
    (workspace, daemon, clock, cx)
}

fn ended(events: Vec<Event>) -> ConversationSnapshot {
    snapshot(CHAT, 1, events)
}

fn running(events: Vec<Event>) -> ConversationSnapshot {
    let mut snapshot = snapshot(CHAT, 1, events);
    snapshot.turn_active = true;
    snapshot
}

fn tell(daemon: &Daemon, cx: &mut VisualTestContext, at_ms: i64, event: Event) {
    daemon.tell(
        ServerMessage::ConversationEvent {
            conversation_id: CHAT,
            event,
            at_ms,
        },
        cx,
    );
}

fn rows(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<Row> {
    workspace.read_with(cx, |this, _| {
        let open = this.open.as_ref().unwrap();
        open.transcript.rows().to_vec()
    })
}

fn kind(row: &Row) -> &'static str {
    match row.kind {
        RowKind::Day => "day",
        RowKind::User { .. } => "user",
        RowKind::Steps { .. } => "steps",
        RowKind::Answer { .. } => "answer",
        RowKind::Thought { .. } => "thought",
        RowKind::Approval(_) => "approval",
        RowKind::Status(_) => "status",
        RowKind::Notice(_) => "notice",
    }
}

fn keys(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, wanted: &str) -> Vec<i64> {
    let rows = rows(workspace, cx);
    let wanted = rows.iter().filter(|row| kind(row) == wanted);
    wanted.map(|row| row.key).collect()
}

fn key(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, wanted: &str) -> i64 {
    *keys(workspace, cx, wanted)
        .first()
        .unwrap_or_else(|| panic!("the conversation has no {wanted} row"))
}

fn of(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    prefix: &str,
    wanted: &str,
) -> Bounds<Pixels> {
    let found = key(workspace, cx, wanted);
    bounds(cx, named(prefix, found))
}

fn of_last(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    prefix: &str,
    wanted: &str,
) -> Bounds<Pixels> {
    let found = *keys(workspace, cx, wanted).last().unwrap();
    bounds(cx, named(prefix, found))
}

fn named(prefix: &str, key: i64) -> &'static str {
    Box::leak(format!("{prefix}-{key}").into_boxed_str())
}

fn step(prefix: &str, key: i64, index: usize) -> &'static str {
    Box::leak(format!("{prefix}-{key}-{index}").into_boxed_str())
}

fn wheel(cx: &mut VisualTestContext, up: f32) {
    let position = bounds(cx, "transcript").center();
    cx.simulate_event(ScrollEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(up))),
        ..Default::default()
    });
    cx.run_until_parked();
}

fn scroll_to_top(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    for _ in 0..400 {
        wheel(cx, 2_000.);
        let top = workspace.read_with(cx, |this, _| {
            this.open.as_ref().unwrap().list.logical_scroll_top()
        });
        if top.item_ix == 0 && top.offset_in_item == px(0.) {
            return;
        }
    }
    panic!("the list never reached its top");
}

fn press(cx: &mut VisualTestContext, position: Point<Pixels>, click_count: usize) {
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count,
        ..Default::default()
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count,
        ..Default::default()
    });
    cx.run_until_parked();
}

fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

fn selected(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |this, _| this.selected_text())
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

fn typed(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> String {
    cx.run_until_parked();
    workspace.read_with(cx, |this, cx| this.composer.read(cx).text().to_owned())
}

fn copy_keys() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    }
}

fn send_keys() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    }
}

fn following(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    cx.run_until_parked();
    workspace.read_with(cx, |this, _| {
        this.open.as_ref().unwrap().list.is_following_tail()
    })
}

fn long_chat(turns: usize) -> Vec<TimedEvent> {
    let mut events = Vec::new();
    for turn in 0..turns {
        let time = NOON + turn as i64 * 4_000;
        events.push(at(time, user(&format!("question {turn}"))));
        events.push(at(time + 1_000, message(&format!("answer {turn}"))));
        events.push(at(time + 2_000, Event::TurnEnded(TurnEnd::Completed)));
    }
    events
}

fn turn() -> Vec<Event> {
    vec![
        user("hi"),
        message("caw"),
        Event::TurnEnded(TurnEnd::Completed),
    ]
}

#[gpui::test]
fn the_users_messages_sit_at_the_end_and_the_bots_at_the_start_with_avatar_and_name(
    cx: &mut TestAppContext,
) {
    let (workspace, _daemon, _clock, cx) = start(cx, ended(turn()));
    let (mine, theirs) = (key(&workspace, cx, "user"), key(&workspace, cx, "answer"));
    let pane = bounds(cx, "transcript");
    let own = bounds(cx, named("bubble", mine));
    let bot = bounds(cx, named("bubble", theirs));
    let avatar = bounds(cx, named("row-avatar", theirs));

    assert!(own.left() > pane.center().x, "{own:?} in {pane:?}");
    assert!(pane.right() - own.right() <= px(25.));
    assert!(bot.right() < pane.center().x, "{bot:?} in {pane:?}");
    assert!(avatar.left() - pane.left() <= px(25.));
    assert!(avatar.right() <= bot.left());
    assert_eq!(avatar.size.width, px(32.));
    let name = bounds(cx, named("row-name", theirs));
    assert!(name.bottom() <= bot.top());
    assert!(!shown(cx, named("row-avatar", mine)));
    assert!(!shown(cx, named("row-name", mine)));
}

#[gpui::test]
fn messages_of_one_author_within_five_minutes_show_the_avatar_once(cx: &mut TestAppContext) {
    for (gap_ms, again) in [(60_000, false), (6 * 60_000, true)] {
        let events = vec![
            at(NOON, user("go")),
            at(NOON + 1_000, tool_started("t1", "Read notes.txt")),
            at(NOON + 2_000, tool_ended("t1", ToolStatus::Completed, "ok")),
            at(NOON + 2_000 + gap_ms, message("done")),
            at(NOON + 3_000 + gap_ms, Event::TurnEnded(TurnEnd::Completed)),
        ];
        let (workspace, _daemon, _clock, cx) = start(cx, timed_snapshot(CHAT, 1, events));
        let (steps, answer) = (key(&workspace, cx, "steps"), key(&workspace, cx, "answer"));
        assert!(shown(cx, named("row-avatar", steps)));
        assert!(shown(cx, named("row-name", steps)));
        assert_eq!(shown(cx, named("row-avatar", answer)), again, "{gap_ms}");
        assert_eq!(shown(cx, named("row-name", answer)), again, "{gap_ms}");
    }
}

#[gpui::test]
fn a_day_separator_sits_between_messages_of_different_days(cx: &mut TestAppContext) {
    let events = vec![
        at(NOON, user("monday")),
        at(NOON + 1_000, message("caw")),
        at(NOON + 2_000, Event::TurnEnded(TurnEnd::Completed)),
        at(NOON + DAY_MS, user("tuesday")),
    ];
    let (workspace, _daemon, _clock, cx) = start(cx, timed_snapshot(CHAT, 1, events));
    let days = keys(&workspace, cx, "day");
    let users = keys(&workspace, cx, "user");
    assert_eq!((days.len(), users.len()), (2, 2));
    let first_day = bounds(cx, named("day", days[0]));
    let second_day = bounds(cx, named("day", days[1]));
    let answer = of(&workspace, cx, "bubble", "answer");
    assert!(first_day.bottom() <= bounds(cx, named("bubble", users[0])).top());
    assert!(answer.bottom() <= second_day.top());
    assert!(second_day.bottom() <= bounds(cx, named("bubble", users[1])).top());
    assert_eq!(second_day.size.height, px(24.));
}

#[gpui::test]
fn a_conversation_of_ten_thousand_events_draws_only_the_rows_near_the_viewport(
    cx: &mut TestAppContext,
) {
    let events = long_chat(3_334);
    assert!(events.len() >= 10_000);
    let (workspace, _daemon, _clock, cx) = start(cx, timed_snapshot(CHAT, 1, events));
    let all = rows(&workspace, cx);
    assert!(all.len() > 6_000);
    let drawn = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        let bubbles = all.iter().filter(|row| row.is_message());
        bubbles
            .filter(|row| cx.debug_bounds(named("bubble", row.key)).is_some())
            .map(|row| row.key)
            .collect::<Vec<_>>()
    };
    let at_end = drawn(cx);
    assert!(!at_end.is_empty() && at_end.len() < 80, "{}", at_end.len());
    assert_eq!(at_end.last(), all.last().map(|row| &row.key));

    let started = Instant::now();
    for _ in 0..30 {
        wheel(cx, 400.);
    }
    let per_frame = started.elapsed() / 30;
    let scrolled = drawn(cx);
    assert!(
        !scrolled.is_empty() && scrolled.len() < 80,
        "{}",
        scrolled.len()
    );
    assert!(scrolled.last() < at_end.first());
    println!(
        "10,000 events: {per_frame:?} for each scrolled frame, {} rows drawn",
        scrolled.len()
    );
}

#[gpui::test]
fn a_long_conversation_loads_older_pages_only_when_the_user_reaches_the_top(
    cx: &mut TestAppContext,
) {
    let mut newest = timed_snapshot(CHAT, 1, long_chat(60));
    newest.older = Some(77);
    let (workspace, mut daemon, _clock, cx) = start(cx, newest);
    let count = |cx: &mut VisualTestContext| rows(&workspace, cx).len();
    let before = count(cx);
    let oldest_shown = key(&workspace, cx, "user");
    assert!(following(&workspace, cx));
    let sent = daemon.sent();
    assert!(
        !sent
            .iter()
            .any(|message| matches!(message, ClientMessage::LoadOlderEvents { .. })),
        "{sent:?}"
    );

    scroll_to_top(&workspace, cx);
    assert!(daemon.sent().contains(&ClientMessage::LoadOlderEvents {
        conversation_id: CHAT,
        before: 77,
    }));
    assert!(shown(cx, "older-loading"));
    wheel(cx, 50.);
    assert_eq!(daemon.sent(), [], "one request for one page");

    let older: Vec<TimedEvent> = long_chat(10)
        .into_iter()
        .map(|event| at(event.at_ms - 3_600_000, event.event))
        .collect();
    daemon.tell(
        ServerMessage::OlderEvents {
            conversation_id: CHAT,
            events: older,
            older: None,
        },
        cx,
    );
    assert_eq!(count(cx), before + 20);
    assert!(!shown(cx, "older-loading"));
    let pane = bounds(cx, "transcript");
    let kept = bounds(cx, named("bubble", oldest_shown));
    assert!(
        kept.top() >= pane.top() && kept.top() - pane.top() < px(60.),
        "the row that was first moved to {kept:?} in {pane:?}"
    );
    scroll_to_top(&workspace, cx);
    assert_eq!(daemon.sent(), [], "no page is left");
    assert_eq!(key(&workspace, cx, "user"), rows(&workspace, cx)[1].key);
}

#[gpui::test]
fn an_older_page_that_ends_inside_a_turn_keeps_the_place_and_loads_one_page(
    cx: &mut TestAppContext,
) {
    let mut events = vec![
        at(
            NOON - 3_000,
            tool_ended("t1", ToolStatus::Completed, "done"),
        ),
        at(NOON - 2_000, message("the answer of the cut turn")),
        at(NOON - 1_000, Event::TurnEnded(TurnEnd::Completed)),
    ];
    events.extend(long_chat(60));
    let mut newest = timed_snapshot(CHAT, 1, events);
    newest.older = Some(77);
    let (workspace, mut daemon, _clock, cx) = start(cx, newest);
    let first_question = key(&workspace, cx, "user");
    scroll_to_top(&workspace, cx);
    daemon.sent();

    let mut older: Vec<TimedEvent> = long_chat(30)
        .into_iter()
        .map(|event| at(event.at_ms - 3_600_000, event.event))
        .collect();
    older.push(at(NOON - 5_000, user("the question of the cut turn")));
    older.push(at(NOON - 4_000, tool_started("t1", "Read")));
    daemon.tell(
        ServerMessage::OlderEvents {
            conversation_id: CHAT,
            events: older,
            older: Some(12),
        },
        cx,
    );
    let pane = bounds(cx, "transcript");
    let kept = bounds(cx, named("bubble", first_question));
    assert!(
        kept.top() >= pane.top() && kept.top() - pane.top() < px(200.),
        "the first question moved to {kept:?} in {pane:?}"
    );
    assert_eq!(daemon.sent(), [], "the next page waits for the top");
}

#[gpui::test]
fn a_click_under_a_tall_code_block_lands_on_the_text_that_shows_there(cx: &mut TestAppContext) {
    let code: String = (0..60)
        .map(|line| format!("a long line of code, number {line}\n"))
        .collect();
    let (workspace, _daemon, _clock, cx) = start(
        cx,
        ended(vec![
            user("show"),
            message(&format!("```\n{code}```\n\nafterword")),
            Event::TurnEnded(TurnEnd::Completed),
        ]),
    );
    let bubble = of(&workspace, cx, "bubble", "answer");
    press(
        cx,
        point(bubble.left() + px(40.), bubble.bottom() - px(8.)),
        2,
    );
    assert_eq!(selected(&workspace, cx).as_deref(), Some("afterword"));
}

#[gpui::test]
fn a_bot_with_no_message_shows_the_empty_state_with_its_name_and_title(cx: &mut TestAppContext) {
    let (_workspace, _daemon, _clock, cx) = start(cx, ended(Vec::new()));
    assert!(shown(cx, "empty-conversation"));
    assert!(shown(cx, "empty-name"));
    assert!(shown(cx, "empty-title"));
    assert!(!shown(cx, "transcript"));
    assert!(bounds(cx, "empty-name").bottom() <= bounds(cx, "empty-title").top());
}

#[gpui::test]
fn an_older_page_that_fails_to_load_keeps_the_list_and_offers_a_retry(cx: &mut TestAppContext) {
    let mut newest = timed_snapshot(CHAT, 1, long_chat(60));
    newest.older = Some(77);
    let (workspace, mut daemon, _clock, cx) = start(cx, newest);
    let before = rows(&workspace, cx);
    scroll_to_top(&workspace, cx);
    daemon.sent();

    daemon.tell(
        ServerMessage::OlderEventsFailed {
            conversation_id: CHAT,
            message: "the database is locked".to_owned(),
        },
        cx,
    );
    assert_eq!(rows(&workspace, cx), before);
    assert!(shown(cx, "older-error"));
    assert!(!shown(cx, "older-loading"));
    let pane = bounds(cx, "transcript");
    assert!(bounds(cx, "older-error").top() - pane.top() < px(4.));
    wheel(cx, 50.);
    assert_eq!(
        daemon.sent(),
        [],
        "a failed page is not asked again by itself"
    );

    click(cx, "retry-older");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::LoadOlderEvents {
            conversation_id: CHAT,
            before: 77,
        }]
    );
    assert!(shown(cx, "older-loading"));
    assert!(!shown(cx, "older-error"));
}

const EVERY_ELEMENT: &str = "# Title

A paragraph with **bold**, *italic*, ~~struck~~, `code`, and a [link](https://example.com).

1. first
2. second

- plain
- [x] done
- [ ] open

> quoted

| Name | Count |
|------|------:|
| crow | 2 |

---

```rust
fn main() {}
```
";

#[gpui::test]
fn every_markdown_element_renders_as_a_formatted_element(cx: &mut TestAppContext) {
    let events = vec![
        user("show me"),
        message(EVERY_ELEMENT),
        Event::TurnEnded(TurnEnd::Completed),
    ];
    let (workspace, _daemon, _clock, cx) = start(cx, ended(events));
    cx.simulate_resize(size(px(1000.), px(1600.)));
    for element in [
        "md-heading",
        "md-paragraph",
        "md-list",
        "md-task-done",
        "md-task-open",
        "md-quote",
        "md-table",
        "md-rule",
        "md-code",
    ] {
        assert!(shown(cx, element), "{element}");
    }
    assert!(bounds(cx, "md-heading").bottom() <= bounds(cx, "md-quote").top());
    assert!(bounds(cx, "md-quote").bottom() <= bounds(cx, "md-table").top());
    assert!(bounds(cx, "md-table").bottom() <= bounds(cx, "md-rule").top());
    assert!(bounds(cx, "md-rule").bottom() <= bounds(cx, "md-code").top());
    assert_eq!(bounds(cx, "md-rule").size.height, px(1.));
    let rendered = workspace.read_with(cx, |this, _| {
        let open = this.open.as_ref().unwrap();
        let answer = open
            .transcript
            .rows()
            .iter()
            .find(|row| kind(row) == "answer");
        open.texts[&answer.unwrap().key].document.text()
    });
    assert_eq!(
        rendered,
        "Title\nA paragraph with bold, italic, struck, code, and a link.\nfirst\nsecond\nplain\ndone\nopen\nquoted\nName\tCount\ncrow\t2\nfn main() {}"
    );
}

#[gpui::test]
fn a_code_block_shows_its_language_and_a_copy_control_that_resets_after_two_seconds(
    cx: &mut TestAppContext,
) {
    let events = vec![
        user("code"),
        message("```rust\nfn main() {}\n```"),
        Event::TurnEnded(TurnEnd::Completed),
    ];
    let (workspace, _daemon, _clock, cx) = start(cx, ended(events));
    let answer = key(&workspace, cx, "answer");
    let block = bounds(cx, "md-code");
    assert!(block.contains(&bounds(cx, "md-code-language").center()));
    assert!(block.contains(&bounds(cx, "copy-code-0").center()));
    assert!(bounds(cx, "md-code-language").bottom() <= bounds(cx, "md-code-text").top());
    assert_eq!(bounds(cx, "md-code-text").size.height, px(19.));

    let copied = |cx: &mut VisualTestContext| workspace.read_with(cx, |this, _| this.copied);
    click(cx, "copy-code-0");
    assert_eq!(clipboard(cx).as_deref(), Some("fn main() {}"));
    assert_eq!(copied(cx), Some(Copied::Code(answer, 0)));

    cx.executor()
        .advance_clock(duration::COPIED - Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(copied(cx), Some(Copied::Code(answer, 0)));
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(copied(cx), None);
}

#[gpui::test]
fn text_that_streams_is_parsed_from_its_last_open_block_only(cx: &mut TestAppContext) {
    let (workspace, daemon, clock, cx) = start(cx, running(vec![user("write")]));
    let mut sent = 0;
    for index in 0..40 {
        let chunk = format!("Paragraph {index} has a few words in it.\n\n");
        sent += chunk.len();
        tell(&daemon, cx, NOON + index, message(&chunk));
        clock.advance(&workspace, cx, 1_000);
    }
    let answer = key(&workspace, cx, "answer");
    let (starts, shown_length) = workspace.read_with(cx, |this, _| {
        let document = &this.open.as_ref().unwrap().texts[&answer].document;
        (document.parse_starts().to_vec(), document.source().len())
    });
    assert_eq!(shown_length, sent);
    assert!(starts.len() >= 40);
    assert!(starts.windows(2).all(|pair| pair[0] <= pair[1]));
    let last = *starts.last().unwrap();
    assert!(
        sent - last < 120,
        "the last parse started at {last} of {sent} bytes"
    );
}

#[gpui::test]
fn an_unclosed_block_at_the_end_of_a_stream_shows_as_the_block_it_becomes(cx: &mut TestAppContext) {
    for (stream, element) in [
        ("```rust\nlet x = 1;", "md-code"),
        ("| name | count |\n", "md-table"),
        ("- one\n- tw", "md-list"),
    ] {
        let (workspace, daemon, clock, cx) = start(cx, running(vec![user("go")]));
        tell(&daemon, cx, NOON, message(stream));
        clock.advance(&workspace, cx, 1_000);
        assert!(shown(cx, element), "{stream:?} shows no {element}");
        let answer = key(&workspace, cx, "answer");
        let rendered = workspace.read_with(cx, |this, _| {
            this.open.as_ref().unwrap().texts[&answer].document.text()
        });
        for marker in ["```", "|", "- "] {
            assert!(!rendered.contains(marker), "{rendered:?} shows {marker:?}");
        }
    }
}

#[gpui::test]
fn a_web_link_opens_in_the_browser_and_another_scheme_shows_its_target(cx: &mut TestAppContext) {
    let events = vec![
        user("links"),
        message("[site](https://example.com) is safe\n\n[file](file:///etc/passwd) is not"),
        Event::TurnEnded(TurnEnd::Completed),
    ];
    let (workspace, _daemon, _clock, cx) = start(cx, ended(events));
    let bubble = of(&workspace, cx, "bubble", "answer");
    let first = point(bubble.left() + px(16.), bubble.top() + px(18.));
    let second = point(first.x, first.y + px(32.));

    press(cx, first, 1);
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
    workspace.read_with(cx, |this, _| assert_eq!(this.notice, None));

    press(cx, second, 1);
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
    workspace.read_with(cx, |this, _| {
        assert_eq!(
            this.notice,
            Some("This link was not opened: file:///etc/passwd".into())
        );
    });
    assert!(shown(cx, "notice"));
}

#[gpui::test]
fn markdown_that_cannot_be_parsed_as_intended_shows_as_written(cx: &mut TestAppContext) {
    let broken =
        "**bold never closed and [a link(with no end\n\n| not | a table\n\n```\nunclosed fence";
    let events = vec![
        user("broken"),
        message(broken),
        Event::TurnEnded(TurnEnd::Completed),
    ];
    let (workspace, _daemon, _clock, cx) = start(cx, ended(events));
    let answer = key(&workspace, cx, "answer");
    assert!(bounds(cx, named("bubble", answer)).size.height > px(40.));
    let rendered = workspace.read_with(cx, |this, _| {
        this.open.as_ref().unwrap().texts[&answer].document.text()
    });
    for written in [
        "**bold never closed and [a link(with no end",
        "| not | a table",
        "unclosed fence",
    ] {
        assert!(rendered.contains(written), "{rendered:?} lost {written:?}");
    }
}

const THREE_BLOCKS: &str = "First paragraph here.\n\n- item one\n- item two\n\n```\ncode line\n```";

fn three_blocks(cx: &mut TestAppContext) -> (Entity<Workspace>, Daemon, &mut VisualTestContext) {
    let events = vec![
        user("alpha beta gamma"),
        message(THREE_BLOCKS),
        Event::TurnEnded(TurnEnd::Completed),
    ];
    let (workspace, daemon, _clock, cx) = start(cx, ended(events));
    (workspace, daemon, cx)
}

fn select_everything(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    let bubble = of(workspace, cx, "bubble", "answer");
    let code = bounds(cx, "md-code-text");
    drag(
        cx,
        point(bubble.left() + px(1.), bubble.top() + px(10.)),
        point(code.right() + px(40.), code.bottom() - px(2.)),
    );
}

#[gpui::test]
fn a_drag_selects_across_paragraphs_lists_and_code_and_the_copy_key_copies_it(
    cx: &mut TestAppContext,
) {
    let (workspace, _daemon, cx) = three_blocks(cx);
    select_everything(&workspace, cx);
    let whole = "First paragraph here.\nitem one\nitem two\ncode line";
    assert_eq!(selected(&workspace, cx).as_deref(), Some(whole));

    cx.simulate_keystrokes(copy_keys());
    assert_eq!(clipboard(cx).as_deref(), Some(whole));

    cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
    workspace.update_in(cx, |this, window, cx| this.focus_composer(window, cx));
    cx.simulate_keystrokes(copy_keys());
    assert_eq!(clipboard(cx).as_deref(), Some(whole));
}

#[gpui::test]
fn the_copy_key_copies_the_selection_of_the_composer_before_the_transcript(
    cx: &mut TestAppContext,
) {
    let (workspace, _daemon, cx) = three_blocks(cx);
    select_everything(&workspace, cx);
    workspace.update_in(cx, |this, window, cx| this.focus_composer(window, cx));
    cx.simulate_input("draft");
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    cx.simulate_keystrokes(copy_keys());
    assert_eq!(clipboard(cx).as_deref(), Some("draft"));
    assert_eq!(typed(&workspace, cx), "draft");
}

#[gpui::test]
fn a_double_click_selects_the_word_and_a_triple_click_the_block(cx: &mut TestAppContext) {
    let (workspace, _daemon, cx) = three_blocks(cx);
    let bubble = of(&workspace, cx, "bubble", "user");
    let word = point(bubble.left() + px(16.), bubble.top() + px(18.));

    press(cx, word, 1);
    assert_eq!(selected(&workspace, cx), None);
    press(cx, word, 2);
    assert_eq!(selected(&workspace, cx).as_deref(), Some("alpha"));
    press(cx, word, 3);
    assert_eq!(
        selected(&workspace, cx).as_deref(),
        Some("alpha beta gamma")
    );
}

#[gpui::test]
fn the_copy_action_of_a_message_copies_its_markdown_source(cx: &mut TestAppContext) {
    let (workspace, _daemon, cx) = three_blocks(cx);
    let answer = key(&workspace, cx, "answer");
    click(cx, named("copy-message", answer));
    assert_eq!(clipboard(cx).as_deref(), Some(THREE_BLOCKS));
    workspace.read_with(cx, |this, _| {
        assert_eq!(this.copied, Some(Copied::Message(answer)));
    });

    let question = key(&workspace, cx, "user");
    click(cx, named("copy-message", question));
    assert_eq!(clipboard(cx).as_deref(), Some("alpha beta gamma"));
}

#[gpui::test]
fn a_click_elsewhere_clears_the_selection(cx: &mut TestAppContext) {
    let (workspace, _daemon, cx) = three_blocks(cx);
    select_everything(&workspace, cx);
    assert!(selected(&workspace, cx).is_some());

    let sidebar = bounds(cx, "bot-2").center();
    cx.simulate_mouse_down(sidebar, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(selected(&workspace, cx), None);

    select_everything(&workspace, cx);
    let other = of(&workspace, cx, "bubble", "user");
    press(cx, point(other.left() + px(16.), other.top() + px(18.)), 2);
    assert_eq!(selected(&workspace, cx).as_deref(), Some("alpha"));
}

#[gpui::test]
fn a_selection_in_a_streaming_message_stays_on_its_characters(cx: &mut TestAppContext) {
    let events = vec![user("go"), message("alpha beta gamma")];
    let (workspace, daemon, clock, cx) = start(cx, running(events));
    let answer = key(&workspace, cx, "answer");
    let bubble = bounds(cx, named("bubble", answer));
    press(
        cx,
        point(bubble.left() + px(74.), bubble.top() + px(18.)),
        2,
    );
    let picked = selected(&workspace, cx);
    assert_eq!(picked.as_deref(), Some("beta"));
    let range = workspace.read_with(cx, |this, _| this.selection.borrow().unwrap().range());

    tell(
        &daemon,
        cx,
        NOON + 5_000,
        message(" delta\n\nA second paragraph."),
    );
    clock.advance(&workspace, cx, 1_000);
    assert_eq!(selected(&workspace, cx), picked);
    workspace.read_with(cx, |this, _| {
        assert_eq!(this.selection.borrow().unwrap().range(), range);
        let rendered = this.open.as_ref().unwrap().texts[&answer].document.text();
        assert_eq!(rendered, "alpha beta gamma delta\nA second paragraph.");
    });
}

fn words(count: usize) -> String {
    (0..count).map(|index| format!("word{index} ")).collect()
}

fn revealed(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> String {
    workspace.read_with(cx, |this, _| {
        let open = this.open.as_ref().unwrap();
        let answer = open
            .transcript
            .rows()
            .iter()
            .find(|row| kind(row) == "answer");
        answer
            .and_then(|row| open.texts.get(&row.key))
            .map(|text| text.document.source().to_owned())
            .unwrap_or_default()
    })
}

#[gpui::test]
fn a_burst_appears_word_by_word_and_is_never_a_second_behind(cx: &mut TestAppContext) {
    let (workspace, daemon, clock, cx) = start(cx, running(vec![user("go")]));
    let burst = words(60);
    tell(&daemon, cx, NOON, message(&burst));
    let mut lengths = Vec::new();
    for frame in 1..=62_u64 {
        clock.advance(&workspace, cx, 16);
        let shown = revealed(&workspace, cx);
        assert!(burst.starts_with(&shown));
        assert!(
            shown.len() == burst.len() || !shown.ends_with(char::is_whitespace),
            "{shown:?} ends between words"
        );
        assert!(
            shown.is_empty()
                || burst[shown.len()..].starts_with(char::is_whitespace)
                || shown.len() == burst.len(),
            "{shown:?} cuts a word"
        );
        let floor = (frame * 16 * 120 / 1000).saturating_sub(12);
        assert!(
            shown.len() as u64 >= floor.min(burst.len() as u64),
            "{} at frame {frame}",
            shown.len()
        );
        lengths.push(shown.len());
    }
    lengths.dedup();
    assert!(
        lengths.len() > 8,
        "the burst appeared in {} steps",
        lengths.len()
    );
    assert_eq!(
        revealed(&workspace, cx),
        burst,
        "all of it is shown within a second"
    );
}

#[gpui::test]
fn the_end_of_a_turn_shows_the_whole_text_within_half_a_second(cx: &mut TestAppContext) {
    let (workspace, daemon, clock, cx) = start(cx, running(vec![user("go")]));
    let burst = words(400);
    tell(&daemon, cx, NOON, message(&burst));
    clock.advance(&workspace, cx, 16);
    assert!(revealed(&workspace, cx).len() < burst.len() / 2);

    tell(
        &daemon,
        cx,
        NOON + 1_000,
        Event::TurnEnded(TurnEnd::Completed),
    );
    let mut waited = 0;
    while revealed(&workspace, cx).len() < burst.len() {
        clock.advance(&workspace, cx, 16);
        waited += 16;
        assert!(waited <= 500, "the text is still hidden after {waited} ms");
    }
    assert_eq!(revealed(&workspace, cx), burst);
}

#[gpui::test]
fn a_window_that_was_hidden_shows_all_received_text_at_once(cx: &mut TestAppContext) {
    let (workspace, daemon, clock, cx) = start(cx, running(vec![user("go")]));
    let burst = words(400);
    tell(&daemon, cx, NOON, message(&burst));
    clock.advance(&workspace, cx, 16);
    assert!(revealed(&workspace, cx).len() < burst.len() / 2);

    clock.advance(&workspace, cx, 5_000);
    assert_eq!(revealed(&workspace, cx), burst);
}

fn new_turn(daemon: &Daemon, cx: &mut VisualTestContext, at_ms: i64) {
    tell(daemon, cx, at_ms, user("one more"));
    tell(daemon, cx, at_ms + 1_000, message("here it is"));
    tell(
        daemon,
        cx,
        at_ms + 2_000,
        Event::TurnEnded(TurnEnd::Completed),
    );
}

#[gpui::test]
fn the_list_stays_at_its_end_while_content_arrives(cx: &mut TestAppContext) {
    let (workspace, daemon, clock, cx) = start(cx, timed_snapshot(CHAT, 1, long_chat(40)));
    assert!(following(&workspace, cx));
    new_turn(&daemon, cx, NOON + DAY_MS / 4);
    clock.advance(&workspace, cx, 1_000);
    assert!(following(&workspace, cx));
    let pane = bounds(cx, "transcript");
    let newest = of_last(&workspace, cx, "bubble", "answer");
    assert!(newest.bottom() <= pane.bottom() && newest.top() >= pane.top());
    assert!(!shown(cx, "jump-to-latest"));

    wheel(cx, 40.);
    assert!(!following(&workspace, cx));
    assert!(!shown(cx, "jump-to-latest"));
    new_turn(&daemon, cx, NOON + DAY_MS / 3);
    assert!(
        following(&workspace, cx),
        "40 px from the end still follows"
    );
}

#[gpui::test]
fn scrolling_up_stops_the_following_and_jump_to_latest_counts_and_returns(cx: &mut TestAppContext) {
    let (workspace, daemon, clock, cx) = start(cx, timed_snapshot(CHAT, 1, long_chat(40)));
    wheel(cx, 300.);
    assert!(!following(&workspace, cx));
    assert!(shown(cx, "jump-to-latest"));
    assert!(!shown(cx, "jump-count"));
    let drawn: Vec<i64> = rows(&workspace, cx)
        .iter()
        .filter(|row| row.is_message())
        .map(|row| row.key)
        .filter(|key| cx.debug_bounds(named("bubble", *key)).is_some())
        .collect();
    let watched = named("bubble", drawn[drawn.len() / 2]);
    let before = bounds(cx, watched);

    new_turn(&daemon, cx, NOON + DAY_MS / 4);
    clock.advance(&workspace, cx, 1_000);
    assert_eq!(bounds(cx, watched), before, "the list moved");
    assert!(!following(&workspace, cx));
    assert!(shown(cx, "jump-to-latest"));
    assert!(shown(cx, "jump-count"));
    let counted = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |this, _| this.open.as_ref().unwrap().new_messages)
    };
    assert_eq!(counted(cx), 2);

    click(cx, "jump-to-latest");
    assert!(following(&workspace, cx));
    assert_eq!(counted(cx), 0);
    assert!(!shown(cx, "jump-to-latest"));
    let pane = bounds(cx, "transcript");
    let newest = of_last(&workspace, cx, "bubble", "answer");
    assert!(newest.bottom() <= pane.bottom() && newest.top() >= pane.top());
}

fn worked() -> Vec<TimedEvent> {
    vec![
        at(NOON, user("fix it")),
        at(NOON + 1_000, thought("I should read the notes first.")),
        at(NOON + 2_000, tool_started("t1", "Read notes.txt")),
        at(
            NOON + 3_000,
            tool_ended("t1", ToolStatus::Completed, "line one"),
        ),
        at(NOON + 4_000, tool_started("t2", "Run tests")),
        at(
            NOON + 9_000,
            tool_ended("t2", ToolStatus::Failed, "error: 1 test failed"),
        ),
        at(NOON + 10_000, message("One test fails.")),
        at(NOON + 12_000, Event::TurnEnded(TurnEnd::Completed)),
    ]
}

#[gpui::test]
fn one_header_above_the_answer_shows_the_count_the_failures_and_the_duration(
    cx: &mut TestAppContext,
) {
    let (workspace, _daemon, _clock, cx) = start(cx, timed_snapshot(CHAT, 1, worked()));
    let steps = keys(&workspace, cx, "steps");
    assert_eq!(steps.len(), 1);
    let header = bounds(cx, named("steps", steps[0]));
    let answer = of(&workspace, cx, "bubble", "answer");
    assert!(header.bottom() <= answer.top());
    for part in ["steps-count", "steps-failed", "steps-duration"] {
        assert!(
            header.contains(&bounds(cx, named(part, steps[0])).center()),
            "{part}"
        );
    }
    assert!(!shown(cx, named("step-list", steps[0])));
    let all = rows(&workspace, cx);
    let row = all.iter().find(|row| row.key == steps[0]).unwrap();
    let RowKind::Steps {
        steps: list,
        live,
        started_ms,
        ended_ms,
    } = &row.kind
    else {
        panic!("not a steps row");
    };
    assert_eq!(list.len(), 3);
    assert!(!live);
    assert_eq!(ended_ms.unwrap() - started_ms, 12_000);
    assert_eq!(steps::duration_label(12_000), "12 s");
    assert_eq!(list[2].state, StepState::Failed);
    assert_eq!(list[2].output.as_deref(), Some("error: 1 test failed"));
}

#[gpui::test]
fn the_header_opens_a_scrollable_list_of_at_most_220_px_and_closes_it(cx: &mut TestAppContext) {
    let mut events = vec![at(NOON, user("many"))];
    for index in 0..30 {
        let id = format!("t{index}");
        let time = NOON + 1_000 + index * 100;
        events.push(at(time, tool_started(&id, &format!("Step {index}"))));
        events.push(at(time + 50, tool_ended(&id, ToolStatus::Completed, "ok")));
    }
    events.push(at(NOON + 9_000, message("done")));
    events.push(at(NOON + 9_500, Event::TurnEnded(TurnEnd::Completed)));
    let (workspace, _daemon, _clock, cx) = start(cx, timed_snapshot(CHAT, 1, events));
    let steps = key(&workspace, cx, "steps");
    assert!(!shown(cx, named("step-list", steps)));

    click(cx, named("steps", steps));
    let list = bounds(cx, named("step-list", steps));
    assert_eq!(list.size.height, px(220.));
    let overflow = workspace.read_with(cx, |this, _| {
        this.open.as_ref().unwrap().step_lists[&steps]
            .max_offset()
            .y
    });
    assert!(overflow > px(200.), "{overflow:?} to scroll");
    assert!(list.contains(&bounds(cx, step("step", steps, 0)).center()));

    click(cx, named("steps", steps));
    assert!(!shown(cx, named("step-list", steps)));
}

#[gpui::test]
fn a_running_turn_shows_its_newest_step_with_the_shine(cx: &mut TestAppContext) {
    let (workspace, daemon, _clock, cx) = start(cx, running(vec![user("many")]));
    for index in 0..30 {
        let id = format!("t{index}");
        tell(
            &daemon,
            cx,
            NOON + index * 100,
            tool_started(&id, &format!("Step {index}")),
        );
    }
    cx.run_until_parked();
    workspace.update(cx, |_, cx| cx.notify());
    let steps = key(&workspace, cx, "steps");
    let list = bounds(cx, named("step-list", steps));
    assert_eq!(list.size.height, px(220.));
    let newest = bounds(cx, step("step-shining", steps, 29));
    assert!(
        newest.top() >= list.top() && newest.bottom() <= list.bottom() + px(1.),
        "{newest:?} outside {list:?}"
    );
    assert!(!shown(cx, step("step-shining", steps, 28)));
    assert!(shown(cx, step("step", steps, 28)));

    tell(
        &daemon,
        cx,
        NOON + 9_000,
        Event::TurnEnded(TurnEnd::Completed),
    );
    assert!(!shown(cx, named("step-list", steps)));
    click(cx, named("steps", steps));
    assert!(!shown(cx, step("step-shining", steps, 29)));
    assert!(shown(cx, step("step", steps, 29)));
}

#[gpui::test]
fn a_failed_step_reveals_its_error_text_on_demand(cx: &mut TestAppContext) {
    let (workspace, _daemon, _clock, cx) = start(cx, timed_snapshot(CHAT, 1, worked()));
    let steps = key(&workspace, cx, "steps");
    click(cx, named("steps", steps));
    assert!(shown(cx, step("step-state", steps, 2)));
    assert!(!shown(cx, step("step-state", steps, 1)));
    assert!(!shown(cx, step("step-output", steps, 2)));

    click(cx, step("step-output-toggle", steps, 2));
    let error = bounds(cx, step("step-output", steps, 2));
    assert!(error.size.height >= px(19.));
    assert!(bounds(cx, step("step", steps, 2)).contains(&error.center()));

    click(cx, step("step-output-toggle", steps, 2));
    assert!(!shown(cx, step("step-output", steps, 2)));
}

#[gpui::test]
fn thinking_is_a_step_and_a_stopped_turn_with_no_answer_shows_it_in_its_own_row(
    cx: &mut TestAppContext,
) {
    let events = vec![
        user("think"),
        thought("I should look at the notes."),
        Event::TurnEnded(TurnEnd::Cancelled),
    ];
    let (workspace, _daemon, _clock, cx) = start(cx, ended(events));
    let kinds: Vec<_> = rows(&workspace, cx).iter().map(kind).collect();
    assert_eq!(kinds, ["day", "user", "steps", "thought", "status"]);
    let (steps, own_row) = (key(&workspace, cx, "steps"), key(&workspace, cx, "thought"));
    let thinking = bounds(cx, named("thought", own_row));
    assert!(thinking.size.height >= px(36.));
    assert!(bounds(cx, named("steps", steps)).bottom() <= thinking.top());
    assert!(thinking.bottom() <= of(&workspace, cx, "status", "status").top());

    click(cx, named("steps", steps));
    assert!(shown(cx, step("step", steps, 0)));
    let all = rows(&workspace, cx);
    let RowKind::Steps { steps: list, .. } = &all[2].kind else {
        panic!("not a steps row");
    };
    assert_eq!(list[0].kind, crate::transcript::StepKind::Thinking);
    assert_eq!(list[0].title, "I should look at the notes.");
}

#[gpui::test]
fn a_tool_call_with_no_end_shows_as_interrupted_once_the_turn_has_ended(cx: &mut TestAppContext) {
    let events = vec![user("run"), tool_started("t1", "Run tests")];
    let (workspace, daemon, _clock, cx) = start(cx, running(events));
    let steps = key(&workspace, cx, "steps");
    let state = |cx: &mut VisualTestContext| {
        let all = rows(&workspace, cx);
        let row = all.iter().find(|row| row.key == steps).unwrap();
        match &row.kind {
            RowKind::Steps { steps, .. } => steps[0].state,
            _ => panic!("not a steps row"),
        }
    };
    assert_eq!(state(cx), StepState::Running);
    assert!(!shown(cx, step("step-state", steps, 0)));

    tell(
        &daemon,
        cx,
        NOON + 5_000,
        Event::TurnInterrupted {
            cause: Interruption::DaemonRestarted,
        },
    );
    assert_eq!(state(cx), StepState::Interrupted);
    click(cx, named("steps", steps));
    assert!(shown(cx, step("step-state", steps, 0)));
    assert!(!shown(cx, step("step-shining", steps, 0)));
}

fn asking(cut: bool) -> ConversationSnapshot {
    let mut ask = request("a1");
    if let Event::ApprovalRequested { cut: flag, .. } = &mut ask {
        *flag = cut;
    }
    let mut snapshot = running(vec![
        user("write"),
        tool_started("tool", "Write notes.txt"),
        ask,
    ]);
    snapshot.pending_approvals = vec!["a1".to_owned()];
    snapshot
}

fn allow_a1() -> ClientMessage {
    ClientMessage::ResolveApproval {
        conversation_id: CHAT,
        approval_id: "a1".to_owned(),
        decision: ApprovalDecision::Allowed,
    }
}

#[gpui::test]
fn the_approval_card_shows_the_title_the_target_and_the_folder_with_three_answers_in_order(
    cx: &mut TestAppContext,
) {
    let (workspace, _daemon, _clock, cx) = start(cx, asking(false));
    let card_key = key(&workspace, cx, "approval");
    let card = bounds(cx, named("approval", card_key));
    assert!(card.size.width <= px(540.));
    let title = bounds(cx, named("approval-title", card_key));
    let target = bounds(cx, named("approval-target", card_key));
    let folder = bounds(cx, named("approval-folder", card_key));
    assert!(card.contains(&title.center()) && card.contains(&target.center()));
    assert!(title.bottom() <= target.top());
    assert!(target.contains(&folder.center()));
    let approval = rows(&workspace, cx)
        .into_iter()
        .find_map(|row| match row.kind {
            RowKind::Approval(approval) => Some(approval),
            _ => None,
        })
        .unwrap();
    assert_eq!(approval.title.as_deref(), Some("Write notes.txt"));
    assert_eq!(approval.target.as_deref(), Some("notes.txt"));
    assert_eq!(approval.folder.as_deref(), Some("/work"));

    let deny = bounds(cx, named("deny", card_key));
    let always = bounds(cx, named("always-allow", card_key));
    let allow = bounds(cx, named("allow", card_key));
    assert!(deny.right() <= always.left() && always.right() <= allow.left());
    assert!(target.bottom() <= deny.top());
    assert!(!shown(cx, named("approval-outcome", card_key)));
}

#[gpui::test]
fn always_allow_confirms_the_move_to_full_access_before_it_changes_the_level(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, _clock, cx) = start(cx, asking(false));
    let card = key(&workspace, cx, "approval");
    daemon.sent();

    click(cx, named("always-allow", card));
    assert!(shown(cx, "full-access-dialog"));
    assert!(shown(cx, "full-access-description"));
    assert_eq!(daemon.sent(), []);
    cx.simulate_keystrokes("escape");
    assert!(!shown(cx, "full-access-dialog"));
    assert_eq!(daemon.sent(), []);

    click(cx, named("always-allow", card));
    click(cx, "confirm-full-access");
    assert!(!shown(cx, "full-access-dialog"));
    assert_eq!(
        daemon.sent(),
        [ClientMessage::UpdateBot {
            bot_id: 1,
            patch: BotPatch {
                permission_level: Some(PermissionLevel::FullAccess),
                ..BotPatch::default()
            },
        }]
    );
    click(cx, named("allow", card));
    assert_eq!(daemon.sent(), [], "the answer is on its way");

    tell(
        &daemon,
        cx,
        NOON + 9_000,
        Event::ApprovalResolved {
            id: Some("a1".to_owned()),
            tool_call_id: "tool".to_owned(),
            title: Some("Write notes.txt".to_owned()),
            decision: ApprovalDecision::Allowed,
            reason: Some(ApprovalReason::PermissionLevel),
        },
    );
    assert!(shown(cx, named("approval-outcome", card)));
    assert!(!shown(cx, named("allow", card)));
}

#[gpui::test]
fn a_request_settled_elsewhere_closes_the_full_access_confirmation(cx: &mut TestAppContext) {
    let answered = Event::ApprovalResolved {
        id: Some("a1".to_owned()),
        tool_call_id: "tool".to_owned(),
        title: Some("Write notes.txt".to_owned()),
        decision: ApprovalDecision::Declined,
        reason: None,
    };
    let stopped = Event::TurnEnded(TurnEnd::Cancelled);
    for outcome in [answered, stopped] {
        let (workspace, mut daemon, _clock, cx) = start(cx, asking(false));
        let card = key(&workspace, cx, "approval");
        click(cx, named("always-allow", card));
        assert!(shown(cx, "full-access-dialog"));
        daemon.sent();

        tell(&daemon, cx, NOON + 9_000, outcome);
        assert!(!shown(cx, "full-access-dialog"));
        workspace.update_in(cx, |this, window, cx| {
            this.full_access_confirmed(window, cx);
        });
        assert!(
            !daemon
                .sent()
                .iter()
                .any(|message| matches!(message, ClientMessage::UpdateBot { .. })),
            "a request that no longer waits does not move the bot to Full access"
        );
    }
}

#[gpui::test]
fn the_buttons_are_disabled_while_the_answer_is_sent_and_a_refusal_enables_them_again(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, _clock, cx) = start(cx, asking(false));
    let card = key(&workspace, cx, "approval");
    daemon.sent();

    click(cx, named("allow", card));
    assert_eq!(daemon.sent(), [allow_a1()]);
    click(cx, named("allow", card));
    click(cx, named("deny", card));
    click(cx, named("always-allow", card));
    assert_eq!(daemon.sent(), []);
    assert!(!shown(cx, "full-access-dialog"));

    daemon.tell(
        ServerMessage::ApprovalNotResolved {
            conversation_id: CHAT,
            approval_id: "a1".to_owned(),
            message: "The bot no longer waits for this answer.".to_owned(),
        },
        cx,
    );
    workspace.read_with(cx, |this, _| {
        assert_eq!(
            this.notice,
            Some("The answer was not sent: The bot no longer waits for this answer.".into())
        );
    });
    assert!(shown(cx, "notice"));
    click(cx, named("allow", card));
    assert_eq!(daemon.sent(), [allow_a1()]);

    daemon.link(
        ConnectionEvent::Disconnected("the pipe closed".to_owned()),
        cx,
    );
    workspace.read_with(cx, |this, _| {
        assert!(this.open.as_ref().unwrap().answering.is_empty());
        assert_eq!(
            this.notice,
            Some("The answer was not sent: the daemon cannot be reached.".into())
        );
    });
}

#[gpui::test]
fn a_request_cut_by_the_size_cap_offers_only_deny(cx: &mut TestAppContext) {
    let (workspace, mut daemon, _clock, cx) = start(cx, asking(true));
    let card = key(&workspace, cx, "approval");
    daemon.sent();
    assert!(shown(cx, named("deny", card)));
    assert!(shown(cx, named("approval-cut", card)));
    assert!(!shown(cx, named("allow", card)));
    assert!(!shown(cx, named("always-allow", card)));

    click(cx, named("deny", card));
    assert_eq!(
        daemon.sent(),
        [ClientMessage::ResolveApproval {
            conversation_id: CHAT,
            approval_id: "a1".to_owned(),
            decision: ApprovalDecision::Declined,
        }]
    );
}

#[gpui::test]
fn an_approval_answered_elsewhere_or_expired_shows_its_outcome_and_no_button(
    cx: &mut TestAppContext,
) {
    let answered = Event::ApprovalResolved {
        id: Some("a1".to_owned()),
        tool_call_id: "tool".to_owned(),
        title: Some("Write notes.txt".to_owned()),
        decision: ApprovalDecision::Declined,
        reason: None,
    };
    let expired = Event::ApprovalClosed {
        id: "a1".to_owned(),
        tool_call_id: "tool".to_owned(),
        title: Some("Write notes.txt".to_owned()),
        reason: ApprovalClosure::Expired,
    };
    for outcome in [answered, expired] {
        let (workspace, daemon, _clock, cx) = start(cx, asking(false));
        let card = key(&workspace, cx, "approval");
        assert!(shown(cx, named("allow", card)));

        tell(&daemon, cx, NOON + 9_000, outcome);
        assert!(shown(cx, named("approval-outcome", card)));
        assert!(shown(cx, named("approval-title", card)));
        for button in ["deny", "always-allow", "allow"] {
            assert!(!shown(cx, named(button, card)), "{button}");
        }
    }
}

#[gpui::test]
fn the_control_is_stop_while_the_bot_works_with_an_empty_box_and_send_with_text(
    cx: &mut TestAppContext,
) {
    let (_workspace, mut daemon, _clock, cx) = start(cx, running(vec![user("go")]));
    daemon.sent();
    assert!(shown(cx, "stop-turn"));
    assert!(!shown(cx, "send-message"));

    cx.simulate_input("wait");
    assert!(shown(cx, "send-message"));
    assert!(!shown(cx, "stop-turn"));
    assert_eq!(bounds(cx, "send-message").size, size(px(28.), px(28.)));

    cx.simulate_keystrokes("backspace backspace backspace backspace");
    click(cx, "stop-turn");
    assert_eq!(
        daemon.sent(),
        [ClientMessage::StopTurn {
            conversation_id: CHAT
        }]
    );

    tell(
        &daemon,
        cx,
        NOON + 5_000,
        Event::TurnEnded(TurnEnd::Cancelled),
    );
    assert!(!shown(cx, "stop-turn"));
    assert!(shown(cx, "send-message"));
}

#[gpui::test]
fn the_send_shortcut_preference_decides_what_enter_does(cx: &mut TestAppContext) {
    let (workspace, mut daemon, _clock, cx) = start(cx, ended(turn()));
    daemon.sent();
    let sent_text = |daemon: &mut Daemon| {
        let sent = daemon.sent();
        let texts = sent.into_iter().filter_map(|message| match message {
            ClientMessage::SendMessage { text, .. } => Some(text),
            _ => None,
        });
        texts.collect::<Vec<_>>()
    };

    cx.simulate_input("one");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("two");
    assert_eq!(typed(&workspace, cx), "one\ntwo");
    cx.simulate_keystrokes("enter");
    assert_eq!(sent_text(&mut daemon), ["one\ntwo"]);
    assert_eq!(typed(&workspace, cx), "");

    daemon.tell(
        ServerMessage::Preferences(Preferences {
            send_shortcut: SendShortcut::ModEnter,
            ..Preferences::default()
        }),
        cx,
    );
    cx.simulate_input("three");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("four");
    assert_eq!(typed(&workspace, cx), "three\nfour");
    assert_eq!(sent_text(&mut daemon), [] as [&str; 0]);
    cx.simulate_keystrokes(send_keys());
    assert_eq!(sent_text(&mut daemon), ["three\nfour"]);
    assert_eq!(typed(&workspace, cx), "");
}

#[gpui::test]
fn a_draft_is_stored_by_the_daemon_and_comes_back_with_its_conversation(cx: &mut TestAppContext) {
    let (workspace, mut daemon, _clock, cx) = start(cx, ended(turn()));
    daemon.sent();
    let draft = |text: &str| ClientMessage::SetDraft {
        conversation_id: CHAT,
        text: text.to_owned(),
    };

    cx.simulate_input("half a thought");
    cx.run_until_parked();
    assert_eq!(daemon.sent(), []);
    cx.executor().advance_clock(duration::DRAFT_SAVE);
    cx.run_until_parked();
    assert_eq!(daemon.sent(), [draft("half a thought")]);

    cx.simulate_input(" more");
    click(cx, "bot-2");
    assert_eq!(
        daemon.sent(),
        [
            draft("half a thought more"),
            ClientMessage::CloseConversation {
                conversation_id: CHAT
            },
            ClientMessage::OpenConversation {
                conversation_id: 20
            },
        ]
    );
    assert_eq!(typed(&workspace, cx), "");

    let mut other = snapshot(20, 2, Vec::new());
    other.draft = "for wren".to_owned();
    daemon.tell(ServerMessage::ConversationOpened(other), cx);
    assert_eq!(typed(&workspace, cx), "for wren");
    daemon.sent();

    click(cx, "bot-1");
    assert_eq!(
        daemon.sent(),
        [
            ClientMessage::CloseConversation {
                conversation_id: 20
            },
            ClientMessage::OpenConversation {
                conversation_id: CHAT
            },
        ]
    );
    let mut back = ended(turn());
    back.draft = "half a thought more".to_owned();
    daemon.tell(ServerMessage::ConversationOpened(back), cx);
    assert_eq!(typed(&workspace, cx), "half a thought more");
    cx.executor().advance_clock(duration::DRAFT_SAVE);
    cx.run_until_parked();
    assert!(
        !daemon
            .sent()
            .iter()
            .any(|message| matches!(message, ClientMessage::SetDraft { .. })),
        "a restored draft is not saved again"
    );
}

#[gpui::test]
fn queued_messages_are_listed_in_order_above_the_composer_with_three_actions_each(
    cx: &mut TestAppContext,
) {
    let mut waiting = running(vec![user("go")]);
    waiting.queue = QueueState {
        messages: vec![
            QueuedMessage {
                id: 1,
                text: "first".to_owned(),
            },
            QueuedMessage {
                id: 2,
                text: "second".to_owned(),
            },
        ],
        paused: false,
    };
    let (workspace, mut daemon, _clock, cx) = start(cx, waiting);
    daemon.sent();
    let panel = bounds(cx, "queue");
    assert!(panel.bottom() <= bounds(cx, "stop-turn").top());
    let (first, second) = (bounds(cx, "queued-0"), bounds(cx, "queued-1"));
    assert!(panel.contains(&first.center()) && panel.contains(&second.center()));
    assert!(first.bottom() <= second.top());
    assert_eq!(first.size.height, px(29.));
    for row in 0..2 {
        let edit = bounds(cx, named("queue-edit", row));
        let now = bounds(cx, named("queue-send-now", row));
        let delete = bounds(cx, named("queue-delete", row));
        assert!(edit.right() <= now.left() && now.right() <= delete.left());
    }

    click(cx, "queue-send-now-1");
    click(cx, "queue-delete-0");
    assert_eq!(
        daemon.sent(),
        [
            ClientMessage::SendQueuedMessageNow {
                conversation_id: CHAT,
                queue_id: 2,
            },
            ClientMessage::DeleteQueuedMessage {
                conversation_id: CHAT,
                queue_id: 1,
            },
        ]
    );
    click(cx, "queue-edit-0");
    assert_eq!(typed(&workspace, cx), "first");
    workspace.read_with(cx, |this, _| assert_eq!(this.editing, Some(1)));
}

#[gpui::test]
fn a_send_that_fails_keeps_the_text_and_states_the_reason_above_the_composer(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, _clock, cx) = start(cx, ended(turn()));
    daemon.sent();
    let message = |text: &str| ClientMessage::SendMessage {
        conversation_id: CHAT,
        text: text.to_owned(),
    };

    cx.simulate_input("hello");
    cx.simulate_keystrokes("enter");
    assert_eq!(daemon.sent(), [message("hello")]);
    assert_eq!(typed(&workspace, cx), "");
    daemon.tell(
        ServerMessage::MessageRefused {
            conversation_id: CHAT,
            message: "The bot is being deleted.".to_owned(),
        },
        cx,
    );
    assert_eq!(typed(&workspace, cx), "hello");
    let banner = bounds(cx, "send-error");
    assert!(banner.bottom() <= bounds(cx, "send-message").top());
    workspace.read_with(cx, |this, _| {
        assert_eq!(this.send_error, Some("The bot is being deleted.".into()));
    });

    cx.simulate_keystrokes("enter");
    assert_eq!(daemon.sent(), [message("hello")]);
    assert!(!shown(cx, "send-error"));
    daemon.tell(
        ServerMessage::MessageAccepted {
            conversation_id: CHAT,
        },
        cx,
    );
    assert_eq!(typed(&workspace, cx), "");
    workspace.read_with(cx, |this, _| assert!(this.unconfirmed.is_empty()));

    cx.simulate_input("again");
    cx.simulate_keystrokes("enter");
    assert_eq!(typed(&workspace, cx), "");
    daemon.link(
        ConnectionEvent::Disconnected("the pipe closed".to_owned()),
        cx,
    );
    assert_eq!(typed(&workspace, cx), "again");
    assert!(shown(cx, "send-error"));
    workspace.read_with(cx, |this, _| {
        assert_eq!(
            this.send_error,
            Some("The message was not sent: the daemon cannot be reached.".into())
        );
    });
}

#[gpui::test]
fn messages_accepted_by_two_conversations_in_any_order_leave_nothing_unconfirmed(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, _clock, cx) = start(cx, ended(turn()));
    cx.simulate_input("for atlas");
    cx.simulate_keystrokes("enter");
    click(cx, "bot-2");
    daemon.tell(
        ServerMessage::ConversationOpened(snapshot(20, 2, Vec::new())),
        cx,
    );
    cx.simulate_input("for wren");
    cx.simulate_keystrokes("enter");
    workspace.read_with(cx, |this, _| assert_eq!(this.unconfirmed.len(), 2));

    for conversation_id in [20, CHAT] {
        daemon.tell(ServerMessage::MessageAccepted { conversation_id }, cx);
    }
    workspace.read_with(cx, |this, _| assert!(this.unconfirmed.is_empty()));
    daemon.sent();

    daemon.link(
        ConnectionEvent::Disconnected("the pipe closed".to_owned()),
        cx,
    );
    assert_eq!(typed(&workspace, cx), "");
    assert!(!shown(cx, "send-error"));
}

#[gpui::test]
fn a_message_over_the_length_limit_is_refused_by_the_client_and_states_the_limit(
    cx: &mut TestAppContext,
) {
    let (workspace, mut daemon, _clock, cx) = start(cx, ended(turn()));
    daemon.sent();
    let long = "a".repeat(MESSAGE_LENGTH + 1);
    workspace.update_in(cx, |this, window, cx| {
        this.composer
            .update(cx, |composer, cx| composer.set_text(&long, window, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.executor().advance_clock(duration::DRAFT_SAVE);
    cx.run_until_parked();
    assert_eq!(daemon.sent(), []);
    assert_eq!(typed(&workspace, cx).len(), MESSAGE_LENGTH + 1);
    assert!(shown(cx, "send-error"));
    workspace.read_with(cx, |this, _| {
        assert_eq!(
            this.send_error,
            Some("A message can hold 100,000 characters at most. This one has 100,001.".into())
        );
    });

    cx.simulate_keystrokes("backspace");
    cx.simulate_keystrokes("enter");
    let sent = daemon.sent();
    assert!(
        matches!(&sent[..], [ClientMessage::SendMessage { text, .. }] if text.len() == MESSAGE_LENGTH),
        "{} messages",
        sent.len()
    );
    assert!(!shown(cx, "send-error"));
}

#[gpui::test]
fn a_turn_that_did_not_end_normally_says_so_after_the_partial_answer(cx: &mut TestAppContext) {
    let failed = |cause, message: &str| Event::TurnFailed {
        message: message.to_owned(),
        cause,
        provider: Some("claude".to_owned()),
    };
    let restarted = Event::TurnInterrupted {
        cause: Interruption::DaemonRestarted,
    };
    let cases = [
        (failed(None, "The model is overloaded."), "Error"),
        (Event::TurnEnded(TurnEnd::Cancelled), "Stopped"),
        (restarted, "Interrupted"),
        (
            failed(Some(FailureCause::SignInRequired), "401"),
            "Sign-in needed",
        ),
        (
            failed(Some(FailureCause::UsageLimit), "429"),
            "Usage limit reached",
        ),
    ];
    for (end, label) in cases {
        let events = vec![user("go"), message("a partial answer")];
        let (workspace, daemon, clock, cx) = start(cx, running(events));
        assert_eq!(keys(&workspace, cx, "status"), [] as [i64; 0]);

        tell(&daemon, cx, NOON + 5_000, end);
        clock.advance(&workspace, cx, 1_000);
        let status = key(&workspace, cx, "status");
        let answer = of(&workspace, cx, "bubble", "answer");
        let row = bounds(cx, named("status", status));
        assert!(answer.bottom() <= row.top(), "{label}");
        assert!(row.contains(&bounds(cx, named("status-label", status)).center()));
        assert_eq!(revealed(&workspace, cx), "a partial answer");
        let all = rows(&workspace, cx);
        let RowKind::Status(shown_status) = &all.last().unwrap().kind else {
            panic!("the last row is not a status");
        };
        let (shown_label, detail) = rows::status_text(shown_status);
        assert_eq!(shown_label, label);
        match label {
            "Error" => assert_eq!(detail, "The model is overloaded."),
            "Stopped" => assert_eq!(detail, ""),
            "Interrupted" => assert!(detail.contains("daemon restarted"), "{detail}"),
            _ => assert!(detail.contains("Claude"), "{detail}"),
        }
    }
}

#[gpui::test]
fn a_turn_that_ended_normally_shows_no_status_label(cx: &mut TestAppContext) {
    let events = vec![user("go"), message("the whole answer")];
    let (workspace, daemon, _clock, cx) = start(cx, running(events));
    tell(
        &daemon,
        cx,
        NOON + 5_000,
        Event::TurnEnded(TurnEnd::Completed),
    );
    let kinds: Vec<_> = rows(&workspace, cx).iter().map(kind).collect();
    assert_eq!(kinds, ["day", "user", "answer"]);
}
