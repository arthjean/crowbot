use std::time::{SystemTime, UNIX_EPOCH};

use crowbot_protocol::ConversationSummary;
use gpui::{AnyElement, Context, FontWeight, Role, SharedString, div, list, prelude::*};

use super::{OpenConversation, Workspace, row_id};
use crate::assets::Icon;
use crate::bots;
use crate::theme::{Theme, control, icon as icon_size, layout, radius, size, space, text};
use crate::ui::{self, ButtonKind};

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

    pub(super) fn render_head(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let line = || {
            div()
                .w_full()
                .py(space::S3)
                .flex()
                .items_center()
                .justify_center()
                .gap(space::S2)
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .text_color(theme.text_muted)
        };
        match &self.open {
            Some(OpenConversation {
                older_error: Some(reason),
                ..
            }) => line()
                .id("older-error")
                .debug_selector(|| "older-error".into())
                .role(Role::Alert)
                .aria_label(SharedString::from(format!(
                    "Older messages could not be loaded. {reason}"
                )))
                .child(ui::icon(
                    Icon::TriangleAlert,
                    icon_size::SM,
                    theme.danger_text,
                ))
                .child(SharedString::from(format!(
                    "Older messages could not be loaded. {reason}"
                )))
                .child(
                    ui::button("retry-older", "Retry", ButtonKind::Outline, theme).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.load_older();
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element(),
            Some(open) if open.loading_older => line()
                .id("older-loading")
                .debug_selector(|| "older-loading".into())
                .role(Role::Status)
                .aria_label("Loading older messages")
                .child(ui::spinner(
                    "older-spinner",
                    icon_size::SM,
                    theme.text_muted,
                ))
                .child("Loading older messages…")
                .into_any_element(),
            _ => div().h(space::S2).into_any_element(),
        }
    }

    fn render_empty(&self, theme: &Theme) -> AnyElement {
        let bot = self.selected_bot().map(|summary| &summary.bot);
        let name = bot.map(|bot| SharedString::from(bot.name.clone()));
        let title = bot
            .filter(|bot| !bot.title.is_empty())
            .map(|bot| SharedString::from(bot.title.clone()));
        div()
            .id("empty-conversation")
            .debug_selector(|| "empty-conversation".into())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(space::S2)
            .p(space::S6)
            .children(bot.map(|bot| {
                ui::avatar(bot, size::AVATAR_EMPTY, size::AVATAR_EMPTY_GLYPH, theme).mb(space::S2)
            }))
            .children(name.map(|name| {
                div()
                    .debug_selector(|| "empty-name".into())
                    .text_size(text::XL.size)
                    .line_height(text::XL.leading)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name)
            }))
            .children(title.map(|title| {
                div()
                    .debug_selector(|| "empty-title".into())
                    .max_w(layout::NOTICE_WIDTH)
                    .text_center()
                    .text_color(theme.text_secondary)
                    .child(title)
            }))
            .child(
                div()
                    .text_color(theme.text_muted)
                    .child("Send a message to start this conversation."),
            )
            .into_any_element()
    }

    fn render_jump(
        &self,
        open: &OpenConversation,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !open.away_from_end() {
            return None;
        }
        let new = open.new_messages;
        let label = match new {
            0 => "Jump to latest".to_owned(),
            1 => "Jump to latest, 1 new message".to_owned(),
            _ => format!("Jump to latest, {new} new messages"),
        };
        let (hover, focus) = (theme.control_hover, theme.border_focus);
        Some(
            div()
                .absolute()
                .bottom(layout::JUMP_BOTTOM)
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("jump-to-latest")
                        .debug_selector(|| "jump-to-latest".into())
                        .occlude()
                        .focusable()
                        .tab_stop(true)
                        .role(Role::Button)
                        .aria_label(SharedString::from(label.clone()))
                        .tooltip(ui::tooltip(label))
                        .h(size::JUMP)
                        .min_w(size::JUMP)
                        .px(space::S3)
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(space::S1_5)
                        .rounded_full()
                        .bg(theme.sheet)
                        .border_1()
                        .border_color(theme.border_strong)
                        .shadow(theme.menu_shadow())
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .focus_visible(move |style| style.border_color(focus))
                        .on_click(cx.listener(|this, _, _, cx| this.jump_to_latest(cx)))
                        .children((new > 0).then(|| {
                            div()
                                .debug_selector(|| "jump-count".into())
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(format!("{new} new")))
                        }))
                        .child(ui::icon(Icon::ArrowDown, icon_size::MD, theme.text)),
                )
                .into_any_element(),
        )
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
            Some(open)
                if open.transcript.rows().is_empty() && open.transcript.older().is_none() =>
            {
                self.render_empty(theme)
            }
            Some(open) => div()
                .debug_selector(|| "transcript".into())
                .relative()
                .size_full()
                .child(
                    list(
                        open.list.clone(),
                        cx.processor(|this, index, window, cx| this.render_item(index, window, cx)),
                    )
                    .size_full(),
                )
                .children(self.render_jump(open, theme, cx))
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
}
