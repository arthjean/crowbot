use crowbot_protocol::{Avatar, Bot, BotState, BotSummary};
use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, Context, Div, ElementId, FontWeight, Hsla,
    Pixels, Role, SharedString, Stateful, Svg, Transformation, Window, div, percentage, prelude::*,
    svg,
};

use crate::assets::Icon;
use crate::mascot::{self, Mood};
use crate::theme::{
    Theme, control, duration, font, icon as icon_size, layout, menu, opacity, radius, size, space,
    text,
};

const ICON_BUTTON_GROUP: &str = "icon-button";

pub fn icon(icon: Icon, size: Pixels, color: impl Into<Hsla>) -> Svg {
    svg()
        .path(icon.path())
        .flex_none()
        .size(size)
        .text_color(color)
}

pub fn spinner(id: impl Into<ElementId>, size: Pixels, color: impl Into<Hsla>) -> AnyElement {
    icon(Icon::Loader, size, color)
        .with_animation(
            id,
            Animation::new(duration::SPINNER).repeat(),
            |spinner, turn| spinner.with_transformation(Transformation::rotate(percentage(turn))),
        )
        .into_any_element()
}

struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .max_w(layout::TOOLTIP_MAX_WIDTH)
            .px(space::S2)
            .py(layout::TOOLTIP_PADDING_Y)
            .rounded(radius::SM)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .font(font::sans())
            .text_color(theme.text)
            .text_size(text::SM.size)
            .line_height(text::SM.leading)
            .font_weight(FontWeight::SEMIBOLD)
            .child(self.text.clone())
    }
}

pub fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Outline,
    /// An outline button on a card: one step above the `surface` fill.
    Raised,
    Ghost,
    Danger,
    Disabled,
    /// The primary action while it cannot run yet: the primary look, faded.
    PrimaryDisabled,
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    theme: &Theme,
) -> Stateful<Div> {
    let label = label.into();
    let focus = theme.border_focus;
    let id = id.into();
    let base = div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .focusable()
        .tab_stop(!matches!(
            kind,
            ButtonKind::Disabled | ButtonKind::PrimaryDisabled
        ))
        .role(Role::Button)
        .aria_label(label.clone())
        .flex_none()
        .h(control::MD)
        .px(space::S3)
        .flex()
        .items_center()
        .justify_center()
        .gap(space::S1_5)
        .rounded(radius::SM)
        .border_1()
        .text_size(text::MD.size)
        .line_height(text::MD.leading)
        .font_weight(FontWeight::MEDIUM)
        .focus_visible(move |style| style.border_color(focus))
        .child(label);
    let transparent = gpui::transparent_black();
    match kind {
        ButtonKind::Primary => {
            let hover = Hsla::from(theme.inverse_surface).opacity(opacity::PRIMARY_HOVER);
            base.bg(theme.inverse_surface)
                .border_color(theme.inverse_surface)
                .text_color(theme.inverse_text)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
        }
        ButtonKind::Outline => {
            let hover = theme.control_hover;
            base.border_color(theme.border_strong)
                .bg(theme.control)
                .text_color(theme.text)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
        }
        ButtonKind::Raised => {
            let hover = theme.control_active;
            base.border_color(theme.border_strong)
                .bg(theme.surface_raised)
                .text_color(theme.text)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
        }
        ButtonKind::Ghost => {
            let hover = theme.hover;
            base.border_color(transparent)
                .text_color(theme.text)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
        }
        ButtonKind::Danger => {
            let hover = theme.danger_soft_hover;
            base.bg(theme.danger_soft)
                .border_color(transparent)
                .text_color(theme.danger_text)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
        }
        ButtonKind::Disabled => base
            .bg(theme.hover)
            .border_color(transparent)
            .text_color(theme.text_muted),
        ButtonKind::PrimaryDisabled => base
            .bg(theme.inverse_surface)
            .border_color(theme.inverse_surface)
            .text_color(theme.inverse_text)
            .opacity(opacity::DISABLED),
    }
}

/// A round icon button: 36 across, a filled circle with a hairline edge and
/// a 14 icon, lighter under the pointer.
pub fn round_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    let label = label.into();
    let (focus, hover) = (theme.border_focus, theme.round_button_hover);
    let id = id.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(label.clone())
        .flex_none()
        .size(size::ROUND_BUTTON)
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(theme.round_button_edge)
        .bg(theme.round_button)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .focus_visible(move |style| style.border_color(focus))
        .tooltip(tooltip(label))
        .child(icon(glyph, icon_size::SM, theme.text_secondary))
}

pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    let label = label.into();
    let (focus, hover, hover_text) = (theme.border_focus, theme.hover, theme.text);
    let id = id.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .group(ICON_BUTTON_GROUP)
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(label.clone())
        .flex_none()
        .size(control::SM)
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SM)
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .focus_visible(move |style| style.border_color(focus))
        .tooltip(tooltip(label))
        .child(
            icon(glyph, icon_size::MD, theme.text_muted)
                .group_hover(ICON_BUTTON_GROUP, move |style| style.text_color(hover_text)),
        )
}

pub fn small_icon_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    let label = label.into();
    let (focus, hover, hover_text) = (theme.border_focus, theme.hover, theme.text);
    div()
        .id(id)
        .group(ICON_BUTTON_GROUP)
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(label.clone())
        .flex_none()
        .size(control::XS)
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SM)
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .focus_visible(move |style| style.border_color(focus))
        .tooltip(tooltip(label))
        .child(
            icon(glyph, size::ACTION_ICON, theme.text_muted)
                .group_hover(ICON_BUTTON_GROUP, move |style| style.text_color(hover_text)),
        )
}

/// The avatar of a bot where it stands as a label, such as beside a message:
/// its mascot at rest in a square of `size`, still.
pub fn avatar(bot: &Bot, size: Pixels, theme: &Theme) -> Div {
    avatar_of(&bot.avatar, size, theme)
}

/// An avatar that belongs to no bot yet, such as a choice in a picker.
pub fn avatar_of(avatar: &Avatar, size: Pixels, theme: &Theme) -> Div {
    mascot::mascot(0, avatar, Mood::Rest, false, size, theme)
}

/// Mascots move, unless the system asks for reduced motion.
fn alive(cx: &App) -> bool {
    !cx.reduce_motion()
}

fn mascot_id(bot: &Bot) -> usize {
    usize::try_from(bot.id).unwrap_or_default()
}

/// The avatar of a bot where it stands for the bot itself: its mascot at
/// rest, alive.
pub fn living_avatar(bot: &Bot, size: Pixels, theme: &Theme, cx: &App) -> Div {
    mascot::mascot(
        mascot_id(bot),
        &bot.avatar,
        Mood::Rest,
        alive(cx),
        size,
        theme,
    )
}

/// The avatar of a bot with the eyes of what the bot does now, alive.
pub fn live_avatar(summary: &BotSummary, size: Pixels, theme: &Theme, cx: &App) -> Div {
    mascot::mascot(
        mascot_id(&summary.bot),
        &summary.bot.avatar,
        summary.state.into(),
        alive(cx),
        size,
        theme,
    )
}

fn badge_shell(ring: impl Into<Hsla>) -> Div {
    div()
        .absolute()
        .right(-size::BADGE_OFFSET_X)
        .bottom(-size::BADGE_OFFSET_Y)
        .rounded_full()
        .border_color(ring)
        .border(size::AVATAR_RING)
        .flex()
        .items_center()
        .justify_center()
}

/// The badge at the lower right of an avatar, for a state that asks for the
/// user: a request, a failure, or new messages.
pub fn state_badge(
    summary: &BotSummary,
    ring: impl Into<Hsla>,
    theme: &Theme,
) -> Option<AnyElement> {
    let shell = badge_shell(ring);
    Some(match summary.state {
        // The mascot of a bot that works shows it: its dots hop.
        BotState::Idle | BotState::Working => return None,
        BotState::Waiting => shell
            .size(size::BADGE_PILL)
            .bg(theme.warning)
            .child(icon(Icon::ShieldCheck, icon_size::XS, theme.on_status))
            .into_any_element(),
        BotState::Failed => shell
            .size(size::BADGE_PILL)
            .bg(theme.danger)
            .child(icon(Icon::X, size::BADGE_ICON, theme.on_accent))
            .into_any_element(),
        BotState::Unread => shell
            .min_w(size::BADGE_PILL)
            .h(size::BADGE_PILL)
            .px(space::S1)
            .bg(theme.unread_surface)
            .text_color(theme.unread_text)
            .text_size(text::SM.size)
            .line_height(text::SM.leading)
            .font_weight(FontWeight::SEMIBOLD)
            .child(SharedString::from(summary.unread.to_string()))
            .into_any_element(),
    })
}

pub fn wait_chip(theme: &Theme) -> Div {
    div()
        .flex_none()
        .h(size::CHIP_HEIGHT)
        .pl(size::CHIP_PADDING_START)
        .pr(size::CHIP_PADDING_END)
        .flex()
        .items_center()
        .gap(size::CHIP_GAP)
        .rounded_full()
        .bg(theme.warning_soft)
        .text_color(theme.warning_text)
        .text_size(text::SM.size)
        .line_height(text::SM.leading)
        .font_weight(FontWeight::SEMIBOLD)
        .child(icon(Icon::ShieldCheck, size::CHIP_ICON, theme.warning_text))
        .child("Review")
}

pub fn count_badge(count: usize, fill: impl Into<gpui::Fill>, color: impl Into<Hsla>) -> Div {
    div()
        .flex_none()
        .min_w(size::COUNT_BADGE)
        .h(size::COUNT_BADGE)
        .px(space::S1)
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(fill)
        .text_color(color)
        .text_size(text::SM.size)
        .line_height(text::SM.leading)
        .font_weight(FontWeight::SEMIBOLD)
        .child(SharedString::from(count.to_string()))
}

/// The surface of a menu, a select popup, or a picker: one step above the
/// panel, a hairline edge, and the panel shadow. A wide picker takes the
/// larger radius, so its rows and inner panels stay concentric.
pub fn menu_panel(wide: bool, theme: &Theme) -> Div {
    let (corner, padding) = if wide {
        (menu::WIDE_RADIUS, menu::WIDE_PADDING)
    } else {
        (menu::RADIUS, menu::PADDING)
    };
    div()
        .flex()
        .flex_col()
        .p(padding)
        .rounded(corner)
        .border_1()
        .border_color(theme.dialog_ring)
        .bg(theme.menu_surface)
        .shadow(theme.menu_shadow())
        .text_color(theme.text)
        .text_size(text::LG.size)
        .line_height(text::LG.leading)
}

/// A row of a menu: 28 tall, inset 8, its corner concentric in the panel,
/// with no fill of its own.
pub fn menu_row_shape(id: impl Into<ElementId>, wide: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(menu::ROW_HEIGHT)
        .px(menu::ROW_INSET)
        .flex()
        .items_center()
        .gap(menu::ROW_GAP)
        .rounded(if wide {
            menu::WIDE_ROW_RADIUS
        } else {
            menu::ROW_RADIUS
        })
        .cursor_pointer()
}

/// A row of a list in a menu. The chosen row keeps its tint; every other
/// row takes the highlight under the pointer.
pub fn menu_row(
    id: impl Into<ElementId>,
    selected: bool,
    wide: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let highlight = theme.menu_highlight;
    menu_row_shape(id, wide)
        .when(selected, |row| row.bg(theme.menu_selected))
        .when(!selected, |row| row.hover(move |style| style.bg(highlight)))
}

/// The mark of the chosen row, at the trailing edge.
pub fn menu_check(theme: &Theme) -> Svg {
    icon(Icon::Check, menu::CHECK, theme.text)
}

pub fn menu_separator(theme: &Theme) -> Div {
    div()
        .flex_none()
        .h(size::HAIRLINE)
        .mx(menu::ROW_INSET)
        .my(menu::SEPARATOR_MARGIN)
        .bg(theme.menu_separator)
}

/// A menu fades in while it drops into place, once, when it opens. Reduced
/// motion shows it at rest.
pub fn menu_reveal(id: impl Into<ElementId>, panel: Stateful<Div>, cx: &App) -> AnyElement {
    if cx.reduce_motion() {
        return panel.into_any_element();
    }
    panel
        .with_animation(
            id,
            Animation::new(duration::MENU_REVEAL).with_easing(gpui::ease_out_quint()),
            |panel, delta| panel.opacity(delta).mt(-menu::REVEAL_DROP * (1. - delta)),
        )
        .into_any_element()
}

/// A text field: a hairline edge on the control fill. Focus turns the edge to
/// the neutral focus color, with no ring; an error turns it to danger.
pub fn input_skin<E: Styled>(field: E, focused: bool, invalid: bool, theme: &Theme) -> E {
    let field = field.rounded(radius::MD).border_1().bg(theme.control);
    if invalid {
        field.border_color(theme.danger)
    } else if focused {
        field.border_color(theme.border_focus)
    } else {
        field.border_color(theme.input_edge)
    }
}

pub fn divider(theme: &Theme) -> Div {
    div()
        .flex_none()
        .w(size::HAIRLINE)
        .h(size::DIVIDER_HEIGHT)
        .bg(theme.border_strong)
}
