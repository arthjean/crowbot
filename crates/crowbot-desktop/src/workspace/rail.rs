use crowbot_protocol::ProviderStatus;
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, Pixels, Role, SharedString, Stateful, div,
    prelude::*,
};

use super::Workspace;
use super::forms;
use crate::assets::Icon;
use crate::chrome::WindowDrag;
use crate::theme::{Theme, control, icon as icon_size, layout, radius, size, space, text};
use crate::ui;

const RAIL_BUTTON_GROUP: &str = "rail-button";

/// The logo Crow Bot ships for a provider, to name it at a glance. The marks
/// come from LobeHub Icons (MIT); see `assets/icons/providers/LICENSE`.
pub(super) fn provider_logo(id: &str) -> Option<Icon> {
    match id {
        "claude" => Some(Icon::ProviderClaude),
        "codex" => Some(Icon::ProviderOpenAi),
        "grok" => Some(Icon::ProviderGrok),
        _ => None,
    }
}

/// The mark of a provider: its logo at `logo`, or the letters of its name
/// for a provider with no logo.
pub(super) fn provider_glyph(
    status: &ProviderStatus,
    logo: Pixels,
    color: impl Into<Hsla>,
) -> AnyElement {
    let color = color.into();
    match provider_logo(&status.id) {
        Some(icon) => ui::icon(icon, logo, color).into_any_element(),
        None => div()
            .text_size(text::SM.size)
            .line_height(text::SM.leading)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color)
            .child(provider_mark(&status.name))
            .into_any_element(),
    }
}

/// The short mark of a provider inside its ring: two letters, since two
/// providers share their first one.
pub(super) fn provider_mark(name: &str) -> SharedString {
    name.chars().take(2).collect::<String>().into()
}

fn rail_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    let label = label.into();
    let (hover, focus) = (theme.control_hover, theme.border_focus);
    div()
        .id(id)
        .group(RAIL_BUTTON_GROUP)
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(label.clone())
        .flex_none()
        .size(control::LG)
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::LG)
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .focus_visible(move |style| style.border_color(focus))
        .tooltip(ui::tooltip(label))
}

/// A page of the window on the rail. The page shown keeps the fill a button
/// takes under the pointer, and its icon in the text color.
fn rail_icon(
    id: &'static str,
    glyph: Icon,
    label: &'static str,
    shown: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let hover_text = theme.text;
    rail_button(id, label, theme)
        .debug_selector(move || id.into())
        .when(shown, |button| {
            button
                .bg(theme.control_hover)
                .aria_label(SharedString::from(format!("{label}, selected")))
        })
        .child(
            ui::icon(
                glyph,
                icon_size::LG,
                if shown { theme.text } else { theme.text_muted },
            )
            .group_hover(RAIL_BUTTON_GROUP, move |style| style.text_color(hover_text)),
        )
}

impl Workspace {
    /// The column of app-wide controls at the left edge of the window: the
    /// state of each provider, and the settings.
    pub(super) fn render_rail(&self, theme: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let logo = div()
            .id("rail-logo")
            .debug_selector(|| "rail-logo".into())
            .window_drag()
            .flex_none()
            .w_full()
            .h(layout::TOP_ROW)
            .flex()
            .items_center()
            .justify_center()
            .when(!cfg!(target_os = "macos"), |row| {
                row.child(ui::icon(Icon::Bird, layout::RAIL_LOGO, theme.text))
            });
        let providers: Vec<Stateful<Div>> = self
            .providers
            .iter()
            .enumerate()
            .map(|(index, status)| {
                let (state, _) = forms::provider_state(status, theme);
                let label = SharedString::from(format!("{}: {state}", status.name));
                let id = status.id.clone();
                let opened = status.id.clone();
                rail_button(("rail-provider", index), label, theme)
                    .debug_selector(move || format!("rail-provider-{id}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_provider_settings(&opened, window, cx);
                    }))
                    .child(
                        div()
                            .size(size::PROVIDER_RING)
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border(size::RING)
                            .border_color(forms::provider_color(status, theme))
                            .child(provider_glyph(
                                status,
                                size::PROVIDER_LOGO,
                                theme.text_secondary,
                            )),
                    )
            })
            .collect();
        let in_settings = self.settings.is_some();
        let bots = rail_icon("open-bots", Icon::Bot, "Bots", !in_settings, theme)
            .on_click(cx.listener(|this, _, window, cx| this.close_settings(window, cx)));
        let settings = rail_icon(
            "open-settings",
            Icon::Settings,
            "Settings",
            in_settings,
            theme,
        )
        .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)));
        div()
            .id("rail")
            .debug_selector(|| "rail".into())
            .role(Role::Toolbar)
            .aria_label("Crow Bot")
            .flex_none()
            .w(layout::RAIL_WIDTH)
            .h_full()
            .pb(space::S3)
            .flex()
            .flex_col()
            .items_center()
            .gap(space::S1_5)
            .child(logo)
            .child(bots)
            .child(div().id("rail-spacer").window_drag().flex_1().w_full())
            .children(providers)
            .child(
                div()
                    .flex_none()
                    .w(size::RAIL_SEPARATOR)
                    .h(size::HAIRLINE)
                    .bg(theme.hover),
            )
            .child(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_mark_tells_apart_providers_with_the_same_first_letter() {
        assert_eq!(provider_mark("Claude"), "Cl");
        assert_eq!(provider_mark("ChatGPT"), "Ch");
        assert_eq!(provider_mark("Grok"), "Gr");
        assert_eq!(provider_mark("X"), "X");
    }
}
