use crowbot_protocol::{
    ApprovalClosure, ApprovalDecision, ConversationSnapshot, Event, Interruption, ToolStatus,
    TurnEnd,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    User(String),
    Assistant(String),
    Thought(String),
    Tool {
        id: String,
        title: String,
        status: Option<ToolStatus>,
    },
    Approval {
        id: Option<String>,
        title: Option<String>,
        state: ApprovalState,
    },
    Notice(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalState {
    Waiting,
    Allowed,
    Declined,
    Cancelled,
    Expired,
}

#[derive(Debug, Default)]
pub struct Transcript {
    items: Vec<Item>,
    turn_active: bool,
}

impl Transcript {
    pub fn from_snapshot(snapshot: ConversationSnapshot) -> Self {
        let mut transcript = Self::default();
        for event in snapshot.events {
            transcript.apply(event);
        }
        transcript.turn_active = snapshot.turn_active;
        for item in &mut transcript.items {
            if let Item::Approval { id, state, .. } = item
                && *state == ApprovalState::Waiting
            {
                let waiting = snapshot.turn_active
                    && id
                        .as_ref()
                        .is_some_and(|id| snapshot.pending_approvals.contains(id));
                if !waiting {
                    *state = ApprovalState::Expired;
                }
            }
        }
        transcript
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn turn_active(&self) -> bool {
        self.turn_active
    }

    pub fn apply(&mut self, event: Event) -> usize {
        let end = self.items.len();
        match event {
            Event::UserText(text) => {
                self.turn_active = true;
                self.items.push(Item::User(text));
                end
            }
            Event::MessageText(text) => match self.items.last_mut() {
                Some(Item::Assistant(message)) => {
                    message.push_str(&text);
                    end - 1
                }
                _ => {
                    self.items.push(Item::Assistant(text));
                    end
                }
            },
            Event::ThoughtText(text) => match self.items.last_mut() {
                Some(Item::Thought(thought)) => {
                    thought.push_str(&text);
                    end - 1
                }
                _ => {
                    self.items.push(Item::Thought(text));
                    end
                }
            },
            Event::ToolCallStarted { id, title, status } => {
                self.update_tool(id, Some(title), status)
            }
            Event::ToolCallUpdated { id, title, status } => self.update_tool(id, title, status),
            Event::ApprovalRequested { id, title, .. } => {
                self.items.push(Item::Approval {
                    id: Some(id),
                    title,
                    state: ApprovalState::Waiting,
                });
                end
            }
            Event::ApprovalResolved {
                id,
                title,
                decision,
                ..
            } => {
                let decided = match decision {
                    ApprovalDecision::Allowed => ApprovalState::Allowed,
                    ApprovalDecision::Declined => ApprovalState::Declined,
                };
                self.settle_approval(id, title, decided)
            }
            Event::ApprovalClosed {
                id, title, reason, ..
            } => {
                let closed = match reason {
                    ApprovalClosure::Cancelled => ApprovalState::Cancelled,
                    ApprovalClosure::Expired => ApprovalState::Expired,
                };
                self.settle_approval(Some(id), title, closed)
            }
            Event::TurnInterrupted { cause } => {
                let first = self.end_turn();
                self.items.push(Item::Notice(
                    match cause {
                        Interruption::DaemonRestarted => "The daemon restarted during this turn.",
                        Interruption::AgentUnresponsive => {
                            "The bot did not stop in time, so its process was closed."
                        }
                    }
                    .to_owned(),
                ));
                first
            }
            Event::TurnEnded(reason) => {
                let first = self.end_turn();
                if let Some(notice) = turn_end_notice(reason) {
                    self.items.push(Item::Notice(notice.to_owned()));
                }
                first
            }
            Event::TurnFailed { message } => {
                let first = self.end_turn();
                self.items
                    .push(Item::Notice(format!("The bot stopped: {message}")));
                first
            }
        }
    }

    fn settle_approval(
        &mut self,
        id: Option<String>,
        title: Option<String>,
        settled: ApprovalState,
    ) -> usize {
        let waiting = self.items.iter().rposition(|item| {
            matches!(
                item,
                Item::Approval { id: waiting, state: ApprovalState::Waiting, .. }
                    if id.is_some() && *waiting == id
            )
        });
        match waiting {
            Some(index) => {
                if let Item::Approval { state, .. } = &mut self.items[index] {
                    *state = settled;
                }
                index
            }
            None => {
                self.items.push(Item::Approval {
                    id,
                    title,
                    state: settled,
                });
                self.items.len() - 1
            }
        }
    }

    fn update_tool(
        &mut self,
        id: String,
        title: Option<String>,
        status: Option<ToolStatus>,
    ) -> usize {
        let end = self.items.len();
        let existing = self
            .items
            .iter()
            .rposition(|item| matches!(item, Item::Tool { id: known, .. } if *known == id));
        match existing {
            Some(index) => {
                if let Item::Tool {
                    title: known_title,
                    status: known_status,
                    ..
                } = &mut self.items[index]
                {
                    if let Some(title) = title {
                        *known_title = title;
                    }
                    if status.is_some() {
                        *known_status = status;
                    }
                }
                index
            }
            None => {
                self.items.push(Item::Tool {
                    id,
                    title: title.unwrap_or_else(|| "Tool call".to_owned()),
                    status,
                });
                end
            }
        }
    }

    fn end_turn(&mut self) -> usize {
        self.turn_active = false;
        let mut first = self.items.len();
        for (index, item) in self.items.iter_mut().enumerate() {
            match item {
                Item::Approval { state, .. } if *state == ApprovalState::Waiting => {
                    *state = ApprovalState::Expired;
                    first = first.min(index);
                }
                Item::Tool { status, .. }
                    if matches!(status, Some(ToolStatus::Pending | ToolStatus::InProgress)) =>
                {
                    *status = None;
                    first = first.min(index);
                }
                _ => {}
            }
        }
        first
    }
}

fn turn_end_notice(reason: TurnEnd) -> Option<&'static str> {
    match reason {
        TurnEnd::Completed => None,
        TurnEnd::MaxTokens => Some("The bot stopped: the reply reached its length limit."),
        TurnEnd::MaxRequests => Some("The bot stopped: the turn reached its request limit."),
        TurnEnd::Refused => Some("The bot declined to continue."),
        TurnEnd::Cancelled => Some("The turn was cancelled."),
        TurnEnd::Other => Some("The bot stopped before finishing."),
    }
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{ConversationSummary, QueueState};

    use super::*;

    fn snapshot(events: Vec<Event>, turn_active: bool, pending: &[&str]) -> ConversationSnapshot {
        ConversationSnapshot {
            conversation: ConversationSummary {
                id: 1,
                title: None,
                cwd: "/work".to_owned(),
                created_at_ms: 0,
                last_activity_ms: 0,
            },
            events,
            turn_active,
            pending_approvals: pending.iter().map(|id| (*id).to_owned()).collect(),
            queue: QueueState::default(),
        }
    }

    fn request(id: &str) -> Event {
        Event::ApprovalRequested {
            id: id.to_owned(),
            tool_call_id: "tool".to_owned(),
            title: Some("Write notes.txt".to_owned()),
        }
    }

    fn approval(id: Option<&str>, state: ApprovalState) -> Item {
        Item::Approval {
            id: id.map(str::to_owned),
            title: Some("Write notes.txt".to_owned()),
            state,
        }
    }

    #[test]
    fn streamed_chunks_grow_one_message() {
        let mut transcript = Transcript::default();
        assert_eq!(transcript.apply(Event::UserText("hi".to_owned())), 0);
        assert_eq!(transcript.apply(Event::MessageText("Hel".to_owned())), 1);
        assert_eq!(transcript.apply(Event::MessageText("lo".to_owned())), 1);
        assert_eq!(
            transcript.items(),
            [
                Item::User("hi".to_owned()),
                Item::Assistant("Hello".to_owned())
            ]
        );
        assert!(transcript.turn_active());
        transcript.apply(Event::TurnEnded(TurnEnd::Completed));
        assert!(!transcript.turn_active());
        assert_eq!(transcript.items().len(), 2);
    }

    #[test]
    fn a_tool_call_between_two_chunks_splits_the_message() {
        let mut transcript = Transcript::default();
        transcript.apply(Event::MessageText("before".to_owned()));
        transcript.apply(Event::ToolCallStarted {
            id: "t1".to_owned(),
            title: "Read".to_owned(),
            status: Some(ToolStatus::Pending),
        });
        transcript.apply(Event::MessageText("after".to_owned()));
        assert_eq!(transcript.items().len(), 3);
    }

    #[test]
    fn a_tool_update_changes_the_call_it_names() {
        let mut transcript = Transcript::default();
        transcript.apply(Event::ToolCallStarted {
            id: "t1".to_owned(),
            title: "Read".to_owned(),
            status: Some(ToolStatus::Pending),
        });
        transcript.apply(Event::MessageText("text".to_owned()));
        let changed = transcript.apply(Event::ToolCallUpdated {
            id: "t1".to_owned(),
            title: None,
            status: Some(ToolStatus::Completed),
        });
        assert_eq!(changed, 0);
        assert_eq!(
            transcript.items()[0],
            Item::Tool {
                id: "t1".to_owned(),
                title: "Read".to_owned(),
                status: Some(ToolStatus::Completed),
            }
        );
    }

    #[test]
    fn an_answer_settles_the_request_it_names() {
        let mut transcript = Transcript::default();
        transcript.apply(request("a1"));
        let changed = transcript.apply(Event::ApprovalResolved {
            id: Some("a1".to_owned()),
            tool_call_id: "tool".to_owned(),
            title: Some("Write notes.txt".to_owned()),
            decision: ApprovalDecision::Allowed,
        });
        assert_eq!(changed, 0);
        assert_eq!(
            transcript.items(),
            [approval(Some("a1"), ApprovalState::Allowed)]
        );
    }

    #[test]
    fn an_answer_without_a_request_stands_alone() {
        let mut transcript = Transcript::default();
        transcript.apply(Event::ApprovalResolved {
            id: None,
            tool_call_id: "tool".to_owned(),
            title: Some("Write notes.txt".to_owned()),
            decision: ApprovalDecision::Declined,
        });
        assert_eq!(
            transcript.items(),
            [approval(None, ApprovalState::Declined)]
        );
    }

    #[test]
    fn the_end_of_a_turn_expires_unanswered_requests_and_unfinished_tools() {
        let mut transcript = Transcript::default();
        transcript.apply(Event::UserText("go".to_owned()));
        transcript.apply(Event::ToolCallStarted {
            id: "t1".to_owned(),
            title: "Write".to_owned(),
            status: Some(ToolStatus::InProgress),
        });
        transcript.apply(request("a1"));
        let changed = transcript.apply(Event::TurnFailed {
            message: "the agent exited".to_owned(),
        });
        assert_eq!(changed, 1);
        assert_eq!(
            transcript.items()[1],
            Item::Tool {
                id: "t1".to_owned(),
                title: "Write".to_owned(),
                status: None,
            }
        );
        assert_eq!(
            transcript.items()[2],
            approval(Some("a1"), ApprovalState::Expired)
        );
        assert_eq!(
            transcript.items()[3],
            Item::Notice("The bot stopped: the agent exited".to_owned())
        );
    }

    #[test]
    fn a_stop_closes_the_request_as_cancelled() {
        let mut transcript = Transcript::default();
        transcript.apply(Event::UserText("go".to_owned()));
        transcript.apply(request("a1"));
        let changed = transcript.apply(Event::ApprovalClosed {
            id: "a1".to_owned(),
            tool_call_id: "tool".to_owned(),
            title: Some("Write notes.txt".to_owned()),
            reason: ApprovalClosure::Cancelled,
        });
        assert_eq!(changed, 1);
        transcript.apply(Event::TurnEnded(TurnEnd::Cancelled));
        assert_eq!(
            transcript.items()[1],
            approval(Some("a1"), ApprovalState::Cancelled)
        );
        assert!(!transcript.turn_active());
    }

    #[test]
    fn a_turn_interrupted_by_a_restart_is_over_and_says_why() {
        let events = vec![
            Event::UserText("go".to_owned()),
            Event::MessageText("partial".to_owned()),
            request("a1"),
            Event::ApprovalClosed {
                id: "a1".to_owned(),
                tool_call_id: "tool".to_owned(),
                title: Some("Write notes.txt".to_owned()),
                reason: ApprovalClosure::Expired,
            },
            Event::TurnInterrupted {
                cause: Interruption::DaemonRestarted,
            },
        ];
        let transcript = Transcript::from_snapshot(snapshot(events, false, &[]));
        assert!(!transcript.turn_active());
        assert_eq!(
            transcript.items()[1..],
            [
                Item::Assistant("partial".to_owned()),
                approval(Some("a1"), ApprovalState::Expired),
                Item::Notice("The daemon restarted during this turn.".to_owned()),
            ]
        );
    }

    #[test]
    fn a_snapshot_keeps_only_the_requests_the_daemon_still_waits_on() {
        let events = vec![
            Event::UserText("go".to_owned()),
            request("a1"),
            request("a2"),
        ];
        let transcript = Transcript::from_snapshot(snapshot(events, true, &["a2"]));
        assert!(transcript.turn_active());
        assert_eq!(
            transcript.items()[1..],
            [
                approval(Some("a1"), ApprovalState::Expired),
                approval(Some("a2"), ApprovalState::Waiting)
            ]
        );
    }

    #[test]
    fn a_snapshot_of_an_idle_conversation_has_no_waiting_request() {
        let events = vec![Event::UserText("go".to_owned()), request("a1")];
        let transcript = Transcript::from_snapshot(snapshot(events, false, &[]));
        assert!(!transcript.turn_active());
        assert_eq!(
            transcript.items()[1],
            approval(Some("a1"), ApprovalState::Expired)
        );
    }
}
