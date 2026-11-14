use std::time::{SystemTime, UNIX_EPOCH};

use crowbot_protocol::{
    ApprovalDecision, ApprovalReason, ClientMessage, ConversationSummary, ToolStatus,
};
use gpui::{
    AnyElement, Context, Div, ElementId, Focusable, FontWeight, Role, SharedString, Text, Window,
    div, list, prelude::*,
};

use super::{Workspace, row_id};
use crate::assets::Icon;
use crate::bots;
use crate::theme::{Theme, control, icon as icon_size, layout, radius, space, text};
use crate::transcript::{ApprovalState, Item};
use crate::ui::{self, ButtonKind};

fn readable(index: usize, text: &str) -> Text {
    Text::new(
        ElementId::from(("message", index)),
        SharedString::from(text.to_owned()),
    )
}

impl Workspace {
    fn render_history(
        &self,
        conversations: &[ConversationSummary],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.selected_bot().map(|bot| bot.current_conversation_id);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .unwrap_or_default();
        let (hover, focus) = (theme.control_hover, theme.border_focus);
        let rows = conversations.iter().map(|summary| {
            let id = summary.id;
            let title: SharedString = summary
                .title
                .clone()
                .unwrap_or_else(|| "New chat".to_owned())
                .into();
            let mut detail = bots::ago(now_ms, summary.last_activity_ms);
            if current == Some(id) {
                detail = format!("Current · {detail}");
            }
            let selected = self.selected == Some(id);
            div()
                .id(("history", row_id(id)))
                .focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(SharedString::from(format!("{title}, {detail}")))
                .flex_none()
                .h(control::LG)
                .px(space::S3)
                .flex()
                .items_center()
                .gap(space::S3)
                .rounded(radius::MD)
                .border_1()
                .border_color(gpui::transparent_black())
                .focus_visible(move |style| style.border_color(focus))
                .cursor_pointer()
                .when(selected, |row| row.bg(theme.surface))
                .when(!selected, |row| row.hover(move |style| style.bg(hover)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_from_history(id, window, cx);
                }))
                .child(div().flex_1().min_w_0().truncate().child(title))
                .child(
                    div()
                        .flex_none()
                        .text_size(text::SM.size)
                        .line_height(text::SM.leading)
                        .text_color(theme.text_readable)
                        .child(SharedString::from(detail)),
                )
        });
        div()
            .size_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("history")
                    .role(Role::List)
                    .aria_label("Conversations of this bot")
                    .w_full()
                    .max_w(layout::COLUMN_WIDTH)
                    .h_full()
                    .overflow_y_scroll()
                    .px(space::S6)
                    .py(space::S4)
                    .flex()
                    .flex_col()
                    .gap(space::S0_5)
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_item(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx);
        let Some((item, last)) = self.open.as_ref().and_then(|open| {
            let items = open.transcript.items();
            Some((items.get(index)?, index + 1 == items.len()))
        }) else {
            return div().into_any_element();
        };
        let message = || div().text_size(text::LG.size).line_height(text::LG.leading);
        let (top, body) = match item {
            Item::User(text) => (
                space::S6,
                div()
                    .flex()
                    .justify_end()
                    .child(
                        message()
                            .max_w(layout::BUBBLE_MAX_WIDTH)
                            .px(layout::BUBBLE_PADDING_X)
                            .py(layout::BUBBLE_PADDING_Y)
                            .rounded(radius::BUBBLE)
                            .bg(theme.surface)
                            .child(readable(index, text)),
                    )
                    .into_any_element(),
            ),
            Item::Assistant(text) => (
                space::S4,
                message()
                    .child(readable(index, text.trim_matches('\n')))
                    .into_any_element(),
            ),
            Item::Thought(text) => (
                space::S3,
                div()
                    .pl(layout::THOUGHT_INDENT)
                    .border_l_2()
                    .border_color(theme.border_strong)
                    .text_color(theme.text_muted)
                    .child(readable(index, text.trim_matches('\n')))
                    .into_any_element(),
            ),
            Item::Tool { title, status, .. } => {
                let (label, color) = match status {
                    Some(ToolStatus::Pending) => ("Waiting", theme.text_muted),
                    Some(ToolStatus::InProgress) => ("Running", theme.text_muted),
                    Some(ToolStatus::Completed) => ("Done", theme.success_text),
                    Some(ToolStatus::Failed) => ("Failed", theme.danger_text),
                    None => ("", theme.text_muted),
                };
                (
                    space::S2,
                    div()
                        .flex()
                        .gap(space::S2)
                        .text_color(theme.text_muted)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(title.clone())),
                        )
                        .child(div().flex_none().text_color(color).child(label))
                        .into_any_element(),
                )
            }
            Item::Approval {
                id,
                title,
                state,
                automatic,
            } => (
                space::S3,
                self.render_approval(
                    index,
                    id.clone(),
                    title.clone(),
                    (*state, *automatic),
                    &theme,
                    cx,
                ),
            ),
            Item::Notice(text) => (
                space::S3,
                div()
                    .text_color(theme.danger_text)
                    .child(readable(index, text))
                    .into_any_element(),
            ),
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(layout::COLUMN_WIDTH)
                    .px(space::S6)
                    .pt(top)
                    .when(last, |column| column.pb(space::S6))
                    .child(body),
            )
            .into_any_element()
    }

    fn render_approval(
        &self,
        index: usize,
        id: Option<String>,
        title: Option<String>,
        (state, automatic): (ApprovalState, Option<ApprovalReason>),
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = title.unwrap_or_else(|| "use a tool".to_owned());
        let settled = |label: &'static str, color| {
            div()
                .flex()
                .gap(space::S2)
                .child(div().flex_none().text_color(color).child(label))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text_muted)
                        .child(SharedString::from(title.clone())),
                )
                .into_any_element()
        };
        let id = match (state, id) {
            (ApprovalState::Waiting, Some(id)) => id,
            (ApprovalState::Allowed, _) => {
                let label = match automatic {
                    Some(ApprovalReason::PermissionLevel) => "Allowed by Full access",
                    _ => "Allowed",
                };
                return settled(label, theme.success_text);
            }
            (ApprovalState::Declined, _) => {
                let label = match automatic {
                    Some(ApprovalReason::NoAllowOnce) => "Declined: it could not be allowed once",
                    _ => "Declined",
                };
                return settled(label, theme.danger_text);
            }
            (ApprovalState::Cancelled, _) => return settled("Cancelled", theme.text_muted),
            (ApprovalState::Expired | ApprovalState::Waiting, _) => {
                return settled("Not answered", theme.text_muted);
            }
        };
        let allow_id = id.clone();
        div()
            .id(("approval", index))
            .role(Role::Group)
            .aria_label(SharedString::from(format!("The bot asks to {title}")))
            .p(space::S4)
            .rounded(radius::LG)
            .bg(theme.warning_soft)
            .border_1()
            .border_color(theme.border_strong)
            .flex()
            .flex_col()
            .gap(space::S3)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S0_5)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S1_5)
                            .text_color(theme.warning_text)
                            .font_weight(FontWeight::MEDIUM)
                            .child(ui::icon(
                                Icon::ShieldCheck,
                                icon_size::SM,
                                theme.warning_text,
                            ))
                            .child("The bot asks for your approval"),
                    )
                    .child(
                        div()
                            .text_size(text::LG.size)
                            .line_height(text::LG.leading)
                            .child(SharedString::from(title)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(space::S2)
                    .child(
                        ui::button(("allow", index), "Allow once", ButtonKind::Primary, theme)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.resolve_approval(allow_id.clone(), ApprovalDecision::Allowed);
                                this.focus_composer(window, cx);
                            })),
                    )
                    .child(
                        ui::button(("decline", index), "Decline", ButtonKind::Outline, theme)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.resolve_approval(id.clone(), ApprovalDecision::Declined);
                                this.focus_composer(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_transcript(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let centered = || {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(space::S3)
                .p(space::S6)
                .text_color(theme.text_muted)
        };
        if let Some(history) = &self.history {
            return self.render_history(history, theme, cx);
        }
        if let Some(reason) = &self.load_error {
            return centered()
                .id("conversation-error")
                .debug_selector(|| "conversation-error".into())
                .role(Role::Alert)
                .aria_label(SharedString::from(format!(
                    "This conversation could not be loaded. {reason}"
                )))
                .child(ui::icon(
                    Icon::TriangleAlert,
                    icon_size::LG,
                    theme.danger_text,
                ))
                .child(
                    div()
                        .text_size(text::LG.size)
                        .line_height(text::LG.leading)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child("This conversation could not be loaded"),
                )
                .child(
                    div()
                        .max_w(layout::NOTICE_WIDTH)
                        .text_center()
                        .child(reason.clone()),
                )
                .child(
                    ui::button("retry-load", "Retry", ButtonKind::Outline, theme).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.retry_load();
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element();
        }
        match &self.open {
            Some(open) if open.transcript.items().is_empty() => centered()
                .child("Send a message to start this conversation.")
                .into_any_element(),
            Some(open) => list(
                open.list.clone(),
                cx.processor(|this, index, _, cx| this.render_item(index, cx)),
            )
            .size_full()
            .into_any_element(),
            None if self.selected_bot.is_none() => centered()
                .child("Create a bot to start.")
                .into_any_element(),
            None => centered()
                .id("conversation-loading")
                .debug_selector(|| "conversation-loading".into())
                .role(Role::Status)
                .aria_label("Opening the conversation")
                .child(ui::spinner(
                    "conversation-spinner",
                    icon_size::MD,
                    theme.text_muted,
                ))
                .child("Opening the conversation…")
                .into_any_element(),
        }
    }

    fn render_queue(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let queue = &self.open.as_ref()?.queue;
        if queue.messages.is_empty() {
            return None;
        }
        let heading = if queue.paused {
            "Queue paused: the last turn did not finish."
        } else if queue.messages.len() == 1 {
            "1 message waits for the bot."
        } else {
            "Messages wait for the bot, in this order."
        };
        let rows = queue.messages.iter().enumerate().map(|(index, message)| {
            let id = message.id;
            let editing = self.editing == Some(id);
            let label = SharedString::from(message.text.clone());
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(
                    div()
                        .id(("queued", index))
                        .role(Role::ListItem)
                        .aria_label(label.clone())
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(editing, |text| text.text_color(theme.text_muted))
                        .child(label),
                )
                .child(
                    ui::button(("queue-edit", index), "Edit", ButtonKind::Ghost, theme).on_click(
                        cx.listener(move |this, _, window, cx| this.edit_queued(id, window, cx)),
                    ),
                )
                .child(
                    ui::button(
                        ("queue-send-now", index),
                        "Send now",
                        ButtonKind::Ghost,
                        theme,
                    )
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.queue_action(|conversation_id| ClientMessage::SendQueuedMessageNow {
                            conversation_id,
                            queue_id: id,
                        });
                    })),
                )
                .child(
                    ui::button(("queue-delete", index), "Delete", ButtonKind::Ghost, theme)
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.queue_action(|conversation_id| {
                                ClientMessage::DeleteQueuedMessage {
                                    conversation_id,
                                    queue_id: id,
                                }
                            });
                        })),
                )
        });
        Some(
            div()
                .id("queue")
                .role(Role::List)
                .aria_label("Queued messages")
                .p(space::S3)
                .rounded(radius::LG)
                .border_1()
                .border_color(theme.border_strong)
                .when(queue.paused, |panel| panel.bg(theme.warning_soft))
                .flex()
                .flex_col()
                .gap(space::S1_5)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(if queue.paused {
                                    theme.warning_text
                                } else {
                                    theme.text_muted
                                })
                                .child(heading),
                        )
                        .children(queue.paused.then(|| {
                            ui::button("queue-resume", "Resume", ButtonKind::Primary, theme)
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.queue_action(|conversation_id| {
                                        ClientMessage::ResumeQueue { conversation_id }
                                    });
                                }))
                        })),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    pub(super) fn render_composer(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let can_send = self.can_send(cx);
        let send_control = if self.can_stop(cx) {
            ui::button("stop-turn", "Stop", ButtonKind::Outline, theme)
                .on_click(cx.listener(|this, _, _, _| this.stop_turn()))
        } else {
            ui::button(
                "send-message",
                if self.editing.is_some() {
                    "Save"
                } else {
                    "Send"
                },
                if can_send {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Disabled
                },
                theme,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.send_message(window, cx);
            }))
        };
        let focused = self.composer.focus_handle(cx).is_focused(window);
        let activity = self.open.as_ref().and_then(|open| {
            if !open.transcript.turn_active() {
                return None;
            }
            let waiting = open.transcript.items().iter().any(|item| {
                matches!(
                    item,
                    Item::Approval {
                        state: ApprovalState::Waiting,
                        ..
                    }
                )
            });
            Some(if waiting {
                "The bot is waiting for your approval."
            } else {
                "The bot is working…"
            })
        });
        div()
            .w_full()
            .max_w(layout::COLUMN_WIDTH)
            .mx_auto()
            .px(space::S6)
            .pb(layout::COMPOSER_BOTTOM)
            .flex()
            .flex_col()
            .gap(space::S2)
            .children(self.render_queue(theme, cx))
            .child(
                div()
                    .id("activity")
                    .role(Role::Status)
                    .h(layout::STATUS_ROW)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .text_color(theme.text_muted)
                    .children(if self.editing.is_some() {
                        Some("Editing a queued message.")
                    } else {
                        activity
                    })
                    .children(self.editing.map(|_| {
                        ui::button("cancel-edit", "Cancel", ButtonKind::Ghost, theme).on_click(
                            cx.listener(|this, _, window, cx| this.cancel_edit(window, cx)),
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(space::S2)
                    .p(space::S1_5)
                    .rounded(radius::LG)
                    .bg(theme.surface)
                    .border_1()
                    .border_color(if focused {
                        theme.border_focus
                    } else {
                        theme.border_strong
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(space::S2)
                            .py(layout::COMPOSER_PADDING_Y)
                            .child(self.composer.clone()),
                    )
                    .child(send_control),
            )
    }
}
