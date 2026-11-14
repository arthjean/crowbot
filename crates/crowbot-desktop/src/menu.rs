use crowbot_protocol::BotSummary;

use crate::sidebar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    MarkRead(i64),
    MarkUnread(i64),
    NewChat(i64),
    Edit(i64),
    Duplicate(i64),
    SetPinned(i64, bool),
    SetHidden(i64, bool),
    Delete(i64),
    NewBot,
    MarkAllRead,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub label: &'static str,
    pub action: MenuAction,
    pub enabled: bool,
    pub danger: bool,
    pub separated: bool,
}

fn entry(label: &'static str, action: MenuAction) -> MenuEntry {
    MenuEntry {
        label,
        action,
        enabled: true,
        danger: false,
        separated: false,
    }
}

pub fn bot_menu(summary: &BotSummary) -> Vec<MenuEntry> {
    let id = summary.bot.id;
    vec![
        if summary.unread > 0 {
            entry("Mark read", MenuAction::MarkRead(id))
        } else {
            entry("Mark unread", MenuAction::MarkUnread(id))
        },
        entry("New chat", MenuAction::NewChat(id)),
        entry("Edit", MenuAction::Edit(id)),
        entry("Duplicate", MenuAction::Duplicate(id)),
        if summary.bot.pinned {
            entry("Unpin", MenuAction::SetPinned(id, false))
        } else {
            entry("Pin", MenuAction::SetPinned(id, true))
        },
        if summary.bot.hidden {
            entry("Restore", MenuAction::SetHidden(id, false))
        } else {
            entry("Hide", MenuAction::SetHidden(id, true))
        },
        MenuEntry {
            danger: true,
            separated: true,
            ..entry("Delete", MenuAction::Delete(id))
        },
    ]
}

pub fn area_menu(bots: &[BotSummary]) -> Vec<MenuEntry> {
    vec![
        entry("New bot", MenuAction::NewBot),
        MenuEntry {
            enabled: !sidebar::unread_bots(bots).is_empty(),
            ..entry("Mark all read", MenuAction::MarkAllRead)
        },
    ]
}

pub fn step(entries: &[MenuEntry], from: Option<usize>, forward: bool) -> Option<usize> {
    let count = entries.len();
    let start = match (from, forward) {
        (Some(from), true) => from + 1,
        (Some(from), false) => from + count - 1,
        (None, true) => 0,
        (None, false) => count.saturating_sub(1),
    };
    (0..count)
        .map(|offset| {
            if forward {
                (start + offset) % count
            } else {
                (start + count - offset) % count
            }
        })
        .find(|index| entries[*index].enabled)
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::BotState;

    use super::*;
    use crate::sidebar::tests::summary;

    fn labels(entries: &[MenuEntry]) -> Vec<&'static str> {
        entries.iter().map(|entry| entry.label).collect()
    }

    #[test]
    fn the_bot_menu_offers_every_action_in_order() {
        let read = summary(7, "Atlas", BotState::Idle);
        let menu = bot_menu(&read);
        assert_eq!(
            labels(&menu),
            [
                "Mark unread",
                "New chat",
                "Edit",
                "Duplicate",
                "Pin",
                "Hide",
                "Delete"
            ]
        );
        assert!(menu.iter().all(|entry| entry.enabled));
        let delete = menu.last().unwrap();
        assert!(delete.danger && delete.separated);
        assert_eq!(delete.action, MenuAction::Delete(7));
        assert_eq!(menu[4].action, MenuAction::SetPinned(7, true));
        assert_eq!(menu[5].action, MenuAction::SetHidden(7, true));
    }

    #[test]
    fn the_bot_menu_follows_the_state_of_the_bot() {
        let mut bot = summary(7, "Atlas", BotState::Unread);
        bot.bot.pinned = true;
        bot.bot.hidden = true;
        let menu = bot_menu(&bot);
        assert_eq!(
            labels(&menu),
            [
                "Mark read",
                "New chat",
                "Edit",
                "Duplicate",
                "Unpin",
                "Restore",
                "Delete"
            ]
        );
        assert_eq!(menu[0].action, MenuAction::MarkRead(7));
        assert_eq!(menu[4].action, MenuAction::SetPinned(7, false));
        assert_eq!(menu[5].action, MenuAction::SetHidden(7, false));
    }

    #[test]
    fn the_empty_area_menu_disables_mark_all_read_when_nothing_is_unread() {
        let read = [summary(1, "Atlas", BotState::Idle)];
        let menu = area_menu(&read);
        assert_eq!(labels(&menu), ["New bot", "Mark all read"]);
        assert!(menu[0].enabled);
        assert!(!menu[1].enabled);

        let unread = [summary(1, "Atlas", BotState::Unread)];
        assert!(area_menu(&unread)[1].enabled);
        assert!(!area_menu(&[])[1].enabled);
    }

    #[test]
    fn the_keyboard_moves_over_the_enabled_entries_and_wraps() {
        let menu = area_menu(&[]);
        assert_eq!(step(&menu, None, true), Some(0));
        assert_eq!(step(&menu, Some(0), true), Some(0));
        assert_eq!(step(&menu, Some(0), false), Some(0));

        let full = bot_menu(&summary(1, "Atlas", BotState::Idle));
        assert_eq!(step(&full, None, true), Some(0));
        assert_eq!(step(&full, None, false), Some(6));
        assert_eq!(step(&full, Some(6), true), Some(0));
        assert_eq!(step(&full, Some(0), false), Some(6));
        assert_eq!(step(&full, Some(2), true), Some(3));
        assert_eq!(step(&[], None, true), None);
    }
}
