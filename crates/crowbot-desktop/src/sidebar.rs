use chrono::{DateTime, Datelike, TimeZone, Timelike};
use crowbot_protocol::{BotState, BotSummary};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    Pinned,
    NeedsYou,
    Unread,
    Bots,
    Hidden,
}

impl GroupKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Pinned => "Pinned",
            Self::NeedsYou => "Needs you",
            Self::Unread => "Unread",
            Self::Bots => "Bots",
            Self::Hidden => "Hidden",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: GroupKind,
    pub bots: Vec<i64>,
}

pub fn group_of(summary: &BotSummary) -> GroupKind {
    if summary.state == BotState::Waiting {
        GroupKind::NeedsYou
    } else if summary.bot.hidden {
        GroupKind::Hidden
    } else if summary.bot.pinned {
        GroupKind::Pinned
    } else if matches!(summary.state, BotState::Unread | BotState::Failed) {
        GroupKind::Unread
    } else {
        GroupKind::Bots
    }
}

pub fn groups(bots: &[BotSummary]) -> Vec<Group> {
    [
        GroupKind::Pinned,
        GroupKind::NeedsYou,
        GroupKind::Unread,
        GroupKind::Bots,
        GroupKind::Hidden,
    ]
    .into_iter()
    .filter_map(|kind| {
        let mut members: Vec<&BotSummary> = bots
            .iter()
            .filter(|summary| group_of(summary) == kind)
            .collect();
        if matches!(kind, GroupKind::NeedsYou | GroupKind::Unread) {
            members.sort_by_key(|summary| std::cmp::Reverse(summary.last_activity_ms));
        } else {
            members.sort_by_key(|summary| summary.bot.id);
        }
        (!members.is_empty()).then(|| Group {
            kind,
            bots: members.iter().map(|summary| summary.bot.id).collect(),
        })
    })
    .collect()
}

pub fn unread_bots(bots: &[BotSummary]) -> Vec<i64> {
    bots.iter()
        .filter(|summary| summary.unread > 0)
        .map(|summary| summary.bot.id)
        .collect()
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn activity_label<Zone: TimeZone>(now: &DateTime<Zone>, then_ms: i64) -> String {
    let Some(then) = DateTime::from_timestamp_millis(then_ms) else {
        return String::new();
    };
    let then = then.with_timezone(&now.timezone());
    if then.date_naive() == now.date_naive() {
        format!("{:02}:{:02}", then.hour(), then.minute())
    } else {
        format!("{} {}", MONTHS[then.month0() as usize], then.day())
    }
}

pub fn state_label(summary: &BotSummary) -> Option<String> {
    match summary.state {
        BotState::Waiting => Some("Needs you".to_owned()),
        BotState::Failed => Some("Failed".to_owned()),
        BotState::Working => Some("Working…".to_owned()),
        BotState::Unread => Some(format!("{} new", summary.unread)),
        BotState::Idle => None,
    }
}

pub fn row_label(summary: &BotSummary) -> String {
    match state_label(summary) {
        Some(state) => format!("{}, {state}", summary.bot.name),
        None => summary.bot.name.clone(),
    }
}

#[cfg(test)]
pub mod tests {
    use chrono::FixedOffset;
    use crowbot_protocol::{Avatar, Bot, PermissionLevel};

    use super::*;

    pub fn summary(id: i64, name: &str, state: BotState) -> BotSummary {
        BotSummary {
            bot: Bot {
                id,
                name: name.to_owned(),
                title: String::new(),
                instructions: String::new(),
                avatar: Avatar {
                    color: "#3b82f6".to_owned(),
                    shape: Default::default(),
                },
                provider: "claude".to_owned(),
                model: None,
                effort: None,
                permission_level: PermissionLevel::AskFirst,
                tool_call_limit: None,
                workspace: "/work".to_owned(),
                pinned: false,
                hidden: false,
                created_at_ms: 0,
            },
            current_conversation_id: id * 10,
            state,
            unread: u32::from(matches!(state, BotState::Unread | BotState::Failed)),
            last_activity_ms: id,
            preview: String::new(),
        }
    }

    fn kinds(groups: &[Group]) -> Vec<(GroupKind, Vec<i64>)> {
        groups
            .iter()
            .map(|group| (group.kind, group.bots.clone()))
            .collect()
    }

    #[test]
    fn groups_come_in_order_and_only_when_they_hold_a_bot() {
        let mut pinned = summary(1, "Pinned", BotState::Idle);
        pinned.bot.pinned = true;
        let mut hidden = summary(2, "Hidden", BotState::Idle);
        hidden.bot.hidden = true;
        let bots = [
            summary(5, "Idle", BotState::Idle),
            hidden,
            summary(4, "Unread", BotState::Unread),
            summary(3, "Waiting", BotState::Waiting),
            pinned,
            summary(6, "Working", BotState::Working),
        ];
        assert_eq!(
            kinds(&groups(&bots)),
            [
                (GroupKind::Pinned, vec![1]),
                (GroupKind::NeedsYou, vec![3]),
                (GroupKind::Unread, vec![4]),
                (GroupKind::Bots, vec![5, 6]),
                (GroupKind::Hidden, vec![2]),
            ]
        );
        assert_eq!(kinds(&groups(&bots[..1])), [(GroupKind::Bots, vec![5])]);
        assert!(groups(&[]).is_empty());
    }

    #[test]
    fn a_bot_that_waits_is_in_needs_you_even_when_pinned_or_hidden() {
        let mut pinned = summary(1, "Pinned", BotState::Waiting);
        pinned.bot.pinned = true;
        let mut hidden = summary(2, "Hidden", BotState::Waiting);
        hidden.bot.hidden = true;
        assert_eq!(group_of(&pinned), GroupKind::NeedsYou);
        assert_eq!(group_of(&hidden), GroupKind::NeedsYou);

        let mut both = summary(3, "Both", BotState::Unread);
        both.bot.pinned = true;
        assert_eq!(group_of(&both), GroupKind::Pinned);
        both.bot.hidden = true;
        assert_eq!(group_of(&both), GroupKind::Hidden);
        assert_eq!(
            group_of(&summary(4, "Failed", BotState::Failed)),
            GroupKind::Unread
        );
    }

    #[test]
    fn a_state_change_moves_the_bot_to_its_group() {
        let mut bots = vec![
            summary(1, "Atlas", BotState::Idle),
            summary(2, "Orion", BotState::Idle),
        ];
        assert_eq!(kinds(&groups(&bots)), [(GroupKind::Bots, vec![1, 2])]);
        bots[1] = summary(2, "Orion", BotState::Waiting);
        assert_eq!(
            kinds(&groups(&bots)),
            [(GroupKind::NeedsYou, vec![2]), (GroupKind::Bots, vec![1])]
        );
        bots[1] = summary(2, "Orion", BotState::Unread);
        assert_eq!(
            kinds(&groups(&bots)),
            [(GroupKind::Unread, vec![2]), (GroupKind::Bots, vec![1])]
        );
    }

    #[test]
    fn the_groups_that_call_for_attention_put_the_latest_activity_first() {
        let bots = [
            summary(1, "Old", BotState::Waiting),
            summary(2, "New", BotState::Waiting),
        ];
        assert_eq!(kinds(&groups(&bots)), [(GroupKind::NeedsYou, vec![2, 1])]);
    }

    #[test]
    fn the_last_activity_reads_as_a_time_today_and_as_a_date_before() {
        let zone = FixedOffset::east_opt(3600).unwrap();
        let now = zone.with_ymd_and_hms(2026, 11, 14, 18, 30, 0).unwrap();
        let at = |hour, minute| {
            zone.with_ymd_and_hms(2026, 11, 14, hour, minute, 0)
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(activity_label(&now, at(9, 5)), "09:05");
        assert_eq!(activity_label(&now, at(0, 0)), "00:00");
        let yesterday = zone
            .with_ymd_and_hms(2026, 11, 13, 23, 59, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(activity_label(&now, yesterday), "Nov 13");
        let earlier = zone
            .with_ymd_and_hms(2026, 3, 2, 12, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(activity_label(&now, earlier), "Mar 2");
    }

    #[test]
    fn a_row_reads_its_name_then_its_state() {
        assert_eq!(row_label(&summary(1, "Atlas", BotState::Idle)), "Atlas");
        assert_eq!(
            row_label(&summary(1, "Atlas", BotState::Waiting)),
            "Atlas, Needs you"
        );
        assert_eq!(
            row_label(&summary(1, "Atlas", BotState::Working)),
            "Atlas, Working…"
        );
        assert_eq!(
            row_label(&summary(1, "Atlas", BotState::Unread)),
            "Atlas, 1 new"
        );
        assert_eq!(
            row_label(&summary(1, "Atlas", BotState::Failed)),
            "Atlas, Failed"
        );
    }

    #[test]
    fn mark_all_read_targets_the_bots_with_something_unread() {
        let bots = [
            summary(1, "Atlas", BotState::Idle),
            summary(2, "Orion", BotState::Unread),
            summary(3, "Vega", BotState::Failed),
        ];
        assert_eq!(unread_bots(&bots), [2, 3]);
        assert!(unread_bots(&bots[..1]).is_empty());
    }
}
