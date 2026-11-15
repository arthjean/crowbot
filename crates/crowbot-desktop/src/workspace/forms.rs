use crowbot_protocol::{AVATAR_COLORS, Avatar, ProviderState, ProviderStatus};
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, Hsla, Role, SharedString, Stateful,
    Window, div, prelude::*,
};

use super::Workspace;
use crate::assets::Icon;
use crate::bots;
use crate::composer::Composer;
use crate::theme::{Theme, control, icon as icon_size, layout, opacity, radius, size, space, text};
use crate::ui::{self, ButtonKind};

pub(super) const GLYPHS: [&str; 11] = [
    "🐦", "🦉", "🦊", "🐙", "🤖", "🧠", "📚", "🔧", "🔬", "📝", "📈",
];
const COLOR_NAMES: [&str; 8] = [
    "Blue", "Green", "Amber", "Red", "Violet", "Pink", "Teal", "Orange",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AvatarOwner {
    Creation,
    Panel,
}

impl AvatarOwner {
    fn prefix(self) -> &'static str {
        match self {
            Self::Creation => "creation",
            Self::Panel => "panel",
        }
    }
}

pub(super) fn initial(name: &str) -> String {
    let first: String = name
        .trim()
        .chars()
        .take(1)
        .flat_map(char::to_uppercase)
        .collect();
    if first.is_empty() {
        "?".to_owned()
    } else {
        first
    }
}

pub(super) fn label(text: &'static str, theme: &Theme) -> Div {
    div()
        .pl(layout::FIELD_LABEL_INDENT)
        .text_color(theme.text_secondary)
        .child(text)
}

pub(super) fn group_heading(text: &'static str, theme: &Theme) -> Div {
    div()
        .pl(space::S1)
        .text_size(text::SM.size)
        .line_height(text::SM.leading)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(text)
}

pub(super) fn card(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(radius::MD)
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.surface)
        .overflow_hidden()
}

pub(super) fn note(id: &'static str, text: impl Into<SharedString>, color: impl Into<Hsla>) -> Div {
    let text = text.into();
    div()
        .debug_selector(move || id.into())
        .pl(layout::FIELD_LABEL_INDENT)
        .text_size(text::SM.size)
        .line_height(text::SM.leading)
        .text_color(color)
        .child(text)
}

pub(super) fn field(label_text: &'static str, control: impl IntoElement, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::S1_5)
        .child(label(label_text, theme))
        .child(control)
}

pub(super) fn provider_state(status: &ProviderStatus, theme: &Theme) -> (&'static str, Hsla) {
    match status.state {
        ProviderState::Ready => ("Ready", theme.success_text.into()),
        ProviderState::SignedOut => ("Signed out", theme.warning_text.into()),
        ProviderState::RuntimeMissing => ("Runtime missing", theme.danger_text.into()),
        ProviderState::Unknown if status.checking => ("Checking…", theme.text_muted.into()),
        ProviderState::Unknown => ("Not known", theme.text_muted.into()),
    }
}

/// What stops a provider from running a turn, in one sentence.
pub(super) fn provider_problem(status: &ProviderStatus) -> Option<SharedString> {
    let name = &status.name;
    match status.state {
        ProviderState::SignedOut => {
            Some(format!("{name} is signed out. A bot cannot answer until you sign in.").into())
        }
        ProviderState::RuntimeMissing => Some(
            match &status.detail {
                Some(detail) => format!("{name} cannot start. {detail}"),
                None => format!("{name} cannot start: its runtime is missing."),
            }
            .into(),
        ),
        ProviderState::Ready | ProviderState::Unknown => None,
    }
}

impl Workspace {
    pub(super) fn input_box(
        &self,
        id: &'static str,
        input: &Entity<Composer>,
        invalid: bool,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let handle = input.focus_handle(cx);
        let focused = handle.is_focused(window);
        let border = if invalid {
            theme.danger
        } else if focused {
            theme.border_focus
        } else {
            theme.border_strong
        };
        div()
            .id(id)
            .debug_selector(move || id.into())
            .min_h(layout::FIELD_HEIGHT)
            .px(space::S3)
            .py(layout::FIELD_PADDING_Y)
            .flex()
            .items_center()
            .rounded(radius::MD)
            .bg(theme.control)
            .border_1()
            .border_color(border)
            .on_click(move |_, window, cx| window.focus(&handle, cx))
            .child(div().flex_1().min_w_0().child(input.clone()))
    }

    pub(super) fn render_color_picker(
        &self,
        owner: AvatarOwner,
        selected: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let (ring, focus) = (Hsla::from(theme.accent), theme.border_focus);
        let prefix = owner.prefix();
        div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(space::S1)
            .children(AVATAR_COLORS.iter().enumerate().map(|(index, color)| {
                let chosen = color.eq_ignore_ascii_case(selected);
                let name = COLOR_NAMES[index];
                let label = if chosen {
                    format!("{name}, selected")
                } else {
                    name.to_owned()
                };
                div()
                    .id((prefix, index))
                    .debug_selector(move || format!("{prefix}-color-{index}"))
                    .focusable()
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label(SharedString::from(label))
                    .flex_none()
                    .size(size::SWATCH_BUTTON)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(if chosen {
                        ring
                    } else {
                        gpui::transparent_black()
                    })
                    .cursor_pointer()
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pick_color(owner, index, cx);
                    }))
                    .child(
                        div()
                            .size(size::SWATCH)
                            .rounded_full()
                            .bg(bots::avatar_color(color).unwrap_or(theme.accent)),
                    )
            }))
    }

    pub(super) fn render_glyph_picker(
        &self,
        owner: AvatarOwner,
        initial: &str,
        selected: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let (ring, focus, hover) = (Hsla::from(theme.accent), theme.border_focus, theme.hover);
        let prefix = owner.prefix();
        let cells = std::iter::once((None, initial.to_owned())).chain(
            GLYPHS
                .iter()
                .enumerate()
                .map(|(index, glyph)| (Some(index), (*glyph).to_owned())),
        );
        div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(space::S0_5)
            .children(cells.enumerate().map(|(cell, (choice, glyph))| {
                let chosen = glyph == selected;
                let label = match (choice, chosen) {
                    (None, true) => "The first letter of the name, selected".to_owned(),
                    (None, false) => "The first letter of the name".to_owned(),
                    (Some(_), true) => format!("{glyph}, selected"),
                    (Some(_), false) => glyph.clone(),
                };
                div()
                    .id((prefix, AVATAR_COLORS.len() + cell))
                    .debug_selector(move || format!("{prefix}-glyph-{cell}"))
                    .focusable()
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label(SharedString::from(label))
                    .flex_none()
                    .size(size::GLYPH_CELL)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::MD)
                    .border_1()
                    .border_color(if chosen {
                        ring
                    } else {
                        gpui::transparent_black()
                    })
                    .when(chosen, |cell| cell.bg(theme.accent_soft))
                    .text_size(size::GLYPH_CELL_TEXT)
                    .line_height(size::GLYPH_CELL)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pick_glyph(owner, choice, cx);
                    }))
                    .child(SharedString::from(glyph))
            }))
    }

    fn pick_color(&mut self, owner: AvatarOwner, index: usize, cx: &mut Context<Self>) {
        match owner {
            AvatarOwner::Creation => {
                if let Some(creation) = self.creation.as_mut() {
                    creation.color = index;
                }
            }
            AvatarOwner::Panel => {
                if let Some(bot) = self.selected_bot().map(|summary| &summary.bot) {
                    let avatar = Avatar {
                        color: AVATAR_COLORS[index].to_owned(),
                        glyph: bot.avatar.glyph.clone(),
                    };
                    self.save_avatar(bot.id, avatar);
                }
            }
        }
        cx.notify();
    }

    fn pick_glyph(&mut self, owner: AvatarOwner, choice: Option<usize>, cx: &mut Context<Self>) {
        match owner {
            AvatarOwner::Creation => {
                if let Some(creation) = self.creation.as_mut() {
                    creation.glyph = choice;
                }
            }
            AvatarOwner::Panel => {
                if let Some(bot) = self.selected_bot().map(|summary| &summary.bot) {
                    let avatar = Avatar {
                        color: bot.avatar.color.clone(),
                        glyph: match choice {
                            Some(index) => GLYPHS[index].to_owned(),
                            None => initial(&bot.name),
                        },
                    };
                    self.save_avatar(bot.id, avatar);
                }
            }
        }
        cx.notify();
    }

    /// The controls that fix a provider: its sign-in, the page of its runtime,
    /// and a new check of its state.
    pub(super) fn provider_controls(
        &self,
        status: &ProviderStatus,
        prefix: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let live = self.live();
        let provider = status.id.clone();
        let selector = move |name: &str| format!("{prefix}-{name}");
        let dimmed = |button: Stateful<Div>| {
            if live {
                button
            } else {
                button.opacity(opacity::DISABLED)
            }
        };
        let mut controls = Vec::new();
        if status.signing_in {
            let id = provider.clone();
            let cancel = selector("cancel-sign-in");
            controls.push(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .text_color(theme.text_muted)
                    .child(ui::spinner(
                        (prefix, 1_usize),
                        icon_size::SM,
                        theme.text_muted,
                    ))
                    .child("Finish signing in in your browser.")
                    .into_any_element(),
            );
            controls.push(
                dimmed(ui::button(
                    (prefix, 2_usize),
                    "Cancel",
                    ButtonKind::Outline,
                    theme,
                ))
                .debug_selector(move || cancel.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.provider_request(crowbot_protocol::ClientMessage::CancelSignIn {
                        provider: id.clone(),
                    });
                    cx.notify();
                }))
                .into_any_element(),
            );
            return controls;
        }
        if status.state == ProviderState::SignedOut && status.can_sign_in {
            let id = provider.clone();
            let sign_in = selector("sign-in");
            controls.push(
                dimmed(ui::button(
                    (prefix, 3_usize),
                    "Sign in",
                    ButtonKind::Primary,
                    theme,
                ))
                .debug_selector(move || sign_in.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.provider_request(crowbot_protocol::ClientMessage::SignIn {
                        provider: id.clone(),
                    });
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        if let Some(url) = status.help_url.clone() {
            let runtime = selector("get-runtime");
            controls.push(
                ui::button(
                    (prefix, 4_usize),
                    "Get the runtime",
                    ButtonKind::Outline,
                    theme,
                )
                .debug_selector(move || runtime.clone())
                .child(ui::icon(
                    Icon::ExternalLink,
                    icon_size::SM,
                    theme.text_muted,
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.open_link(&url, cx)))
                .into_any_element(),
            );
        }
        let check = selector("check");
        if status.checking {
            controls.push(
                div()
                    .debug_selector(move || check.clone())
                    .h(control::MD)
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .text_color(theme.text_muted)
                    .child(ui::spinner(
                        (prefix, 5_usize),
                        icon_size::SM,
                        theme.text_muted,
                    ))
                    .child("Checking…")
                    .into_any_element(),
            );
        } else {
            controls.push(
                dimmed(ui::button(
                    (prefix, 6_usize),
                    "Check again",
                    ButtonKind::Ghost,
                    theme,
                ))
                .debug_selector(move || check.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.provider_request(crowbot_protocol::ClientMessage::CheckProvider {
                        provider: provider.clone(),
                    });
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        controls
    }
}
