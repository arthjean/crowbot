use std::collections::HashSet;

use chrono::{Local, NaiveDate, TimeZone};
use crowbot_protocol::{
    ApprovalClosure, ApprovalDecision, ApprovalReason, ConversationSnapshot, Event, FailureCause,
    Interruption, TimedEvent, ToolKind, ToolStatus, TurnEnd,
};

pub const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalState {
    Waiting,
    Allowed,
    Declined,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Thinking,
    Tool(Option<ToolKind>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Running,
    Done,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub id: Option<String>,
    pub kind: StepKind,
    pub title: String,
    pub input: Option<String>,
    pub output: Option<String>,
    pub state: StepState,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
    pub allowed_by_level: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Stopped,
    DaemonRestarted,
    Unresponsive,
    LengthLimit,
    RequestLimit,
    Refused,
    Unfinished,
    Failed {
        message: String,
        cause: Option<FailureCause>,
        provider: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub id: Option<String>,
    pub title: Option<String>,
    pub target: Option<String>,
    pub folder: Option<String>,
    pub cut: bool,
    pub state: ApprovalState,
    pub automatic: Option<ApprovalReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Day,
    User {
        text: String,
    },
    Steps {
        steps: Vec<Step>,
        live: bool,
        started_ms: i64,
        ended_ms: Option<i64>,
    },
    Answer {
        text: String,
        streaming: bool,
    },
    Thought {
        text: String,
    },
    Approval(Approval),
    Status(Status),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub key: i64,
    pub at_ms: i64,
    pub kind: RowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User,
    Bot,
}

impl Row {
    pub fn author(&self) -> Option<Author> {
        match self.kind {
            RowKind::Day => None,
            RowKind::User { .. } => Some(Author::User),
            _ => Some(Author::Bot),
        }
    }

    pub fn is_message(&self) -> bool {
        matches!(self.kind, RowKind::User { .. } | RowKind::Answer { .. })
    }
}

pub fn local_day(at_ms: i64) -> Option<NaiveDate> {
    Local
        .timestamp_millis_opt(at_ms)
        .single()
        .map(|time| time.date_naive())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub first: usize,
    pub before: usize,
    pub after: usize,
}

#[derive(Debug, Default)]
struct Turn {
    started_ms: i64,
    steps: Option<usize>,
    answer: Option<usize>,
    split_text: bool,
}

#[derive(Debug, Default)]
pub struct Transcript {
    events: Vec<TimedEvent>,
    first_seq: i64,
    older: Option<i64>,
    rows: Vec<Row>,
    turn: Option<Turn>,
    turn_active: bool,
    waiting: HashSet<String>,
    last_day: Option<NaiveDate>,
}

impl Transcript {
    pub fn from_snapshot(snapshot: ConversationSnapshot) -> Self {
        let mut transcript = Self {
            older: snapshot.older,
            waiting: snapshot.pending_approvals.iter().cloned().collect(),
            ..Self::default()
        };
        for event in snapshot.events {
            transcript.push(event, true);
        }
        transcript.settle(snapshot.turn_active);
        transcript
    }

    /// A part of the history that stops before the live end of its
    /// conversation: no turn runs in it.
    pub fn from_events(events: Vec<TimedEvent>, older: Option<i64>) -> Self {
        let mut transcript = Self {
            older,
            ..Self::default()
        };
        for event in events {
            transcript.push(event, true);
        }
        transcript.settle(false);
        transcript
    }

    /// Gives the loaded events away. The rows stay as they are.
    pub fn take_events(&mut self) -> Vec<TimedEvent> {
        std::mem::take(&mut self.events)
    }

    /// The id of the first loaded event, when the daemon stored it.
    pub fn first_event_id(&self) -> Option<i64> {
        self.events
            .first()
            .map(|event| event.id)
            .filter(|id| *id > 0)
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn turn_active(&self) -> bool {
        self.turn_active
    }

    pub fn older(&self) -> Option<i64> {
        self.older
    }

    /// Whether the stored event with this id is among the loaded ones.
    pub fn holds_event(&self, id: i64) -> bool {
        self.older.is_none_or(|before| id >= before)
            && self
                .events
                .iter()
                .any(|event| event.id > 0 && event.id <= id)
    }

    /// The row of the message that the stored event with this id is part of.
    /// A text that was stored in several events shows as the first of them.
    pub fn row_of_event(&self, id: i64) -> Option<usize> {
        let index = self
            .events
            .iter()
            .rposition(|event| event.id > 0 && event.id <= id)?;
        let seq = self.first_seq + i64::try_from(index).ok()?;
        self.rows.iter().rposition(|row| {
            row.key.div_euclid(4) <= seq
                && matches!(
                    row.kind,
                    RowKind::User { .. } | RowKind::Answer { .. } | RowKind::Thought { .. }
                )
        })
    }

    pub fn has_waiting_approval(&self) -> bool {
        self.rows.iter().any(|row| {
            matches!(
                &row.kind,
                RowKind::Approval(Approval {
                    state: ApprovalState::Waiting,
                    ..
                })
            )
        })
    }

    pub fn starts_group(&self, index: usize) -> bool {
        let Some(row) = self.rows.get(index) else {
            return false;
        };
        let Some(previous) = index
            .checked_sub(1)
            .and_then(|before| self.rows.get(before))
        else {
            return true;
        };
        previous.author() != row.author() || row.at_ms - previous.at_ms > GROUP_WINDOW_MS
    }

    pub fn apply(&mut self, event: TimedEvent) -> Change {
        let before = self.rows.len();
        match &event.event {
            Event::UserText(_) => self.turn_active = true,
            // A request that arrives live waits on the daemon, as the ones a
            // snapshot names: an older page that loads must not expire it.
            Event::ApprovalRequested { id, .. } => {
                self.waiting.insert(id.clone());
            }
            _ => {}
        }
        let ends = event.event.ends_turn();
        let first = self.push(event, true);
        if ends {
            self.turn_active = false;
            self.waiting.clear();
        }
        Change {
            first: first.min(before),
            before,
            after: self.rows.len(),
        }
    }

    pub fn prepend(&mut self, older_events: Vec<TimedEvent>, older: Option<i64>) -> usize {
        let before = self.rows.len();
        let turn_active = self.turn_active;
        let waiting = std::mem::take(&mut self.waiting);
        let newer = std::mem::take(&mut self.events);
        let mut merged = Self::default();
        for event in older_events {
            merged.push(event, true);
        }
        let added = i64::try_from(merged.events.len()).unwrap_or_default();
        let mut rebuilt = Self {
            first_seq: self.first_seq - added,
            older,
            waiting,
            ..Self::default()
        };
        for event in merged.events {
            rebuilt.push(event, false);
        }
        for (index, event) in newer.into_iter().enumerate() {
            rebuilt.push(event, index > 0);
        }
        *self = rebuilt;
        self.settle(turn_active);
        self.rows.len().saturating_sub(before)
    }

    fn settle(&mut self, turn_active: bool) {
        self.turn_active = turn_active;
        if !turn_active {
            self.waiting.clear();
            self.close_turn(None);
        }
        let waiting = &self.waiting;
        for row in &mut self.rows {
            if let RowKind::Approval(approval) = &mut row.kind
                && approval.state == ApprovalState::Waiting
                && !approval.id.as_ref().is_some_and(|id| waiting.contains(id))
            {
                approval.state = ApprovalState::Expired;
            }
        }
    }

    fn push(&mut self, timed: TimedEvent, merge: bool) -> usize {
        let merged = merge
            && match (
                self.events.last_mut().map(|last| &mut last.event),
                &timed.event,
            ) {
                (Some(Event::MessageText(buffer)), Event::MessageText(text))
                | (Some(Event::ThoughtText(buffer)), Event::ThoughtText(text)) => {
                    buffer.push_str(text);
                    true
                }
                _ => false,
            };
        if !merged {
            self.events.push(timed.clone());
        }
        let seq = self.first_seq + i64::try_from(self.events.len()).unwrap_or_default() - 1;
        self.build(seq, timed)
    }

    fn day_row(&mut self, seq: i64, at_ms: i64) {
        let day = local_day(at_ms);
        if day.is_some() && day != self.last_day {
            self.last_day = day;
            self.rows.push(Row {
                key: key(seq, Slot::Day),
                at_ms,
                kind: RowKind::Day,
            });
        }
    }

    fn turn(&mut self, at_ms: i64) -> &mut Turn {
        self.turn.get_or_insert_with(|| Turn {
            started_ms: at_ms,
            ..Turn::default()
        })
    }

    fn steps_row(&mut self, seq: i64, at_ms: i64) -> usize {
        if let Some(index) = self.turn(at_ms).steps {
            return index;
        }
        self.day_row(seq, at_ms);
        let started_ms = self.turn(at_ms).started_ms;
        let row = Row {
            key: key(seq, Slot::Steps),
            at_ms,
            kind: RowKind::Steps {
                steps: Vec::new(),
                live: true,
                started_ms,
                ended_ms: None,
            },
        };
        let index = match self.turn(at_ms).answer {
            Some(answer) => {
                self.rows.insert(answer, row);
                self.turn(at_ms).answer = Some(answer + 1);
                answer
            }
            None => {
                self.rows.push(row);
                self.rows.len() - 1
            }
        };
        self.turn(at_ms).steps = Some(index);
        index
    }

    fn steps_mut(&mut self, seq: i64, at_ms: i64) -> Option<(usize, &mut Vec<Step>)> {
        let index = self.steps_row(seq, at_ms);
        self.turn(at_ms).split_text = true;
        match &mut self.rows[index].kind {
            RowKind::Steps { steps, .. } => Some((index, steps)),
            _ => None,
        }
    }

    fn build(&mut self, seq: i64, timed: TimedEvent) -> usize {
        let at_ms = timed.at_ms;
        let end = self.rows.len();
        match timed.event {
            Event::UserText(text) => {
                self.close_turn(None);
                self.day_row(seq, at_ms);
                self.turn = Some(Turn {
                    started_ms: at_ms,
                    ..Turn::default()
                });
                self.rows.push(Row {
                    key: key(seq, Slot::Main),
                    at_ms,
                    kind: RowKind::User { text },
                });
                end
            }
            Event::MessageText(text) => {
                if let Some(index) = self.turn(at_ms).answer {
                    let split = std::mem::take(&mut self.turn(at_ms).split_text);
                    if let RowKind::Answer { text: answer, .. } = &mut self.rows[index].kind {
                        if split && !answer.is_empty() && !answer.ends_with("\n\n") {
                            answer.push_str(if answer.ends_with('\n') { "\n" } else { "\n\n" });
                        }
                        answer.push_str(&text);
                    }
                    return index;
                }
                self.day_row(seq, at_ms);
                self.rows.push(Row {
                    key: key(seq, Slot::Main),
                    at_ms,
                    kind: RowKind::Answer {
                        text,
                        streaming: true,
                    },
                });
                let index = self.rows.len() - 1;
                let turn = self.turn(at_ms);
                turn.answer = Some(index);
                turn.split_text = false;
                end.min(index)
            }
            Event::ThoughtText(text) => {
                if text.is_empty() {
                    return end;
                }
                let Some((index, steps)) = self.steps_mut(seq, at_ms) else {
                    return end;
                };
                match steps.last_mut() {
                    Some(step)
                        if step.kind == StepKind::Thinking && step.state == StepState::Running =>
                    {
                        step.title.push_str(&text);
                    }
                    _ => steps.push(Step {
                        id: None,
                        kind: StepKind::Thinking,
                        title: text,
                        input: None,
                        output: None,
                        state: StepState::Running,
                        started_ms: at_ms,
                        ended_ms: None,
                        allowed_by_level: false,
                    }),
                }
                index.min(end)
            }
            Event::ToolCallStarted {
                id,
                title,
                status,
                kind,
                input,
            } => self.tool(seq, at_ms, id, Some(title), status, kind, input, None),
            Event::ToolCallUpdated {
                id,
                title,
                status,
                kind,
                input,
                output,
            } => self.tool(seq, at_ms, id, title, status, kind, input, output),
            Event::ApprovalRequested {
                id,
                title,
                target,
                folder,
                cut,
                ..
            } => {
                self.day_row(seq, at_ms);
                self.rows.push(Row {
                    key: key(seq, Slot::Main),
                    at_ms,
                    kind: RowKind::Approval(Approval {
                        id: Some(id),
                        title,
                        target,
                        folder,
                        cut,
                        state: ApprovalState::Waiting,
                        automatic: None,
                    }),
                });
                end
            }
            Event::ApprovalResolved {
                id,
                tool_call_id,
                title,
                decision,
                reason,
            } => {
                let state = match decision {
                    ApprovalDecision::Allowed => ApprovalState::Allowed,
                    ApprovalDecision::Declined => ApprovalState::Declined,
                };
                if let Some(id) = &id {
                    self.waiting.remove(id);
                }
                if let Some(index) = self.settle_approval(id.as_deref(), state, reason) {
                    return index;
                }
                if reason == Some(ApprovalReason::PermissionLevel) {
                    return self.allowed_by_level(seq, at_ms, &tool_call_id, title);
                }
                self.day_row(seq, at_ms);
                self.rows.push(Row {
                    key: key(seq, Slot::Main),
                    at_ms,
                    kind: RowKind::Approval(Approval {
                        id,
                        title,
                        target: None,
                        folder: None,
                        cut: false,
                        state,
                        automatic: reason,
                    }),
                });
                end
            }
            Event::ApprovalClosed {
                id, title, reason, ..
            } => {
                let state = match reason {
                    ApprovalClosure::Cancelled => ApprovalState::Cancelled,
                    ApprovalClosure::Expired => ApprovalState::Expired,
                };
                self.waiting.remove(&id);
                if let Some(index) = self.settle_approval(Some(&id), state, None) {
                    return index;
                }
                self.day_row(seq, at_ms);
                self.rows.push(Row {
                    key: key(seq, Slot::Main),
                    at_ms,
                    kind: RowKind::Approval(Approval {
                        id: Some(id),
                        title,
                        target: None,
                        folder: None,
                        cut: false,
                        state,
                        automatic: None,
                    }),
                });
                end
            }
            Event::TurnEnded(reason) => {
                let status = match reason {
                    TurnEnd::Completed => None,
                    TurnEnd::Cancelled => Some(Status::Stopped),
                    TurnEnd::MaxTokens => Some(Status::LengthLimit),
                    TurnEnd::MaxRequests => Some(Status::RequestLimit),
                    TurnEnd::Refused => Some(Status::Refused),
                    TurnEnd::Other => Some(Status::Unfinished),
                };
                self.end_turn(seq, at_ms, status)
            }
            Event::TurnInterrupted { cause } => {
                let status = match cause {
                    Interruption::DaemonRestarted => Status::DaemonRestarted,
                    Interruption::AgentUnresponsive => Status::Unresponsive,
                };
                self.end_turn(seq, at_ms, Some(status))
            }
            Event::TurnFailed {
                message,
                cause,
                provider,
            } => self.end_turn(
                seq,
                at_ms,
                Some(Status::Failed {
                    message,
                    cause,
                    provider,
                }),
            ),
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn tool(
        &mut self,
        seq: i64,
        at_ms: i64,
        id: String,
        title: Option<String>,
        status: Option<ToolStatus>,
        kind: Option<ToolKind>,
        input: Option<String>,
        output: Option<String>,
    ) -> usize {
        let end = self.rows.len();
        let Some((index, steps)) = self.steps_mut(seq, at_ms) else {
            return end;
        };
        if let Some(thinking) = steps
            .last_mut()
            .filter(|step| step.kind == StepKind::Thinking && step.state == StepState::Running)
        {
            thinking.state = StepState::Done;
            thinking.ended_ms = Some(at_ms);
        }
        let position = steps
            .iter()
            .rposition(|step| step.id.as_deref() == Some(id.as_str()));
        let step = match position {
            Some(position) => &mut steps[position],
            None => {
                steps.push(Step {
                    id: Some(id),
                    kind: StepKind::Tool(None),
                    title: "Tool call".to_owned(),
                    input: None,
                    output: None,
                    state: StepState::Running,
                    started_ms: at_ms,
                    ended_ms: None,
                    allowed_by_level: false,
                });
                let last = steps.len() - 1;
                &mut steps[last]
            }
        };
        if let Some(title) = title {
            step.title = title;
        }
        if kind.is_some() {
            step.kind = StepKind::Tool(kind);
        }
        if input.is_some() {
            step.input = input;
        }
        if output.is_some() {
            step.output = output;
        }
        match status {
            Some(ToolStatus::Completed) => {
                step.state = StepState::Done;
                step.ended_ms = Some(at_ms);
            }
            Some(ToolStatus::Failed) => {
                step.state = StepState::Failed;
                step.ended_ms = Some(at_ms);
            }
            Some(ToolStatus::Pending | ToolStatus::InProgress) | None => {}
        }
        index.min(end)
    }

    fn allowed_by_level(
        &mut self,
        seq: i64,
        at_ms: i64,
        tool_call_id: &str,
        title: Option<String>,
    ) -> usize {
        let end = self.rows.len();
        let Some((index, steps)) = self.steps_mut(seq, at_ms) else {
            return end;
        };
        match steps
            .iter_mut()
            .rfind(|step| step.id.as_deref() == Some(tool_call_id))
        {
            Some(step) => step.allowed_by_level = true,
            None => steps.push(Step {
                id: Some(tool_call_id.to_owned()),
                kind: StepKind::Tool(None),
                title: title.unwrap_or_else(|| "Tool call".to_owned()),
                input: None,
                output: None,
                state: StepState::Running,
                started_ms: at_ms,
                ended_ms: None,
                allowed_by_level: true,
            }),
        }
        index.min(end)
    }

    fn settle_approval(
        &mut self,
        id: Option<&str>,
        settled: ApprovalState,
        reason: Option<ApprovalReason>,
    ) -> Option<usize> {
        let id = id?;
        let index = self.rows.iter().rposition(|row| {
            matches!(
                &row.kind,
                RowKind::Approval(Approval { id: Some(waiting), state: ApprovalState::Waiting, .. })
                    if waiting == id
            )
        })?;
        if let RowKind::Approval(approval) = &mut self.rows[index].kind {
            approval.state = settled;
            approval.automatic = reason;
        }
        Some(index)
    }

    fn end_turn(&mut self, seq: i64, at_ms: i64, status: Option<Status>) -> usize {
        let end = self.rows.len();
        let first = self.close_turn(Some((seq, at_ms)));
        if let Some(status) = status {
            self.day_row(seq, at_ms);
            self.rows.push(Row {
                key: key(seq, Slot::Main),
                at_ms,
                kind: RowKind::Status(status),
            });
        }
        first.min(end)
    }

    fn close_turn(&mut self, ended: Option<(i64, i64)>) -> usize {
        let mut first = self.rows.len();
        for (index, row) in self.rows.iter_mut().enumerate() {
            if let RowKind::Approval(approval) = &mut row.kind
                && approval.state == ApprovalState::Waiting
            {
                approval.state = ApprovalState::Expired;
                first = first.min(index);
            }
        }
        let Some(turn) = self.turn.take() else {
            return first;
        };
        let ended_ms = ended.map(|(_, at_ms)| at_ms);
        let mut thinking = String::new();
        if let Some(index) = turn.steps
            && let RowKind::Steps {
                steps,
                live,
                ended_ms: steps_ended,
                ..
            } = &mut self.rows[index].kind
        {
            *live = false;
            *steps_ended = ended_ms;
            for step in steps.iter_mut() {
                if step.state == StepState::Running {
                    step.state = match step.kind {
                        StepKind::Thinking => StepState::Done,
                        StepKind::Tool(_) => StepState::Interrupted,
                    };
                    step.ended_ms = ended_ms;
                }
                if step.kind == StepKind::Thinking {
                    if !thinking.is_empty() {
                        thinking.push_str("\n\n");
                    }
                    thinking.push_str(step.title.trim());
                }
            }
            first = first.min(index);
        }
        match turn.answer {
            Some(index) => {
                if let RowKind::Answer { streaming, .. } = &mut self.rows[index].kind {
                    *streaming = false;
                }
                first = first.min(index);
            }
            None => {
                if let (false, Some((seq, at_ms))) = (thinking.is_empty(), ended) {
                    self.rows.push(Row {
                        key: key(seq, Slot::Thought),
                        at_ms,
                        kind: RowKind::Thought { text: thinking },
                    });
                }
            }
        }
        first
    }
}

#[derive(Clone, Copy)]
enum Slot {
    Day,
    Steps,
    Main,
    Thought,
}

fn key(seq: i64, slot: Slot) -> i64 {
    seq * 4 + slot as i64
}

#[cfg(test)]
pub mod tests {
    use crowbot_protocol::{ConversationSummary, QueueState};

    use super::*;

    pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    pub const NOON: i64 = 1_762_862_400_000;

    pub fn at(at_ms: i64, event: Event) -> TimedEvent {
        TimedEvent {
            id: 0,
            at_ms,
            event,
        }
    }

    pub fn snapshot(
        events: Vec<TimedEvent>,
        turn_active: bool,
        pending: &[&str],
    ) -> ConversationSnapshot {
        ConversationSnapshot {
            conversation: ConversationSummary {
                id: 1,
                bot_id: 1,
                title: None,
                created_at_ms: 0,
                last_activity_ms: 0,
            },
            events,
            older: None,
            draft: String::new(),
            turn_active,
            pending_approvals: pending.iter().map(|id| (*id).to_owned()).collect(),
            queue: QueueState::default(),
            turn_model: None,
        }
    }

    pub fn user(text: &str) -> Event {
        Event::UserText(text.to_owned())
    }

    pub fn message(text: &str) -> Event {
        Event::MessageText(text.to_owned())
    }

    pub fn thought(text: &str) -> Event {
        Event::ThoughtText(text.to_owned())
    }

    pub fn tool_started(id: &str, title: &str) -> Event {
        Event::ToolCallStarted {
            id: id.to_owned(),
            title: title.to_owned(),
            status: Some(ToolStatus::InProgress),
            kind: Some(ToolKind::Execute),
            input: Some(format!("run {title}")),
        }
    }

    pub fn tool_ended(id: &str, status: ToolStatus, output: &str) -> Event {
        Event::ToolCallUpdated {
            id: id.to_owned(),
            title: None,
            status: Some(status),
            kind: None,
            input: None,
            output: Some(output.to_owned()),
        }
    }

    pub fn request(id: &str) -> Event {
        Event::ApprovalRequested {
            id: id.to_owned(),
            tool_call_id: "tool".to_owned(),
            title: Some("Write notes.txt".to_owned()),
            target: Some("notes.txt".to_owned()),
            folder: Some("/work".to_owned()),
            cut: false,
        }
    }

    fn timeline(events: Vec<Event>) -> Transcript {
        let mut transcript = Transcript::default();
        for (index, event) in events.into_iter().enumerate() {
            transcript.apply(at(NOON + index as i64 * 1_000, event));
        }
        transcript
    }

    fn kinds(transcript: &Transcript) -> Vec<&'static str> {
        transcript
            .rows()
            .iter()
            .map(|row| match &row.kind {
                RowKind::Day => "day",
                RowKind::User { .. } => "user",
                RowKind::Steps { .. } => "steps",
                RowKind::Answer { .. } => "answer",
                RowKind::Thought { .. } => "thought",
                RowKind::Approval(_) => "approval",
                RowKind::Status(_) => "status",
            })
            .collect()
    }

    fn steps(transcript: &Transcript) -> &[Step] {
        transcript
            .rows()
            .iter()
            .find_map(|row| match &row.kind {
                RowKind::Steps { steps, .. } => Some(steps.as_slice()),
                _ => None,
            })
            .unwrap()
    }

    fn approval(transcript: &Transcript, index: usize) -> &Approval {
        match &transcript.rows()[index].kind {
            RowKind::Approval(approval) => approval,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn streamed_chunks_grow_one_answer_row() {
        let mut transcript = Transcript::default();
        let first = transcript.apply(at(NOON, user("hi")));
        assert_eq!((first.before, first.after), (0, 2));
        transcript.apply(at(NOON + 1, message("Hel")));
        let grown = transcript.apply(at(NOON + 2, message("lo")));
        assert_eq!(
            grown,
            Change {
                first: 2,
                before: 3,
                after: 3
            }
        );
        assert_eq!(kinds(&transcript), ["day", "user", "answer"]);
        assert_eq!(
            transcript.rows()[2].kind,
            RowKind::Answer {
                text: "Hello".to_owned(),
                streaming: true,
            }
        );
        assert!(transcript.turn_active());
        transcript.apply(at(NOON + 3, Event::TurnEnded(TurnEnd::Completed)));
        assert!(!transcript.turn_active());
        assert_eq!(kinds(&transcript), ["day", "user", "answer"]);
        assert!(matches!(
            transcript.rows()[2].kind,
            RowKind::Answer {
                streaming: false,
                ..
            }
        ));
    }

    #[test]
    fn the_steps_of_a_turn_sit_in_one_row_above_its_answer() {
        let transcript = timeline(vec![
            user("go"),
            message("Before."),
            thought("Let me "),
            thought("look."),
            tool_started("t1", "Read"),
            tool_ended("t1", ToolStatus::Completed, "contents"),
            message("After."),
            tool_started("t2", "Test"),
            tool_ended("t2", ToolStatus::Failed, "error: boom"),
            Event::TurnEnded(TurnEnd::Completed),
        ]);
        assert_eq!(kinds(&transcript), ["day", "user", "steps", "answer"]);
        let steps = steps(&transcript);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].kind, StepKind::Thinking);
        assert_eq!(steps[0].title, "Let me look.");
        assert_eq!(steps[0].state, StepState::Done);
        assert_eq!(steps[1].kind, StepKind::Tool(Some(ToolKind::Execute)));
        assert_eq!(steps[1].title, "Read");
        assert_eq!(steps[1].input.as_deref(), Some("run Read"));
        assert_eq!(steps[1].output.as_deref(), Some("contents"));
        assert_eq!(steps[1].state, StepState::Done);
        assert_eq!(steps[1].ended_ms, Some(NOON + 5_000));
        assert_eq!(steps[2].state, StepState::Failed);
        assert_eq!(steps[2].output.as_deref(), Some("error: boom"));
        assert_eq!(
            transcript.rows()[3].kind,
            RowKind::Answer {
                text: "Before.\n\nAfter.".to_owned(),
                streaming: false,
            }
        );
        assert!(matches!(
            transcript.rows()[2].kind,
            RowKind::Steps {
                live: false,
                started_ms: NOON,
                ended_ms: Some(ended),
                ..
            } if ended == NOON + 9_000
        ));
    }

    #[test]
    fn a_tool_call_with_no_end_shows_as_interrupted_once_the_turn_ended() {
        let mut transcript = timeline(vec![user("go"), tool_started("t1", "Build")]);
        assert_eq!(steps(&transcript)[0].state, StepState::Running);
        let changed = transcript.apply(at(NOON + 9_000, Event::TurnEnded(TurnEnd::Cancelled)));
        assert_eq!(changed.first, 2);
        assert_eq!(steps(&transcript)[0].state, StepState::Interrupted);
        assert_eq!(kinds(&transcript), ["day", "user", "steps", "status"]);
        assert_eq!(transcript.rows()[3].kind, RowKind::Status(Status::Stopped));

        let restarted = Transcript::from_snapshot(snapshot(
            vec![
                at(NOON, user("go")),
                at(NOON + 1, tool_started("t1", "Build")),
                at(
                    NOON + 2,
                    Event::TurnInterrupted {
                        cause: Interruption::DaemonRestarted,
                    },
                ),
            ],
            false,
            &[],
        ));
        assert_eq!(steps(&restarted)[0].state, StepState::Interrupted);
        assert_eq!(
            restarted.rows()[3].kind,
            RowKind::Status(Status::DaemonRestarted)
        );
    }

    #[test]
    fn a_stopped_turn_with_no_answer_shows_its_thinking_in_a_row_of_its_own() {
        let transcript = timeline(vec![
            user("go"),
            thought("Weighing the options."),
            Event::TurnEnded(TurnEnd::Cancelled),
        ]);
        assert_eq!(
            kinds(&transcript),
            ["day", "user", "steps", "thought", "status"]
        );
        assert_eq!(
            transcript.rows()[3].kind,
            RowKind::Thought {
                text: "Weighing the options.".to_owned()
            }
        );

        let answered = timeline(vec![
            user("go"),
            thought("Hm."),
            message("Done."),
            Event::TurnEnded(TurnEnd::Completed),
        ]);
        assert_eq!(kinds(&answered), ["day", "user", "steps", "answer"]);
    }

    #[test]
    fn an_empty_thought_adds_no_step() {
        let transcript = timeline(vec![
            user("go"),
            thought(""),
            Event::TurnEnded(TurnEnd::Cancelled),
        ]);
        assert_eq!(kinds(&transcript), ["day", "user", "status"]);
    }

    #[test]
    fn each_way_a_turn_ends_gets_its_status_and_a_normal_end_gets_none() {
        let ended = |end: Event| {
            let transcript = timeline(vec![user("go"), message("partial"), end]);
            assert!(matches!(
                &transcript.rows()[2].kind,
                RowKind::Answer { text, streaming: false } if text == "partial"
            ));
            transcript.rows().get(3).map(|row| row.kind.clone())
        };
        assert_eq!(ended(Event::TurnEnded(TurnEnd::Completed)), None);
        assert_eq!(
            ended(Event::TurnEnded(TurnEnd::Cancelled)),
            Some(RowKind::Status(Status::Stopped))
        );
        assert_eq!(
            ended(Event::TurnInterrupted {
                cause: Interruption::DaemonRestarted
            }),
            Some(RowKind::Status(Status::DaemonRestarted))
        );
        assert_eq!(
            ended(Event::TurnFailed {
                message: "not signed in".to_owned(),
                cause: Some(FailureCause::SignInRequired),
                provider: Some("claude".to_owned()),
            }),
            Some(RowKind::Status(Status::Failed {
                message: "not signed in".to_owned(),
                cause: Some(FailureCause::SignInRequired),
                provider: Some("claude".to_owned()),
            }))
        );
        assert_eq!(
            ended(Event::TurnEnded(TurnEnd::MaxTokens)),
            Some(RowKind::Status(Status::LengthLimit))
        );
    }

    #[test]
    fn a_day_separator_sits_between_messages_of_different_days() {
        let mut transcript = Transcript::default();
        transcript.apply(at(NOON, user("monday")));
        transcript.apply(at(NOON + 1_000, message("yes")));
        transcript.apply(at(NOON + 2_000, Event::TurnEnded(TurnEnd::Completed)));
        transcript.apply(at(NOON + 2 * DAY_MS, user("wednesday")));
        assert_eq!(kinds(&transcript), ["day", "user", "answer", "day", "user"]);
        assert_eq!(transcript.rows()[3].at_ms, NOON + 2 * DAY_MS);
    }

    #[test]
    fn messages_of_one_author_within_five_minutes_form_a_group() {
        let mut transcript = Transcript::default();
        for (offset, event) in [
            (0, user("one")),
            (
                1_000,
                Event::TurnFailed {
                    message: "gone".to_owned(),
                    cause: None,
                    provider: None,
                },
            ),
            (2_000, user("two")),
            (3_000, Event::TurnEnded(TurnEnd::Cancelled)),
            (4_000, user("three")),
            (5_000, thought("hm")),
            (6_000, message("answer")),
            (7_000, Event::TurnEnded(TurnEnd::Completed)),
            (7_000 + GROUP_WINDOW_MS + 1, user("later")),
        ] {
            transcript.apply(at(NOON + offset, event));
        }
        assert_eq!(
            kinds(&transcript),
            [
                "day", "user", "status", "user", "status", "user", "steps", "answer", "user"
            ]
        );
        let groups: Vec<bool> = (0..transcript.rows().len())
            .map(|index| transcript.starts_group(index))
            .collect();
        assert_eq!(
            groups,
            [true, true, true, true, true, true, true, false, true]
        );

        let mut same = Transcript::default();
        same.apply(at(NOON, message("first")));
        same.apply(at(NOON + 1_000, Event::TurnEnded(TurnEnd::Completed)));
        same.apply(at(NOON + 2_000, message("second")));
        same.apply(at(NOON + 3_000, Event::TurnEnded(TurnEnd::Completed)));
        same.apply(at(NOON + 4_000 + GROUP_WINDOW_MS, message("third")));
        assert_eq!(kinds(&same), ["day", "answer", "answer", "answer"]);
        assert!(same.starts_group(1));
        assert!(!same.starts_group(2));
        assert!(same.starts_group(3));
    }

    #[test]
    fn an_answer_settles_the_request_it_names() {
        let mut transcript = timeline(vec![user("go"), request("a1")]);
        assert!(transcript.has_waiting_approval());
        assert_eq!(
            approval(&transcript, 2).target.as_deref(),
            Some("notes.txt")
        );
        assert_eq!(approval(&transcript, 2).folder.as_deref(), Some("/work"));
        let changed = transcript.apply(at(
            NOON + 5_000,
            Event::ApprovalResolved {
                id: Some("a1".to_owned()),
                tool_call_id: "tool".to_owned(),
                title: Some("Write notes.txt".to_owned()),
                decision: ApprovalDecision::Allowed,
                reason: None,
            },
        ));
        assert_eq!(changed.first, 2);
        assert_eq!(approval(&transcript, 2).state, ApprovalState::Allowed);
        assert!(!transcript.has_waiting_approval());
    }

    #[test]
    fn an_answer_given_by_the_permission_level_marks_the_step_and_adds_no_card() {
        let transcript = timeline(vec![
            user("go"),
            tool_started("tool", "Write notes.txt"),
            Event::ApprovalResolved {
                id: Some("a1".to_owned()),
                tool_call_id: "tool".to_owned(),
                title: Some("Write notes.txt".to_owned()),
                decision: ApprovalDecision::Allowed,
                reason: Some(ApprovalReason::PermissionLevel),
            },
        ]);
        assert_eq!(kinds(&transcript), ["day", "user", "steps"]);
        assert!(steps(&transcript)[0].allowed_by_level);

        let waited = timeline(vec![
            user("go"),
            request("a1"),
            Event::ApprovalResolved {
                id: Some("a1".to_owned()),
                tool_call_id: "tool".to_owned(),
                title: None,
                decision: ApprovalDecision::Allowed,
                reason: Some(ApprovalReason::PermissionLevel),
            },
        ]);
        assert_eq!(approval(&waited, 2).state, ApprovalState::Allowed);
        assert_eq!(
            approval(&waited, 2).automatic,
            Some(ApprovalReason::PermissionLevel)
        );
    }

    #[test]
    fn a_refusal_without_a_request_stands_alone() {
        let transcript = timeline(vec![
            user("go"),
            Event::ApprovalResolved {
                id: None,
                tool_call_id: "tool".to_owned(),
                title: Some("Write notes.txt".to_owned()),
                decision: ApprovalDecision::Declined,
                reason: Some(ApprovalReason::NoAllowOnce),
            },
        ]);
        assert_eq!(approval(&transcript, 2).state, ApprovalState::Declined);
        assert_eq!(
            approval(&transcript, 2).automatic,
            Some(ApprovalReason::NoAllowOnce)
        );
    }

    #[test]
    fn the_end_of_a_turn_expires_unanswered_requests() {
        let mut transcript = timeline(vec![user("go"), request("a1")]);
        let changed = transcript.apply(at(
            NOON + 9_000,
            Event::TurnFailed {
                message: "the agent exited".to_owned(),
                cause: None,
                provider: None,
            },
        ));
        assert_eq!(changed.first, 2);
        assert_eq!(approval(&transcript, 2).state, ApprovalState::Expired);
        assert!(matches!(
            &transcript.rows()[3].kind,
            RowKind::Status(Status::Failed { message, .. }) if message == "the agent exited"
        ));
    }

    #[test]
    fn a_stop_closes_the_request_as_cancelled() {
        let mut transcript = timeline(vec![user("go"), request("a1")]);
        transcript.apply(at(
            NOON + 3_000,
            Event::ApprovalClosed {
                id: "a1".to_owned(),
                tool_call_id: "tool".to_owned(),
                title: None,
                reason: ApprovalClosure::Cancelled,
            },
        ));
        transcript.apply(at(NOON + 4_000, Event::TurnEnded(TurnEnd::Cancelled)));
        assert_eq!(approval(&transcript, 2).state, ApprovalState::Cancelled);
        assert!(!transcript.turn_active());
    }

    #[test]
    fn a_snapshot_keeps_only_the_requests_the_daemon_still_waits_on() {
        let events = vec![
            at(NOON, user("go")),
            at(NOON + 1, request("a1")),
            at(NOON + 2, request("a2")),
        ];
        let transcript = Transcript::from_snapshot(snapshot(events.clone(), true, &["a2"]));
        assert!(transcript.turn_active());
        assert_eq!(approval(&transcript, 2).state, ApprovalState::Expired);
        assert_eq!(approval(&transcript, 3).state, ApprovalState::Waiting);

        let idle = Transcript::from_snapshot(snapshot(events, false, &[]));
        assert!(!idle.turn_active());
        assert_eq!(approval(&idle, 2).state, ApprovalState::Expired);
        assert_eq!(approval(&idle, 3).state, ApprovalState::Expired);
    }

    #[test]
    fn a_request_that_arrived_live_still_waits_after_an_older_page_loads() {
        let mut page = snapshot(vec![at(NOON + 10_000, user("go"))], true, &[]);
        page.older = Some(40);
        let mut transcript = Transcript::from_snapshot(page);
        transcript.apply(at(NOON + 11_000, request("a1")));
        assert!(transcript.has_waiting_approval());

        transcript.prepend(
            vec![
                at(NOON, user("first")),
                at(NOON + 1_000, Event::TurnEnded(TurnEnd::Completed)),
            ],
            None,
        );
        assert_eq!(kinds(&transcript), ["day", "user", "user", "approval"]);
        assert_eq!(approval(&transcript, 3).state, ApprovalState::Waiting);

        transcript.apply(at(NOON + 12_000, Event::TurnEnded(TurnEnd::Cancelled)));
        assert_eq!(approval(&transcript, 3).state, ApprovalState::Expired);
    }

    #[test]
    fn an_older_page_joins_the_rows_it_precedes_and_keeps_their_keys() {
        let newer = vec![
            at(NOON + 10_000, message(" world")),
            at(NOON + 11_000, Event::TurnEnded(TurnEnd::Completed)),
            at(NOON + 12_000, user("again")),
            at(NOON + 13_000, request("a1")),
        ];
        let mut page = snapshot(newer, true, &["a1"]);
        page.older = Some(40);
        let mut transcript = Transcript::from_snapshot(page);
        assert_eq!(transcript.older(), Some(40));
        assert_eq!(kinds(&transcript), ["day", "answer", "user", "approval"]);
        let user_key = transcript.rows()[2].key;
        let approval_key = transcript.rows()[3].key;

        let added = transcript.prepend(
            vec![
                at(NOON, user("first")),
                at(NOON + 1_000, tool_started("t1", "Read")),
                at(NOON + 2_000, message("Hello")),
            ],
            None,
        );
        assert_eq!(added, 2);
        assert_eq!(transcript.older(), None);
        assert_eq!(
            kinds(&transcript),
            ["day", "user", "steps", "answer", "user", "approval"]
        );
        assert_eq!(
            transcript.rows()[3].kind,
            RowKind::Answer {
                text: "Hello world".to_owned(),
                streaming: false,
            }
        );
        assert_eq!(transcript.rows()[4].key, user_key);
        assert_eq!(transcript.rows()[5].key, approval_key);
        assert_eq!(approval(&transcript, 5).state, ApprovalState::Waiting);
        assert!(transcript.turn_active());
        assert_eq!(steps(&transcript)[0].state, StepState::Interrupted);

        transcript.apply(at(NOON + 14_000, message("live")));
        assert_eq!(
            kinds(&transcript),
            [
                "day", "user", "steps", "answer", "user", "approval", "answer"
            ]
        );
    }
}
