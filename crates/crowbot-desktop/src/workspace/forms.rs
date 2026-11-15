use crowbot_protocol::{
    AVATAR_COLORS, Avatar, ClientMessage, ModelChoice, ModelsState, ProviderState, ProviderStatus,
    RuntimeInstall,
};
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, Hsla, Role, SharedString, Stateful,
    Window, div, prelude::*,
};

use super::Workspace;
use crate::assets::Icon;
use crate::bots;
use crate::composer::Composer;
use crate::theme::{
    Theme, control, font, icon as icon_size, layout, opacity, radius, size, space, text,
};
use crate::ui::{self, ButtonKind};

pub(super) const GLYPHS: [&str; 11] = [
    "🐦", "🦉", "🦊", "🐙", "🤖", "🧠", "📚", "🔧", "🔬", "📝", "📈",
];
const COLOR_NAMES: [&str; 17] = [
    "Red", "Orange", "Yellow", "Lime", "Green", "Cyan", "Sky blue", "Blue", "Navy", "Indigo",
    "Violet", "Magenta", "Pink", "Wine", "Brown", "Sand", "Gray",
];
/// The palette index of the automatic color of each picture: the letter, then
/// each emoji of `GLYPHS`. Pictures next to each other get different colors.
const AUTOMATIC_COLORS: [usize; 12] = [7, 0, 1, 4, 10, 6, 11, 5, 2, 9, 12, 3];

/// The place of a picture in the picker: 0 for a letter, then one per emoji.
pub(super) fn glyph_cell(glyph: &str) -> usize {
    GLYPHS
        .iter()
        .position(|candidate| *candidate == glyph)
        .map_or(0, |index| index + 1)
}

/// The color a picture takes when the color is automatic.
pub(super) fn automatic_color(cell: usize) -> &'static str {
    AVATAR_COLORS[AUTOMATIC_COLORS[cell % AUTOMATIC_COLORS.len()]]
}

/// The avatar has the automatic color of its picture.
pub(super) fn is_automatic(avatar: &Avatar) -> bool {
    avatar
        .color
        .eq_ignore_ascii_case(automatic_color(glyph_cell(&avatar.glyph)))
}

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

pub(super) fn label(label: &'static str, theme: &Theme) -> Div {
    div()
        .text_size(text::SM.size)
        .line_height(text::SM.leading)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_secondary)
        .child(label)
}

/// The label of a field that cannot stay empty, with its mark.
pub(super) fn required_label(text: &'static str, theme: &Theme) -> Div {
    label(text, theme)
        .flex()
        .gap(space::S0_5)
        .child(div().text_color(theme.danger).child("*"))
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
        .text_size(text::SM.size)
        .line_height(text::SM.leading)
        .text_color(color)
        .child(text)
}

pub(super) fn field(label_text: &'static str, control: impl IntoElement, theme: &Theme) -> Div {
    labeled(label(label_text, theme), control)
}

pub(super) fn labeled(label: Div, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::S1_5)
        .child(label)
        .child(control)
}

pub(super) fn provider_state(status: &ProviderStatus, theme: &Theme) -> (&'static str, Hsla) {
    match status.state {
        ProviderState::Ready => ("Ready", theme.success_text.into()),
        ProviderState::SignedOut => ("Signed out", theme.warning_text.into()),
        ProviderState::RuntimeMissing => ("Runtime missing", theme.danger_text.into()),
        ProviderState::NotInstalled => ("Not installed yet", theme.text_muted.into()),
        ProviderState::Unknown if status.checking => ("Checking…", theme.text_muted.into()),
        ProviderState::Unknown => ("Not known", theme.text_muted.into()),
    }
}

/// A color a view picks from the theme.
pub(super) type Tint = Hsla;

/// The color that tells the state of a provider at a glance, next to its
/// name or around its mark.
pub(super) fn provider_color(status: &ProviderStatus, theme: &Theme) -> Tint {
    match status.state {
        ProviderState::Ready => theme.success.into(),
        ProviderState::SignedOut => theme.warning.into(),
        ProviderState::RuntimeMissing => theme.danger.into(),
        ProviderState::NotInstalled | ProviderState::Unknown => theme.text_dim.into(),
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
        ProviderState::Ready | ProviderState::NotInstalled | ProviderState::Unknown => None,
    }
}

/// What a person should know about a provider before a bot runs on it: what
/// stops it, as a warning, or a runtime it does not have yet, which a turn
/// downloads and which only then takes a sign-in.
pub(super) fn provider_notice(status: &ProviderStatus) -> Option<(SharedString, bool)> {
    if let Some(problem) = provider_problem(status) {
        return Some((problem, true));
    }
    (status.state == ProviderState::NotInstalled).then(|| {
        let downloads = status.detail.clone().unwrap_or_else(|| {
            format!(
                "Crow Bot downloads {} when a bot first needs it.",
                status.name
            )
        });
        let text = format!("{downloads} Install it now to sign in before the first message.");
        (text.into(), false)
    })
}

/// One choice of a picker. `value` is what the choice stores: `None` is the
/// default of the provider.
pub(super) struct Choice {
    pub value: Option<String>,
    pub label: SharedString,
    pub chosen: bool,
}

type Pick = fn(&mut Workspace, Option<String>, &mut Window, &mut Context<Workspace>);

fn megabytes(bytes: u64) -> u64 {
    bytes.div_ceil(1 << 20)
}

/// How far the download of a runtime is, in words.
pub(super) fn download_text(name: &str, install: RuntimeInstall) -> SharedString {
    format!(
        "Downloading the {name} runtime: {} of {} MB…",
        megabytes(install.received),
        megabytes(install.total)
    )
    .into()
}

fn default_choice(default: Option<&str>, listed: &[ModelChoice], chosen: bool) -> Choice {
    let name = default.map(|default| {
        listed
            .iter()
            .find(|choice| choice.id == default)
            .map_or(default, |choice| choice.name.as_str())
    });
    Choice {
        value: None,
        label: match name {
            Some(name) => format!("Default: {name}").into(),
            None => "Default".into(),
        },
        chosen,
    }
}

fn choices(default: Option<&str>, listed: &[ModelChoice], current: Option<&str>) -> Vec<Choice> {
    std::iter::once(default_choice(default, listed, current.is_none()))
        .chain(listed.iter().map(|choice| Choice {
            value: Some(choice.id.clone()),
            label: choice.name.clone().into(),
            chosen: current == Some(choice.id.as_str()),
        }))
        .collect()
}

impl Workspace {
    /// The providers a bot can run on, with the one it runs on marked.
    pub(super) fn provider_choices(&self, current: &str) -> Vec<Choice> {
        self.providers
            .iter()
            .map(|status| Choice {
                value: Some(status.id.clone()),
                label: status.name.clone().into(),
                chosen: status.id == current,
            })
            .collect()
    }

    /// The models of a provider, after its default, with the current one
    /// marked. A provider that lists no model has its default only.
    pub(super) fn model_choices(&self, provider: &str, current: Option<&str>) -> Vec<Choice> {
        match self.models.get(provider) {
            Some(models) => choices(models.default_model.as_deref(), &models.models, current),
            None => choices(None, &[], current),
        }
    }

    /// The reasoning levels of a provider, after its default. Empty when the
    /// provider has none.
    pub(super) fn effort_choices(&self, provider: &str, current: Option<&str>) -> Vec<Choice> {
        match self.models.get(provider) {
            Some(models) if !models.efforts.is_empty() => {
                choices(models.default_effort.as_deref(), &models.efforts, current)
            }
            _ => Vec::new(),
        }
    }

    /// Why the models of a provider are not listed, when they are not.
    pub(super) fn models_note(&self, provider: &str) -> Option<SharedString> {
        let models = self.models.get(provider)?;
        match models.state {
            ModelsState::Listed => None,
            ModelsState::Loading => Some("Asking the provider for its models…".into()),
            ModelsState::Unavailable => {
                let reason = models
                    .detail
                    .clone()
                    .unwrap_or_else(|| "The list of models is not available.".to_owned());
                Some(format!("{reason} The bot runs on the default model of the provider.").into())
            }
        }
    }

    /// A row of choices, one of which is marked.
    pub(super) fn choice_chips(
        &self,
        prefix: &'static str,
        name: &'static str,
        choices: Vec<Choice>,
        pick: Pick,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let live = self.live();
        let (hover, focus, accent) = (theme.control_hover, theme.border_focus, theme.accent);
        div()
            .id(prefix)
            .debug_selector(move || prefix.into())
            .role(Role::Group)
            .aria_label(name)
            .flex()
            .flex_wrap()
            .gap(space::S1)
            .when(!live, |chips| chips.opacity(opacity::DISABLED))
            .children(choices.into_iter().enumerate().map(|(index, choice)| {
                let Choice {
                    value,
                    label,
                    chosen,
                } = choice;
                let spoken = if chosen {
                    format!("{label}, selected")
                } else {
                    label.to_string()
                };
                div()
                    .id((prefix, index))
                    .debug_selector(move || {
                        if chosen {
                            format!("{prefix}-{index}-chosen")
                        } else {
                            format!("{prefix}-{index}")
                        }
                    })
                    .focusable()
                    .tab_stop(live)
                    .role(Role::Button)
                    .aria_label(SharedString::from(spoken))
                    .h(layout::BADGE_HEIGHT)
                    .px(space::S2)
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(if chosen {
                        Hsla::from(accent)
                    } else {
                        theme.border_strong.into()
                    })
                    .when(chosen, |chip| {
                        chip.bg(theme.accent_soft).font_weight(FontWeight::MEDIUM)
                    })
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .when(live, |chip| {
                        chip.cursor_pointer()
                            .focus_visible(move |style| style.border_color(focus))
                            .when(!chosen, |chip| chip.hover(move |style| style.bg(hover)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                pick(this, value.clone(), window, cx);
                                cx.notify();
                            }))
                    })
                    .child(label)
            }))
    }

    /// What stops a provider from running a turn, with the controls that fix it.
    pub(super) fn render_provider_notice(
        &self,
        status: &ProviderStatus,
        prefix: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (problem, warning) = provider_notice(status)?;
        let (surface, mark, tint) = if warning {
            (theme.warning_soft, Icon::TriangleAlert, theme.warning_text)
        } else {
            (theme.hover, Icon::Info, theme.text_muted)
        };
        Some(
            div()
                .id((prefix, 40_usize))
                .debug_selector(move || format!("{prefix}-provider-notice"))
                .role(Role::Status)
                .aria_label(problem.clone())
                .p(space::S3)
                .flex()
                .flex_col()
                .gap(space::S2)
                .rounded(radius::MD)
                .bg(surface)
                .child(
                    div()
                        .flex()
                        .gap(space::S2)
                        .child(
                            div()
                                .flex_none()
                                .h(text::MD.leading)
                                .flex()
                                .items_center()
                                .child(ui::icon(mark, icon_size::SM, tint)),
                        )
                        .child(div().flex_1().min_w_0().child(problem)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(space::S2)
                        .children(self.provider_controls(status, prefix, theme, cx)),
                )
                .into_any_element(),
        )
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
        ui::input_skin(
            div()
                .id(id)
                .debug_selector(move || id.into())
                .min_h(layout::FIELD_HEIGHT)
                .px(space::S3)
                .py(layout::FIELD_PADDING_Y)
                .flex()
                .items_center(),
            focused,
            invalid,
            theme,
        )
        .on_click(move |_, window, cx| window.focus(&handle, cx))
        .child(div().flex_1().min_w_0().child(input.clone()))
    }

    /// The avatar colors: the automatic one, which follows the picture, then
    /// the palette, in rows of `layout::SWATCH_COLUMNS`.
    pub(super) fn render_color_picker(
        &self,
        owner: AvatarOwner,
        avatar: &Avatar,
        automatic: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let (ring, focus, hover, edge) = (
            Hsla::from(theme.accent_text),
            theme.border_focus,
            theme.hover,
            theme.border_strong,
        );
        let prefix = owner.prefix();
        let auto = automatic_color(glyph_cell(&avatar.glyph));
        let swatches: Vec<Stateful<Div>> = std::iter::once((None, auto))
            .chain(
                AVATAR_COLORS
                    .iter()
                    .enumerate()
                    .map(|(index, color)| (Some(index), *color)),
            )
            .map(|(choice, color)| {
                let chosen = match choice {
                    None => automatic,
                    Some(_) => !automatic && color.eq_ignore_ascii_case(&avatar.color),
                };
                let name = match choice {
                    None => "Automatic color",
                    Some(index) => COLOR_NAMES[index],
                };
                let label = if chosen {
                    format!("{name}, selected")
                } else {
                    name.to_owned()
                };
                let (id, selector) = match choice {
                    None => (
                        gpui::ElementId::from((prefix, usize::MAX)),
                        format!("{prefix}-color-auto"),
                    ),
                    Some(index) => (
                        gpui::ElementId::from((prefix, index)),
                        format!("{prefix}-color-{index}"),
                    ),
                };
                div()
                    .id(id)
                    .debug_selector(move || selector.clone())
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
                    .hover(move |style| style.bg(hover))
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pick_color(owner, choice, cx);
                    }))
                    .child(
                        div()
                            .size(size::SWATCH)
                            .rounded_full()
                            .border_1()
                            .border_color(edge)
                            .bg(bots::avatar_color(color).unwrap_or(theme.accent)),
                    )
            })
            .collect();
        let mut rows = Vec::new();
        let mut swatches = swatches.into_iter().peekable();
        while swatches.peek().is_some() {
            rows.push(
                div()
                    .flex()
                    .justify_center()
                    .gap(space::S1)
                    .children(swatches.by_ref().take(layout::SWATCH_COLUMNS)),
            );
        }
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(space::S1)
            .children(rows)
    }

    /// The pictures an avatar can show: the first letter of the name, then
    /// the emoji, each drawn as the avatar it gives, in the current color.
    pub(super) fn render_glyph_picker(
        &self,
        owner: AvatarOwner,
        initial: &str,
        avatar: &Avatar,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let (ring, focus, hover) = (
            Hsla::from(theme.accent_text),
            theme.border_focus,
            theme.hover,
        );
        let halo = theme.accent_soft;
        let prefix = owner.prefix();
        let chosen_cell = glyph_cell(&avatar.glyph);
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
                let chosen = cell == chosen_cell;
                let label = match (choice, chosen) {
                    (None, true) => "The first letter of the name, selected".to_owned(),
                    (None, false) => "The first letter of the name".to_owned(),
                    (Some(_), true) => format!("{glyph}, selected"),
                    (Some(_), false) => glyph.clone(),
                };
                let sample = Avatar {
                    color: avatar.color.clone(),
                    glyph,
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
                    .when(chosen, |cell| {
                        cell.shadow(vec![gpui::BoxShadow {
                            color: halo.into(),
                            offset: gpui::point(gpui::px(0.), gpui::px(0.)),
                            blur_radius: gpui::px(0.),
                            spread_radius: size::RING,
                            inset: false,
                        }])
                    })
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pick_glyph(owner, choice, cx);
                    }))
                    .child(ui::avatar_of(
                        &sample,
                        size::AVATAR_FACE,
                        size::AVATAR_FACE_GLYPH,
                        theme,
                    ))
            }))
    }

    /// `None` is the automatic color.
    fn pick_color(&mut self, owner: AvatarOwner, choice: Option<usize>, cx: &mut Context<Self>) {
        match owner {
            AvatarOwner::Creation => {
                if let Some(creation) = self.creation.as_mut() {
                    creation.color = choice;
                }
            }
            AvatarOwner::Panel => {
                if let Some(bot) = self.selected_bot().map(|summary| &summary.bot) {
                    let color = match choice {
                        Some(index) => AVATAR_COLORS[index],
                        None => automatic_color(glyph_cell(&bot.avatar.glyph)),
                    };
                    let avatar = Avatar {
                        color: color.to_owned(),
                        glyph: bot.avatar.glyph.clone(),
                    };
                    self.save_avatar(bot.id, avatar);
                }
            }
        }
        cx.notify();
    }

    /// `None` is the first letter of the name. A bot on its automatic color
    /// takes the automatic color of its new picture.
    fn pick_glyph(&mut self, owner: AvatarOwner, choice: Option<usize>, cx: &mut Context<Self>) {
        match owner {
            AvatarOwner::Creation => {
                if let Some(creation) = self.creation.as_mut() {
                    creation.glyph = choice;
                }
            }
            AvatarOwner::Panel => {
                if let Some(bot) = self.selected_bot().map(|summary| &summary.bot) {
                    let cell = choice.map_or(0, |index| index + 1);
                    let color = if is_automatic(&bot.avatar) {
                        automatic_color(cell).to_owned()
                    } else {
                        bot.avatar.color.clone()
                    };
                    let avatar = Avatar {
                        color,
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
        let name = &status.name;
        // Several providers show the same controls: each one is spoken with
        // the name of its provider.
        let spoken = |label: String| SharedString::from(label);
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
            // The page asks the person to check that it shows the code of
            // this sign-in: the row shows it too.
            if let Some(code) = status.sign_in_url.as_deref().and_then(sign_in_code) {
                let shown = selector("sign-in-code");
                controls.push(
                    div()
                        .id((prefix, 4_usize))
                        .debug_selector(move || shown.clone())
                        .role(Role::Status)
                        .aria_label(spoken(format!("Sign-in code of {name}: {code}")))
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_color(theme.text_muted).child("Code"))
                        .child(
                            div()
                                .font(font::mono())
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(code),
                        )
                        .into_any_element(),
                );
            }
            if let Some(page) = status.sign_in_url.clone() {
                let open = selector("sign-in-page");
                controls.push(
                    ui::button(
                        (prefix, 3_usize),
                        "Open the page",
                        ButtonKind::Outline,
                        theme,
                    )
                    .debug_selector(move || open.clone())
                    .aria_label(spoken(format!("Open the sign-in page of {name}")))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_link(&page, cx)))
                    .into_any_element(),
                );
            }
            controls.push(
                dimmed(ui::button(
                    (prefix, 2_usize),
                    "Cancel",
                    ButtonKind::Outline,
                    theme,
                ))
                .debug_selector(move || cancel.clone())
                .aria_label(spoken(format!("Cancel the sign-in to {name}")))
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
            // A provider with several sign-ins has one button for each, by
            // its name. With one, the button starts that one.
            let methods: Vec<(Option<String>, SharedString)> = if status.sign_in_methods.len() > 1 {
                status
                    .sign_in_methods
                    .iter()
                    .map(|method| (Some(method.id.clone()), method.name.clone().into()))
                    .collect()
            } else {
                vec![(None, "Sign in".into())]
            };
            for (index, (method, label)) in methods.into_iter().enumerate() {
                let id = provider.clone();
                let sign_in = match &method {
                    Some(method) => selector(&format!("sign-in-{method}")),
                    None => selector("sign-in"),
                };
                let kind = if index == 0 {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Outline
                };
                let said = match &method {
                    Some(_) => format!("{name}: {label}"),
                    None => format!("Sign in to {name}"),
                };
                controls.push(
                    dimmed(ui::button((prefix, 10 + index), label, kind, theme))
                        .debug_selector(move || sign_in.clone())
                        .aria_label(spoken(said))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.provider_request(ClientMessage::SignIn {
                                provider: id.clone(),
                                method: method.clone(),
                            });
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        if let Some(install) = status.install {
            let download = selector("download");
            let progress = download_text(&status.name, install);
            controls.push(
                div()
                    .id((prefix, 7_usize))
                    .debug_selector(move || download.clone())
                    .role(Role::Status)
                    .aria_label(progress.clone())
                    .h(control::MD)
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .text_color(theme.text_muted)
                    .child(ui::spinner(
                        (prefix, 8_usize),
                        icon_size::SM,
                        theme.text_muted,
                    ))
                    .child(progress)
                    .into_any_element(),
            );
        } else if status.state == ProviderState::NotInstalled {
            let id = provider.clone();
            let install = selector("install");
            controls.push(
                dimmed(ui::button(
                    (prefix, 9_usize),
                    "Install now",
                    ButtonKind::Outline,
                    theme,
                ))
                .debug_selector(move || install.clone())
                .aria_label(spoken(format!("Install the {name} runtime now")))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.provider_request(ClientMessage::InstallRuntime {
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
                .aria_label(spoken(format!("Check {name} again")))
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

/// The code a sign-in page carries in its address, as the person must find
/// it on that page. Only letters, digits, and dashes are taken.
pub(super) fn sign_in_code(page: &str) -> Option<String> {
    let (_, query) = page.split_once('?')?;
    let code = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("user_code="))?;
    let plain = !code.is_empty()
        && code.len() <= 16
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    plain.then(|| code.to_ascii_uppercase())
}
