use gpui::{
    Context, Div, FontWeight, Pixels, Role, SharedString, Stateful, TextRun, Window,
    WindowControlArea, canvas, div, prelude::*,
};

use super::{Workspace, model_label};
use crate::assets::Icon;
use crate::chrome::{Side, WindowDrag};
use crate::theme::{Theme, control, font, icon as icon_size, layout, radius, size, space, text};
use crate::ui;

fn text_width(window: &Window, content: &SharedString) -> Pixels {
    let run = TextRun {
        len: content.len(),
        font: font::sans(),
        color: gpui::transparent_black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(content.clone(), text::MD.size, &[run], None)
        .width()
}

fn title_is_hidden(slot: Pixels, natural: Pixels) -> bool {
    slot < layout::HEADER_TITLE_MIN && slot.ceil() < natural.floor()
}

impl Workspace {
    pub(super) fn render_header(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let header = div()
            .id("conversation-header")
            .debug_selector(|| "conversation-header".into())
            .window_drag()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .gap(space::S2)
            .pl(layout::HEADER_PADDING_START)
            .pr(space::S2);
        let Some(summary) = self.selected_bot() else {
            return header;
        };
        let name = SharedString::from(summary.bot.name.clone());
        let title = SharedString::from(summary.bot.title.trim().to_owned());
        let (hover, focus) = (theme.hover, theme.border_focus);
        let name_width = text_width(window, &name)
            .min(layout::HEADER_NAME_MAX)
            .ceil();
        let button_min =
            name_width + size::AVATAR_HEADER + space::S1_5 + space::S1 * 2. + size::HAIRLINE * 2.;
        let bot_button = div()
            .id("open-bot-settings")
            .debug_selector(|| "open-bot-settings".into())
            .occlude()
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(SharedString::from(format!("{name}, bot settings")))
            .flex_shrink(layout::HEADER_TITLE_SHRINK)
            .min_w(button_min)
            .h(control::SM)
            .px(space::S1)
            .flex()
            .items_center()
            .gap(space::S1_5)
            .rounded(radius::SM)
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(focus))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_side_panel(cx)))
            .child(ui::live_avatar(summary, size::AVATAR_HEADER, theme, cx))
            .child(
                div()
                    .id("bot-name")
                    .role(Role::Heading)
                    .aria_label(name.clone())
                    .flex_none()
                    .max_w(layout::HEADER_NAME_MAX)
                    .truncate()
                    .child(crate::truncated::truncated(name)),
            )
            .children((!title.is_empty()).then(|| self.render_title(title, theme, window)));

        let running = self.open.as_ref().and_then(|open| {
            open.turn_model
                .as_ref()
                .filter(|_| open.live().turn_active())
        });
        let model = model_label(running.unwrap_or(&summary.bot.model));
        let history_open = self.history.is_some();
        let controls = div()
            .flex_shrink(layout::HEADER_CONTROLS_SHRINK)
            .min_w_0()
            .flex()
            .items_center()
            .gap(space::S1_5)
            .children(self.pending_model().map(|next| {
                let label = SharedString::from(format!("Next turn: {next}"));
                div()
                    .id("pending-model")
                    .debug_selector(|| "pending-model".into())
                    .role(Role::Status)
                    .aria_label(label.clone())
                    .min_w_0()
                    .h(layout::BADGE_HEIGHT)
                    .px(space::S2)
                    .flex()
                    .items_center()
                    .gap(space::S1)
                    .rounded_full()
                    .bg(theme.accent_soft)
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .font_weight(FontWeight::MEDIUM)
                    .child(ui::icon(Icon::Clock, icon_size::XS, theme.accent_text))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(crate::truncated::truncated(label)),
                    )
            }))
            .child(
                div()
                    .id("current-model")
                    .debug_selector(|| "current-model".into())
                    .role(Role::Label)
                    .aria_label(SharedString::from(format!("Model: {model}")))
                    .min_w_0()
                    .max_w(layout::HEADER_MODEL_MAX)
                    .h(control::SM)
                    .px(space::S2)
                    .flex()
                    .items_center()
                    .rounded(radius::MD)
                    .text_color(theme.text_secondary)
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(crate::truncated::truncated(model)),
                    ),
            )
            .child(
                div().occlude().child(
                    ui::icon_button(
                        "toggle-history",
                        Icon::History,
                        if history_open {
                            "Back to chat"
                        } else {
                            "History"
                        },
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_history();
                        cx.notify();
                    })),
                ),
            )
            .child(div().occlude().child(
                ui::icon_button("new-chat", Icon::SquarePen, "New chat", theme).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.new_chat();
                        this.focus_composer(window, cx);
                    }),
                ),
            ))
            .child(
                div()
                    .flex_none()
                    .ml(layout::DIVIDER_MARGIN)
                    .child(ui::divider(theme)),
            )
            .child(
                div().occlude().child(
                    ui::icon_button(
                        "toggle-side-panel",
                        Icon::PanelRight,
                        if self.side_panel {
                            "Hide bot settings"
                        } else {
                            "Open bot settings"
                        },
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_side_panel(cx))),
                ),
            );
        header
            .child(bot_button)
            .child(div().flex_1())
            .child(controls)
    }

    fn render_title(&self, title: SharedString, theme: &Theme, window: &Window) -> Div {
        let separator = SharedString::from("·");
        let natural = text_width(window, &separator) + space::S1_5 + text_width(window, &title);
        let hidden = title_is_hidden(self.title_width.get(), natural);
        let measured = self.title_width.clone();
        div()
            .relative()
            .flex_shrink(layout::HEADER_TITLE_SHRINK)
            .min_w_0()
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, _| {
                        let width = bounds.size.width;
                        let before = measured.replace(width);
                        if title_is_hidden(before, natural) != title_is_hidden(width, natural) {
                            window.on_next_frame(|window, _| window.refresh());
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .id("bot-title")
                    .debug_selector(|| "bot-title".into())
                    .aria_label(title.clone())
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .text_color(theme.text_placeholder)
                    .when(hidden, |row| row.invisible())
                    .child(div().flex_none().child(separator))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(crate::truncated::truncated(title)),
                    ),
            )
    }

    /// The window buttons on `side` of the title band. Windows draws its
    /// caption buttons on the right. Linux draws the buttons the desktop's
    /// layout names on that side, while the client decorates the window.
    pub(super) fn render_window_buttons(
        &self,
        side: Side,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if cfg!(target_os = "linux") {
            return self.render_linux_buttons(side, theme, window, cx);
        }
        if side == Side::Right {
            self.render_caption_buttons(theme, window, cx)
        } else {
            None
        }
    }

    #[cfg(target_os = "linux")]
    fn render_linux_buttons(
        &self,
        side: Side,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !self.frame.client {
            return None;
        }
        let buttons = crate::chrome::buttons(window, cx, side);
        if buttons.is_empty() {
            return None;
        }
        let maximized = window.is_maximized();
        Some(
            div()
                .debug_selector(move || format!("window-buttons-{side:?}").to_lowercase())
                .flex_none()
                .h_full()
                .flex()
                .children(
                    buttons
                        .into_iter()
                        .map(|button| crate::chrome::button(button, maximized, theme)),
                ),
        )
    }

    #[cfg(not(target_os = "linux"))]
    fn render_linux_buttons(
        &self,
        _: Side,
        _: &Theme,
        _: &Window,
        _: &mut Context<Self>,
    ) -> Option<Div> {
        None
    }

    fn render_caption_buttons(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !cfg!(target_os = "windows") {
            return None;
        }
        let maximized = window.is_maximized();
        let (hover, close_hover, close_text) = (theme.hover, theme.window_close, theme.on_accent);
        let caption = |id: &'static str, label: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .occlude()
                .role(Role::Button)
                .aria_label(label)
                .flex_none()
                .w(layout::CAPTION_BUTTON)
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .font(font::window_glyphs())
                .text_size(text::WINDOW_GLYPH)
                .text_color(theme.text_secondary)
                .child(glyph)
        };
        Some(
            div()
                .flex_none()
                .h_full()
                .flex()
                .child(
                    caption("window-minimize", "Minimize", "\u{e921}")
                        .window_control_area(WindowControlArea::Min)
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(|_, _, window, _| window.minimize_window())),
                )
                .child(
                    caption(
                        "window-maximize",
                        if maximized { "Restore" } else { "Maximize" },
                        if maximized { "\u{e923}" } else { "\u{e922}" },
                    )
                    .window_control_area(WindowControlArea::Max)
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(|_, _, window, _| window.zoom_window())),
                )
                .child(
                    caption("window-close", "Close", "\u{e8bb}")
                        .window_control_area(WindowControlArea::Close)
                        .hover(move |style| style.bg(close_hover).text_color(close_text))
                        .on_click(cx.listener(|_, _, window, _| window.remove_window())),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    #[test]
    fn the_title_hides_only_when_it_is_cut_below_its_minimum_width() {
        let minimum = layout::HEADER_TITLE_MIN;
        assert!(!title_is_hidden(px(200.), px(200.)));
        assert!(!title_is_hidden(minimum, px(200.)));
        assert!(title_is_hidden(minimum - px(1.), px(200.)));
        assert!(title_is_hidden(px(0.), px(200.)));
        assert!(!title_is_hidden(px(30.), px(30.)));
        assert!(!title_is_hidden(px(30.), px(30.4)));
    }
}
