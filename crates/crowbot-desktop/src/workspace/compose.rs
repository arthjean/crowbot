use crowbot_protocol::ClientMessage;
use gpui::{AnyElement, Context, Div, Focusable, Role, SharedString, Window, div, prelude::*};

use super::{Workspace, forms};
use crate::assets::Icon;
use crate::theme::{Theme, icon as icon_size, layout, radius, size, space, text};
use crate::ui::{self, ButtonKind};

impl Workspace {
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
                .debug_selector(move || format!("queued-{index}"))
                .flex_none()
                .h(layout::QUEUE_ROW)
                .flex()
                .items_center()
                .gap(space::S1)
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
                    ui::small_icon_button(("queue-edit", index), Icon::Pencil, "Edit", theme)
                        .debug_selector(move || format!("queue-edit-{index}"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_queued(id, window, cx);
                        })),
                )
                .child(
                    ui::small_icon_button(
                        ("queue-send-now", index),
                        Icon::ArrowUp,
                        "Send now",
                        theme,
                    )
                    .debug_selector(move || format!("queue-send-now-{index}"))
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.queue_action(|conversation_id| ClientMessage::SendQueuedMessageNow {
                            conversation_id,
                            queue_id: id,
                        });
                    })),
                )
                .child(
                    ui::small_icon_button(("queue-delete", index), Icon::Trash, "Delete", theme)
                        .debug_selector(move || format!("queue-delete-{index}"))
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
                .debug_selector(|| "queue".into())
                .role(Role::List)
                .aria_label("Queued messages")
                .px(space::S3)
                .py(space::S2)
                .rounded(radius::LG)
                .border_1()
                .border_color(theme.border_strong)
                .when(queue.paused, |panel| panel.bg(theme.warning_soft))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(text::SM.size)
                                .line_height(text::SM.leading)
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
                .child(
                    div()
                        .id("queue-rows")
                        .max_h(layout::QUEUE_MAX_HEIGHT)
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    fn render_send_error(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reason = self.send_error.clone()?;
        Some(
            div()
                .id("send-error")
                .debug_selector(|| "send-error".into())
                .role(Role::Alert)
                .aria_label(reason.clone())
                .pl(space::S3)
                .pr(space::S1)
                .py(space::S1)
                .flex()
                .items_center()
                .gap(space::S2)
                .rounded(radius::CARD)
                .bg(theme.danger_soft)
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .text_color(theme.danger_text)
                .child(ui::icon(
                    Icon::CircleAlert,
                    icon_size::SM,
                    theme.danger_text,
                ))
                .child(div().flex_1().min_w_0().child(reason))
                .child(
                    ui::small_icon_button("dismiss-send-error", Icon::X, "Dismiss", theme)
                        .debug_selector(|| "dismiss-send-error".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send_error = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_send_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let focus = theme.border_focus;
        let round = |name: &'static str, label: &'static str| {
            div()
                .id(name)
                .debug_selector(move || name.to_owned())
                .focusable()
                .role(Role::Button)
                .aria_label(label)
                .flex_none()
                .size(size::SEND)
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(gpui::transparent_black())
                .focus_visible(move |style| style.border_color(focus))
        };
        let hover = theme.text_secondary;
        if self.can_stop(cx) {
            return round("stop-turn", "Stop the bot")
                .tab_stop(true)
                .tooltip(ui::tooltip("Stop"))
                .bg(theme.inverse_surface)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(|this, _, _, _| this.stop_turn()))
                .child(ui::icon(Icon::Square, icon_size::MD, theme.inverse_text))
                .into_any_element();
        }
        let label = if self.editing.is_some() {
            "Save the queued message"
        } else {
            "Send"
        };
        if !self.can_send(cx) {
            return round("send-message", label)
                .tab_stop(false)
                .aria_description("Unavailable")
                .bg(theme.hover)
                .child(ui::icon(Icon::ArrowUp, icon_size::MD, theme.text_muted))
                .into_any_element();
        }
        round("send-message", label)
            .tab_stop(true)
            .tooltip(ui::tooltip(label))
            .bg(theme.inverse_surface)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(|this, _, window, cx| this.send_message(window, cx)))
            .child(ui::icon(Icon::ArrowUp, icon_size::MD, theme.inverse_text))
            .into_any_element()
    }

    pub(super) fn render_composer(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let focused = self.composer.focus_handle(cx).is_focused(window);
        let activity = self.open.as_ref().and_then(|open| {
            if !open.live().turn_active() {
                return None;
            }
            // A runtime that downloads before the turn starts is told here.
            let download = self.selected_bot().and_then(|summary| {
                let status = self
                    .providers
                    .iter()
                    .find(|status| status.id == summary.bot.provider)?;
                Some(forms::download_text(&status.name, status.install?))
            });
            Some(if open.live().has_waiting_approval() {
                (
                    SharedString::from("The bot is waiting for your approval."),
                    "activity",
                )
            } else if let Some(download) = download {
                (download, "activity-download")
            } else {
                (SharedString::from("The bot is working…"), "activity")
            })
        });
        let (activity, activity_selector) = match activity {
            Some((text, selector)) => (Some(text), selector),
            None => (None, "activity"),
        };
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
            .children(self.render_send_error(theme, cx))
            .child(
                div()
                    .id("activity")
                    .debug_selector(move || activity_selector.into())
                    .role(Role::Status)
                    .h(layout::STATUS_ROW)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .text_color(theme.text_muted)
                    .children(if self.editing.is_some() {
                        Some(SharedString::from("Editing a queued message."))
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
                    .rounded(radius::XL)
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
                    .child(self.render_send_control(theme, cx)),
            )
    }
}
