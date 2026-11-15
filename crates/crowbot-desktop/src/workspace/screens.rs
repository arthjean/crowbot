use crowbot_protocol::PROTOCOL_VERSION;
use gpui::{
    AnyElement, Context, Div, FontWeight, Role, SharedString, Stateful, Window, WindowControlArea,
    div, prelude::*,
};

use super::{Link, Screen, Workspace};
use crate::assets::Icon;
use crate::theme::{Theme, icon as icon_size, layout, radius, space, text};
use crate::ui::{self, ButtonKind};

pub(super) fn update_advice(daemon_protocol: u32) -> &'static str {
    if daemon_protocol > PROTOCOL_VERSION {
        "Update this app: the daemon is newer."
    } else {
        "Update the daemon: it is older than this app."
    }
}

fn card(theme: &Theme) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .mr(layout::CARD_INSET)
        .mb(layout::CARD_INSET)
        .flex()
        .rounded(radius::CARD)
        .bg(theme.canvas)
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
}

impl Workspace {
    fn guard_focus(&mut self, live: bool, window: &mut Window, cx: &mut Context<Self>) {
        if live {
            if std::mem::take(&mut self.inert) {
                self.focus_composer(window, cx);
            }
            return;
        }
        self.inert = true;
        if !self.retry_focus.is_focused(window) && !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }
    }

    pub(super) fn render_shell(
        &mut self,
        root: Stateful<Div>,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let width = f32::from(window.viewport_size().width);
        let shown = self.sidebar.shown(width);
        // The panel takes its place beside the conversation while enough of
        // it stays, and covers it in a narrower window.
        let docked =
            width - shown.width - f32::from(layout::SIDE_PANEL) >= layout::CONVERSATION_MIN;
        let live = self.live();
        self.guard_focus(live, window, cx);
        let composing =
            self.history.is_none() && self.selected_bot.is_some() && self.load_error.is_none();
        let creating = self.creation.is_some();
        let pane = div()
            .id("conversation")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col();
        let pane = if creating {
            pane.child(self.render_creation(theme, window, cx))
        } else {
            pane.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_transcript(theme, cx)),
            )
            .children(composing.then(|| self.render_composer(theme, window, cx)))
        };
        let panes = div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .child(self.render_sidebar(shown, theme, cx))
            .child(pane)
            .child(self.render_resizer(shown, theme, cx))
            .children(self.render_bot_panel(docked, theme, window, cx))
            .children((!live).then(|| {
                div()
                    .id("inert")
                    .debug_selector(|| "inert".into())
                    .occlude()
                    .absolute()
                    .inset_0()
            }));
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(layout::TOP_ROW)
                    .flex()
                    .child(self.render_sidebar_topbar(shown, theme, cx))
                    .child(if creating {
                        self.render_creation_header(theme, cx)
                    } else {
                        self.render_header(theme, window, cx)
                    })
                    .children(self.render_caption_buttons(theme, window, cx)),
            )
            .child(
                card(theme)
                    .flex_col()
                    .children(self.render_link_notice(theme, cx))
                    .child(panes),
            );
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.render_rail(theme, cx))
                .child(main),
        )
    }

    fn render_link_notice(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let (detail, retry): (SharedString, bool) = match &self.link {
            Link::Connected | Link::Incompatible { .. } => return None,
            Link::Connecting => ("Connecting…".into(), false),
            Link::Starting => ("Starting the daemon…".into(), false),
            Link::Reconnecting(reason) if reason.is_empty() => ("Reconnecting…".into(), false),
            Link::Reconnecting(reason) => (format!("Reconnecting… ({reason})").into(), false),
            Link::Failed(reason) => (reason.clone(), true),
        };
        Some(
            div()
                .id("connection-notice")
                .debug_selector(|| "connection-notice".into())
                .role(Role::Status)
                .aria_label(SharedString::from(format!(
                    "The daemon is unreachable. {detail}"
                )))
                .flex_none()
                .p(space::S3)
                .flex()
                .items_center()
                .gap(space::S3)
                .border_b_1()
                .border_color(theme.border_strong)
                .child(ui::icon(
                    Icon::TriangleAlert,
                    icon_size::MD,
                    theme.warning_text,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::MEDIUM)
                                .child("The daemon is unreachable."),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text_muted)
                                .child(detail),
                        ),
                )
                .children(retry.then(|| {
                    ui::button("retry-connection", "Retry", ButtonKind::Outline, theme)
                        .track_focus(&self.retry_focus)
                        .on_click(cx.listener(|this, _, _, cx| this.retry_connection(cx)))
                })),
        )
    }

    pub(super) fn render_screen(
        &self,
        screen: &Screen,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let column = || {
            div()
                .w_full()
                .max_w(layout::NOTICE_WIDTH)
                .flex()
                .flex_col()
                .items_center()
                .gap(space::S3)
        };
        let heading = |title: &'static str| {
            div()
                .text_size(text::XL.size)
                .line_height(text::XL.leading)
                .font_weight(FontWeight::SEMIBOLD)
                .text_center()
                .child(title)
        };
        let retry = |label: &'static str, cx: &mut Context<Self>| {
            ui::button("retry-connection", label, ButtonKind::Outline, theme)
                .track_focus(&self.retry_focus)
                .on_click(cx.listener(|this, _, _, cx| this.retry_connection(cx)))
        };
        let body: AnyElement = match screen {
            Screen::Loading | Screen::Shell => {
                let status = match self.link {
                    Link::Starting => "Starting the daemon…",
                    Link::Connected => "Loading your bots…",
                    _ => "Connecting to the daemon…",
                };
                column()
                    .id("loading")
                    .debug_selector(|| "loading".into())
                    .role(Role::Status)
                    .aria_label(status)
                    .text_color(theme.text_muted)
                    .child(ui::spinner(
                        "loading-spinner",
                        icon_size::LG,
                        theme.text_muted,
                    ))
                    .child(status)
                    .into_any_element()
            }
            Screen::Unreachable(reason) => column()
                .id("unreachable")
                .debug_selector(|| "unreachable".into())
                .role(Role::Alert)
                .aria_label(SharedString::from(format!(
                    "The daemon is unreachable. {reason}"
                )))
                .child(ui::icon(
                    Icon::TriangleAlert,
                    icon_size::LG,
                    theme.warning_text,
                ))
                .child(heading("The daemon is unreachable"))
                .child(
                    div()
                        .text_center()
                        .text_color(theme.text_muted)
                        .child(reason.clone()),
                )
                .child(retry("Retry", cx))
                .into_any_element(),
            Screen::Incompatible {
                daemon_protocol,
                daemon_version,
            } => {
                let advice = update_advice(*daemon_protocol);
                let mut rows: Vec<(&'static str, SharedString)> = vec![
                    (
                        "This app speaks",
                        format!("protocol version {PROTOCOL_VERSION}").into(),
                    ),
                    (
                        "The daemon speaks",
                        format!("protocol version {daemon_protocol}").into(),
                    ),
                    ("App version", env!("CARGO_PKG_VERSION").into()),
                ];
                if let Some(version) = daemon_version {
                    rows.push(("Daemon version", version.clone()));
                }
                column()
                    .id("incompatible")
.debug_selector(|| "incompatible".into())
                    .role(Role::Alert)
                    .aria_label(SharedString::from(format!(
                        "This app speaks protocol version {PROTOCOL_VERSION} and the daemon speaks protocol version {daemon_protocol}. {advice}"
                    )))
                    .child(ui::icon(
                        Icon::TriangleAlert,
                        icon_size::LG,
                        theme.warning_text,
                    ))
                    .child(heading("Crow Bot and its daemon do not match"))
                    .child(
                        div()
                            .text_center()
                            .text_color(theme.text_muted)
                            .child(advice),
                    )
                    .child(
                        div()
                            .w_full()
                            .py(space::S2)
                            .flex()
                            .flex_col()
                            .children(rows.into_iter().map(|(label, value)| {
                                div()
                                    .py(space::S2)
                                    .flex()
                                    .justify_between()
                                    .gap(space::S4)
                                    .border_b_1()
                                    .border_color(theme.border_strong)
                                    .child(div().text_color(theme.text_muted).child(label))
                                    .child(div().child(value))
                            })),
                    )
                    .child(retry("Check again", cx))
                    .into_any_element()
            }
        };
        let loading = matches!(screen, Screen::Loading | Screen::Shell);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("screen-top")
                    .window_control_area(WindowControlArea::Drag)
                    .flex_none()
                    .h(layout::TOP_ROW)
                    .flex()
                    .child(div().flex_1())
                    .children(self.render_caption_buttons(theme, window, cx)),
            )
            .child(
                card(theme)
                    .ml(layout::CARD_INSET)
                    .items_center()
                    .justify_center()
                    .p(space::S6)
                    .when(loading, |card| {
                        card.window_control_area(WindowControlArea::Drag)
                    })
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mismatch_screen_names_the_side_to_update() {
        assert!(update_advice(PROTOCOL_VERSION + 1).starts_with("Update this app"));
        assert!(update_advice(PROTOCOL_VERSION - 1).starts_with("Update the daemon"));
    }
}
