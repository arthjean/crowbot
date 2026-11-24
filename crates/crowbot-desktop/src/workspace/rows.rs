use std::rc::Rc;
use std::time::Instant;

use chrono::{DateTime, Datelike, Local, TimeZone};
use crowbot_protocol::{ApprovalDecision, ApprovalReason, FailureCause, ProviderState, TurnOrigin};
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Div, FontWeight, Hsla, Role, SharedString,
    Stateful, Window, div, prelude::*,
};

use super::{Copied, HEAD, OpenConversation, RowText, Workspace};
use crate::assets::Icon;
use crate::markdown::{self, Actions, Block, Document};
use crate::reveal::Reveal;
use crate::theme::{
    Theme, duration, font, icon as icon_size, layout, opacity, radius, size, space, text,
};
use crate::transcript::{Approval, ApprovalState, Notice, Row, RowKind, Status, local_day};
use crate::ui::{self, ButtonKind};

const MESSAGE_GROUP: &str = "message";

fn prepare(
    open: &mut OpenConversation,
    index: usize,
    now: Instant,
    window: &mut Window,
) -> Option<Vec<Rc<Block>>> {
    let row = open.transcript.rows().get(index)?;
    match &row.kind {
        RowKind::Answer { text, streaming } => {
            let shown = open.texts.entry(row.key).or_insert_with(|| RowText {
                document: Document::default(),
                reveal: Reveal::shown_from(text.len(), now),
            });
            let length = shown.reveal.advance(text, now);
            shown
                .document
                .set(&text[..length], *streaming || length < text.len());
            if !shown.reveal.done() {
                window.request_animation_frame();
            }
            Some(shown.document.blocks())
        }
        RowKind::User { text, .. } | RowKind::Thought { text } => {
            let shown = open.texts.entry(row.key).or_insert_with(|| RowText {
                document: Document::plain(text),
                reveal: Reveal::shown_from(text.len(), now),
            });
            Some(shown.document.blocks())
        }
        RowKind::Steps { live, .. } => {
            let list = open.step_lists.entry(row.key).or_default();
            if *live {
                list.scroll_to_bottom();
            }
            None
        }
        _ => None,
    }
}

pub(super) fn day_label(at_ms: i64, now: DateTime<Local>) -> String {
    let Some(day) = local_day(at_ms) else {
        return String::new();
    };
    let today = now.date_naive();
    if day == today {
        "Today".to_owned()
    } else if Some(day) == today.pred_opt() {
        "Yesterday".to_owned()
    } else if day.year() == today.year() {
        day.format("%a, %b %-d").to_string()
    } else {
        day.format("%b %-d, %Y").to_string()
    }
}

fn time_label(at_ms: i64) -> SharedString {
    Local
        .timestamp_millis_opt(at_ms)
        .single()
        .map(|time| time.format("%-I:%M %p").to_string())
        .unwrap_or_default()
        .into()
}

pub(super) fn provider_name(provider: Option<&str>) -> &str {
    match provider {
        Some("claude") => "Claude",
        Some("codex") => "ChatGPT",
        Some("grok") => "Grok",
        Some(other) if !other.is_empty() => other,
        _ => "the provider",
    }
}

/// Who started a turn, for the turns a person did not start.
pub(super) fn origin_label(origin: TurnOrigin) -> Option<&'static str> {
    match origin {
        TurnOrigin::User => None,
        TurnOrigin::Retry => Some("Sent again by Crow Bot after a failure"),
        TurnOrigin::Routine => Some("Started by a routine"),
        TurnOrigin::Review => Some("Started by a review"),
        TurnOrigin::Goal => Some("Started for a goal"),
        TurnOrigin::Delegation => Some("Started by a delegation"),
        TurnOrigin::Budget => Some("Started by a budget check"),
    }
}

pub(super) fn status_text(status: &Status) -> (&'static str, String) {
    match status {
        Status::Stopped => ("Stopped", String::new()),
        Status::DaemonRestarted => (
            "Interrupted",
            "The daemon restarted before the bot finished.".to_owned(),
        ),
        Status::Unresponsive => (
            "Interrupted",
            "The agent stopped answering, so the turn was ended.".to_owned(),
        ),
        Status::NoProgress => (
            "Stopped",
            "The bot repeated the same steps with the same results, so Crow Bot stopped the turn."
                .to_owned(),
        ),
        Status::Inactive => (
            "Stopped",
            "The agent sent nothing for too long, so Crow Bot stopped the turn.".to_owned(),
        ),
        Status::ToolCallLimit => (
            "Stopped",
            "The turn reached the bot's limit of tool calls.".to_owned(),
        ),
        Status::LengthLimit => (
            "Stopped",
            "The answer reached the length limit of the model.".to_owned(),
        ),
        Status::RequestLimit => (
            "Stopped",
            "The turn reached the limit of requests to the model.".to_owned(),
        ),
        Status::Refused => ("Declined", "The model declined to answer.".to_owned()),
        Status::Unfinished => (
            "Unfinished",
            "The turn ended before the bot finished.".to_owned(),
        ),
        Status::Failed {
            message,
            cause,
            provider,
        } => {
            let name = provider_name(provider.as_deref());
            match cause {
                Some(FailureCause::SignInRequired) => (
                    "Sign-in needed",
                    format!("Sign in to {name}, then send your message again."),
                ),
                Some(FailureCause::UsageLimit) => (
                    "Usage limit reached",
                    format!(
                        "Your {name} plan reached its usage limit. Wait until the limit resets or raise it at {name}, then send your message again."
                    ),
                ),
                _ => ("Error", message.clone()),
            }
        }
    }
}

/// What a notice of the daemon says, with the names of the providers.
pub(super) fn notice_text(notice: &Notice) -> String {
    match notice {
        Notice::ModelUnavailable { model, provider } => {
            let name = match provider.as_deref() {
                None | Some("custom") => "The agent",
                known => provider_name(known),
            };
            format!(
                "{name} no longer offers the model {model}. The bot now runs on the default model."
            )
        }
        Notice::ProviderChanged { from, to } => {
            let (from, to) = (provider_name(Some(from)), provider_name(Some(to)));
            format!("The bot moved from {from} to {to}. {to} received what was said before.")
        }
        Notice::ToolRefused { tool, message } => {
            format!("Crow Bot did not run the tool {tool}: {message}")
        }
    }
}

fn column(key: i64, top: gpui::Pixels, last: bool, body: impl IntoElement) -> AnyElement {
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(
            div()
                .id(("row", key.cast_unsigned()))
                .w_full()
                .max_w(layout::COLUMN_WIDTH)
                .px(space::S6)
                .pt(top)
                .when(last, |column| column.pb(space::S6))
                .child(body),
        )
        .into_any_element()
}

impl Workspace {
    pub(super) fn render_item(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx);
        let Some(index) = index.checked_sub(HEAD) else {
            return self.render_head(&theme, cx);
        };
        // The last row of a part of the history comes on screen: the events
        // that follow it are asked for.
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.newer.is_some() && index + 1 == open.transcript.rows().len())
        {
            self.load_newer();
        }
        let now = (self.clock)();
        let blocks = self
            .open
            .as_mut()
            .and_then(|open| prepare(open, index, now, window));
        let row = self.render_message_row(index, blocks.unwrap_or_default(), &theme, cx);
        let flashed = self.open.as_mut().and_then(|open| {
            let key = open.transcript.rows().get(index)?.key;
            let (flashed, started) = open.flash?;
            if now.duration_since(started) >= duration::FLASH {
                open.flash = None;
            }
            (flashed == key && open.flash.is_some()).then_some(key)
        });
        match flashed {
            Some(key) => {
                let wash = Hsla::from(theme.accent_soft);
                div()
                    .debug_selector(|| "revealed-message".into())
                    .w_full()
                    .child(row)
                    .with_animation(
                        ("revealed", key.cast_unsigned()),
                        Animation::new(duration::FLASH),
                        move |row, elapsed| row.bg(wash.opacity(opacity::SHOWN - elapsed)),
                    )
                    .into_any_element()
            }
            None => row,
        }
    }

    fn text_actions(&self, key: i64, cx: &mut Context<Self>) -> Actions {
        let (links, codes) = (cx.weak_entity(), cx.weak_entity());
        Actions {
            open_link: Rc::new(move |target, _, cx| {
                links.update(cx, |this, cx| this.open_link(target, cx)).ok();
            }),
            copy_code: Rc::new(move |index, code, _, cx| {
                codes
                    .update(cx, |this, cx| {
                        this.copy(Copied::Code(key, index), code.to_owned(), cx);
                    })
                    .ok();
            }),
            copied_code: match self.copied {
                Some(Copied::Code(row, index)) if row == key => Some(index),
                _ => None,
            },
        }
    }

    fn message_text(
        &self,
        key: i64,
        blocks: &[Rc<Block>],
        color: impl Into<Hsla>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let actions = self.text_actions(key, cx);
        div()
            .min_w_0()
            .text_size(text::LG.size)
            .line_height(text::LG.leading)
            .child(markdown::message(
                key,
                blocks,
                color.into(),
                &self.selection,
                &actions,
                theme,
            ))
    }

    fn message_actions(&self, row: &Row, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let key = row.key;
        let copied = self.copied == Some(Copied::Message(key));
        div()
            .flex_none()
            .h(layout::ACTION_ROW)
            .flex()
            .items_center()
            .gap(space::S1_5)
            .opacity(if copied {
                opacity::SHOWN
            } else {
                opacity::HIDDEN
            })
            .group_hover(MESSAGE_GROUP, |style| style.opacity(opacity::SHOWN))
            .child(
                div()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_muted)
                    .child(time_label(row.at_ms)),
            )
            .child(
                ui::small_icon_button(
                    "copy-message",
                    if copied { Icon::Check } else { Icon::Copy },
                    if copied { "Copied" } else { "Copy message" },
                    theme,
                )
                .debug_selector(move || format!("copy-message-{key}"))
                .on_click(cx.listener(move |this, _, _, cx| this.copy_message(key, cx))),
            )
    }

    fn render_message_row(
        &self,
        index: usize,
        blocks: Vec<Rc<Block>>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(open) = self.open.as_ref() else {
            return div().into_any_element();
        };
        let rows = open.transcript.rows();
        let Some(row) = rows.get(index) else {
            return div().into_any_element();
        };
        let key = row.key;
        let last = index + 1 == rows.len();
        let starts = open.transcript.starts_group(index);
        let top = if starts {
            layout::ROW_GAP
        } else {
            layout::ROW_GAP_GROUPED
        };
        let body = match &row.kind {
            RowKind::Day => {
                let label = SharedString::from(day_label(row.at_ms, Local::now()));
                let day = div()
                    .debug_selector(move || format!("day-{key}"))
                    .h(layout::DAY_ROW)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_muted)
                    .child(label);
                return column(key, layout::ROW_GAP, last, day);
            }
            RowKind::Notice(notice) => {
                let text = SharedString::from(notice_text(notice));
                let note = div()
                    .id("notice")
                    .debug_selector(move || format!("notice-{key}"))
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .w_full()
                    .px(space::S6)
                    .py(space::S1)
                    .text_center()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_muted)
                    .child(text);
                return column(key, layout::ROW_GAP, last, note);
            }
            RowKind::User { origin, .. } => {
                let bubble = self
                    .message_text(key, &blocks, theme.text, theme, cx)
                    .debug_selector(move || format!("bubble-{key}"))
                    .max_w(layout::BUBBLE_MAX_WIDTH)
                    .px(layout::BUBBLE_PADDING_X)
                    .py(layout::BUBBLE_PADDING_Y)
                    .rounded(radius::BUBBLE)
                    .bg(theme.surface_raised);
                let line = div()
                    .group(MESSAGE_GROUP)
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(space::S2)
                    .child(self.message_actions(row, theme, cx))
                    .child(bubble);
                // A turn the daemon started says so above its message.
                let Some(label) = origin_label(*origin) else {
                    return column(key, top, last, line);
                };
                let started = div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(space::S1)
                    .child(
                        div()
                            .debug_selector(move || format!("origin-{key}"))
                            .text_size(text::SM.size)
                            .line_height(text::SM.leading)
                            .text_color(theme.text_muted)
                            .child(label),
                    )
                    .child(line);
                return column(key, top, last, started);
            }
            RowKind::Answer { .. } => {
                let ghost = blocks
                    .iter()
                    .any(|block| matches!(**block, Block::Code { .. } | Block::Table { .. }));
                let bubble = self
                    .message_text(key, &blocks, theme.text, theme, cx)
                    .debug_selector(move || format!("bubble-{key}"))
                    .max_w_full()
                    .when(ghost, |bubble| bubble.w_full())
                    .when(!ghost && !blocks.is_empty(), |bubble| {
                        bubble
                            .px(layout::BUBBLE_PADDING_X)
                            .py(layout::BUBBLE_PADDING_Y)
                            .rounded(radius::BUBBLE)
                            .bg(theme.surface)
                    });
                div()
                    .group(MESSAGE_GROUP)
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(bubble)
                    .child(self.message_actions(row, theme, cx))
                    .into_any_element()
            }
            RowKind::Thought { .. } => div()
                .debug_selector(move || format!("thought-{key}"))
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .text_size(text::SM.size)
                        .line_height(text::SM.leading)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(ui::icon(Icon::Lightbulb, icon_size::XS, theme.text_muted))
                        .child("Thinking"),
                )
                .child(
                    self.message_text(key, &blocks, theme.text_secondary, theme, cx)
                        .border_l(size::QUOTE_BAR)
                        .border_color(theme.border_focus)
                        .pl(space::S3),
                )
                .into_any_element(),
            RowKind::Steps { .. } => self.render_steps(open, row, theme, cx),
            RowKind::Approval(approval) => self.render_approval(open, key, approval, theme, cx),
            RowKind::Status(status) => {
                let controls = self.status_controls(status, last, theme, cx);
                render_status(key, status, controls, theme)
            }
        };
        let bot = self.selected_bot().map(|summary| &summary.bot);
        let avatar = bot.filter(|_| starts).map(|bot| {
            ui::avatar(bot, size::AVATAR_MESSAGE, theme)
                .debug_selector(move || format!("row-avatar-{key}"))
        });
        let name = bot.filter(|_| starts).map(|bot| {
            div()
                .debug_selector(move || format!("row-name-{key}"))
                .flex()
                .items_baseline()
                .gap(space::S2)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(bot.name.clone())),
                )
                .child(
                    div()
                        .text_size(text::SM.size)
                        .line_height(text::SM.leading)
                        .text_color(theme.text_muted)
                        .child(time_label(row.at_ms)),
                )
        });
        let line = div()
            .debug_selector(move || format!("row-{key}"))
            .flex()
            .gap(space::S2)
            .child(div().flex_none().w(layout::AVATAR_GUTTER).children(avatar))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .children(name)
                    .child(body),
            );
        column(key, top, last, line)
    }

    fn render_approval(
        &self,
        open: &OpenConversation,
        key: i64,
        approval: &Approval,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = SharedString::from(
            approval
                .title
                .clone()
                .unwrap_or_else(|| "Use a tool".to_owned()),
        );
        let waiting = approval.state == ApprovalState::Waiting;
        let outcome = approval_outcome(approval.state, approval.automatic).map(|(label, tone)| {
            let color = match tone {
                Tone::Allowed => theme.success_text,
                Tone::Denied => theme.danger_text,
                Tone::Closed => theme.text_muted,
            };
            (label, color)
        });
        let target = approval
            .target
            .as_ref()
            .map(|target| SharedString::from(target.clone()));
        let folder = approval
            .folder
            .as_ref()
            .map(|folder| SharedString::from(folder.clone()));
        let details = (target.is_some() || folder.is_some()).then(|| {
            div()
                .id("approval-target")
                .debug_selector(move || format!("approval-target-{key}"))
                .max_h(layout::APPROVAL_TARGET_MAX_HEIGHT)
                .overflow_y_scroll()
                .p(space::S3)
                .rounded(radius::MD)
                .bg(theme.canvas)
                .border_1()
                .border_color(theme.border_strong)
                .font(font::mono())
                .text_size(text::CODE.size)
                .line_height(text::CODE.leading)
                .flex()
                .flex_col()
                .gap(space::S1)
                .children(folder.map(|folder| {
                    div()
                        .debug_selector(move || format!("approval-folder-{key}"))
                        .text_color(theme.text_muted)
                        .child(folder)
                }))
                .children(target.map(|target| div().child(target)))
        });
        let buttons = approval.id.clone().filter(|_| waiting).map(|id| {
            let sending = open.answering.contains(&id);
            let kind = |kind| if sending { ButtonKind::Disabled } else { kind };
            let button = |name: &'static str, label: &'static str, kind| {
                ui::button(name, label, kind, theme).debug_selector(move || format!("{name}-{key}"))
            };
            let deny = button("deny", "Deny", kind(ButtonKind::Outline));
            let always = button("always-allow", "Always allow", kind(ButtonKind::Outline));
            let allow = button("allow", "Allow", kind(ButtonKind::Primary));
            let (deny_id, always_id, allow_id) = (id.clone(), id.clone(), id);
            div()
                .flex()
                .justify_end()
                .gap(space::S2)
                .child(deny.when(!sending, |deny| {
                    deny.on_click(cx.listener(move |this, _, window, cx| {
                        this.resolve_approval(deny_id.clone(), ApprovalDecision::Declined);
                        this.focus_composer(window, cx);
                        cx.notify();
                    }))
                }))
                .children((!approval.cut).then(|| {
                    always.when(!sending, |always| {
                        always.on_click(cx.listener(move |this, _, window, cx| {
                            this.ask_full_access(always_id.clone(), window, cx);
                        }))
                    })
                }))
                .children((!approval.cut).then(|| {
                    allow.when(!sending, |allow| {
                        allow.on_click(cx.listener(move |this, _, window, cx| {
                            this.resolve_approval(allow_id.clone(), ApprovalDecision::Allowed);
                            this.focus_composer(window, cx);
                            cx.notify();
                        }))
                    })
                }))
        });
        let heading = if waiting {
            "The bot asks for your approval"
        } else {
            "Approval request"
        };
        let card: Stateful<Div> = div()
            .id("approval")
            .debug_selector(move || format!("approval-{key}"))
            .role(Role::Group)
            .aria_label(SharedString::from(format!("{heading}: {title}")))
            .w_full()
            .max_w(layout::APPROVAL_WIDTH)
            .px(space::S4)
            .pt(space::S4)
            .pb(space::S3)
            .rounded(radius::CARD)
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border_strong)
            .flex()
            .flex_col()
            .gap(space::S3);
        card.child(
            div()
                .flex()
                .flex_col()
                .gap(space::S0_5)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .text_size(text::SM.size)
                        .line_height(text::SM.leading)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if waiting {
                            theme.warning_text
                        } else {
                            theme.text_muted
                        })
                        .child(ui::icon(
                            Icon::ShieldCheck,
                            icon_size::SM,
                            if waiting {
                                theme.warning_text
                            } else {
                                theme.text_muted
                            },
                        ))
                        .child(heading),
                )
                .child(
                    div()
                        .debug_selector(move || format!("approval-title-{key}"))
                        .text_size(text::LG.size)
                        .line_height(text::LG.leading)
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                ),
        )
        .children(details)
        .children((waiting && approval.cut).then(|| {
            div()
                .debug_selector(move || format!("approval-cut-{key}"))
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .text_color(theme.warning_text)
                .child("This request is too long to show in full, so it can only be denied.")
        }))
        .children(outcome.map(|(label, color)| {
            div()
                .debug_selector(move || format!("approval-outcome-{key}"))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(label)
        }))
        .children(buttons)
        .into_any_element()
    }
}

impl Workspace {
    /// The sign-in of the provider, under the last turn that failed for the
    /// lack of one, until the provider is signed in.
    fn status_controls(
        &self,
        status: &Status,
        last: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Status::Failed {
            cause: Some(FailureCause::SignInRequired),
            provider: Some(provider),
            ..
        } = status
        else {
            return Vec::new();
        };
        match self.providers.iter().find(|known| known.id == *provider) {
            Some(known) if last && known.state != ProviderState::Ready => {
                self.provider_controls(known, "turn", theme, cx)
            }
            _ => Vec::new(),
        }
    }
}

fn render_status(
    key: i64,
    status: &Status,
    controls: Vec<AnyElement>,
    theme: &Theme,
) -> AnyElement {
    let (label, detail) = status_text(status);
    let failed = matches!(status, Status::Failed { .. });
    let provider_message = match status {
        Status::Failed {
            message,
            cause: Some(FailureCause::SignInRequired | FailureCause::UsageLimit),
            ..
        } if !message.is_empty() => Some(SharedString::from(message.clone())),
        _ => None,
    };
    let (glyph, color) = match status {
        Status::Failed { .. } => (Icon::CircleAlert, theme.danger_text),
        Status::Stopped => (Icon::Square, theme.text_muted),
        _ => (Icon::TriangleAlert, theme.warning_text),
    };
    let spoken = if detail.is_empty() {
        label.to_owned()
    } else {
        format!("{label}: {detail}")
    };
    div()
        .id("status")
        .debug_selector(move || format!("status-{key}"))
        .role(if failed { Role::Alert } else { Role::Status })
        .aria_label(SharedString::from(spoken))
        .flex()
        .flex_col()
        .gap(space::S1)
        .when(failed, |row| {
            row.p(space::S3)
                .rounded(radius::LG)
                .bg(theme.danger_soft)
                .max_w(layout::BUBBLE_MAX_WIDTH)
        })
        .child(
            div()
                .debug_selector(move || format!("status-label-{key}"))
                .flex()
                .items_center()
                .gap(space::S1_5)
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(ui::icon(glyph, icon_size::XS, color))
                .child(label),
        )
        .children((!detail.is_empty()).then(|| {
            div()
                .when(!failed, |detail| detail.text_color(theme.text_muted))
                .child(SharedString::from(detail))
        }))
        .children(provider_message.map(|message| {
            div()
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .text_color(theme.text_secondary)
                .child(message)
        }))
        .children((!controls.is_empty()).then(|| {
            div()
                .debug_selector(move || format!("status-controls-{key}"))
                .pt(space::S1)
                .flex()
                .flex_wrap()
                .items_center()
                .gap(space::S2)
                .children(controls)
        }))
        .into_any_element()
}

/// How an outcome of an approval reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Allowed,
    Denied,
    Closed,
}

/// The line that says how an approval ended, and who ended it when the
/// daemon did.
fn approval_outcome(
    state: ApprovalState,
    automatic: Option<ApprovalReason>,
) -> Option<(&'static str, Tone)> {
    Some(match (state, automatic) {
        (ApprovalState::Waiting, _) => return None,
        (ApprovalState::Allowed, Some(ApprovalReason::PermissionLevel)) => {
            ("Allowed by Full access", Tone::Allowed)
        }
        (ApprovalState::Allowed, Some(ApprovalReason::DaemonTool)) => {
            ("Allowed: a Crow Bot tool", Tone::Allowed)
        }
        (ApprovalState::Allowed, Some(ApprovalReason::Rule { .. })) => {
            ("Allowed by a rule of this bot", Tone::Allowed)
        }
        (ApprovalState::Allowed, _) => ("Allowed", Tone::Allowed),
        (ApprovalState::Declined, Some(ApprovalReason::NoAllowOnce)) => {
            ("Denied: it could not be allowed once", Tone::Denied)
        }
        (ApprovalState::Declined, Some(ApprovalReason::Floor)) => {
            ("Denied: Crow Bot never allows this", Tone::Denied)
        }
        (ApprovalState::Declined, Some(ApprovalReason::Rule { .. })) => {
            ("Denied by a rule of this bot", Tone::Denied)
        }
        (ApprovalState::Declined, Some(ApprovalReason::NotRecorded)) => {
            ("Denied: the answer could not be saved", Tone::Denied)
        }
        (ApprovalState::Declined, _) => ("Denied", Tone::Denied),
        (ApprovalState::Cancelled, _) => ("Cancelled", Tone::Closed),
        (ApprovalState::Expired, _) => ("Not answered", Tone::Closed),
    })
}

#[cfg(test)]
mod tests {

    use chrono::Duration;

    use super::*;

    #[test]
    fn a_day_reads_as_today_yesterday_or_its_date() {
        let now = Local
            .with_ymd_and_hms(2026, 11, 14, 15, 0, 0)
            .single()
            .unwrap();
        let at = |days: i64| (now - Duration::days(days)).timestamp_millis();
        assert_eq!(day_label(at(0), now), "Today");
        assert_eq!(day_label(at(1), now), "Yesterday");
        assert_eq!(day_label(at(3), now), "Wed, Nov 11");
        assert_eq!(day_label(at(400), now), "Oct 10, 2025");
    }

    #[test]
    fn a_provider_error_names_the_action_and_the_provider() {
        let failed = |cause, provider: &str| Status::Failed {
            message: "401".to_owned(),
            cause,
            provider: Some(provider.to_owned()),
        };
        let (label, detail) = status_text(&failed(Some(FailureCause::SignInRequired), "claude"));
        assert_eq!(label, "Sign-in needed");
        assert!(detail.starts_with("Sign in to Claude"), "{detail}");
        let (label, detail) = status_text(&failed(Some(FailureCause::UsageLimit), "codex"));
        assert_eq!(label, "Usage limit reached");
        assert!(detail.contains("ChatGPT"), "{detail}");
        assert_eq!(
            notice_text(&Notice::ProviderChanged {
                from: "claude".to_owned(),
                to: "codex".to_owned(),
            }),
            "The bot moved from Claude to ChatGPT. ChatGPT received what was said before."
        );
        assert_eq!(
            notice_text(&Notice::ModelUnavailable {
                model: "opus".to_owned(),
                provider: Some("claude".to_owned()),
            }),
            "Claude no longer offers the model opus. The bot now runs on the default model."
        );
        assert_eq!(
            status_text(&failed(None, "claude")),
            ("Error", "401".to_owned())
        );
        assert_eq!(status_text(&Status::Stopped).0, "Stopped");
        assert!(
            status_text(&Status::DaemonRestarted)
                .1
                .contains("daemon restarted")
        );
    }

    #[test]
    fn an_answer_of_a_rule_a_floor_or_a_failed_save_names_who_gave_it() {
        let outcome = |state, reason| approval_outcome(state, Some(reason));
        assert_eq!(
            outcome(ApprovalState::Declined, ApprovalReason::Floor),
            Some(("Denied: Crow Bot never allows this", Tone::Denied))
        );
        assert_eq!(
            outcome(ApprovalState::Declined, ApprovalReason::Rule { id: 4 }),
            Some(("Denied by a rule of this bot", Tone::Denied))
        );
        assert_eq!(
            outcome(ApprovalState::Allowed, ApprovalReason::Rule { id: 4 }),
            Some(("Allowed by a rule of this bot", Tone::Allowed))
        );
        assert_eq!(
            outcome(ApprovalState::Declined, ApprovalReason::NotRecorded),
            Some(("Denied: the answer could not be saved", Tone::Denied))
        );
        assert_eq!(approval_outcome(ApprovalState::Waiting, None), None);
    }
}
