use crowbot_protocol::{Bot, BotState, BotSummary};
use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, Context, Div, ElementId, FontWeight, Hsla,
    Pixels, Role, SharedString, Stateful, Svg, Transformation, Window, div, percentage, prelude::*,
    svg,
};

use crate::assets::Icon;
use crate::bots;
use crate::theme::{
    Theme, control, duration, font, icon as icon_size, layout, radius, size, space, text,
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
    Ghost,
    Danger,
    Disabled,
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
        .tab_stop(kind != ButtonKind::Disabled)
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
            let hover = theme.text_secondary;
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
    }
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

pub fn avatar(bot: &Bot, diameter: Pixels, glyph: Pixels, theme: &Theme) -> Div {
    div()
        .flex_none()
        .size(diameter)
        .rounded_full()
        .bg(bots::avatar_color(&bot.avatar.color).unwrap_or(theme.accent))
        .flex()
        .items_center()
        .justify_center()
        .text_size(glyph)
        .line_height(diameter)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.on_accent)
        .child(SharedString::from(bot.avatar.glyph.clone()))
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

fn hop(phase: f32) -> f32 {
    let rise = duration::WORKING_RISE;
    if phase < rise {
        (phase / rise * std::f32::consts::PI).sin()
    } else {
        0.
    }
}

pub fn state_badge(
    summary: &BotSummary,
    ring: impl Into<Hsla>,
    theme: &Theme,
) -> Option<AnyElement> {
    let id = usize::try_from(summary.bot.id).unwrap_or_default();
    let shell = badge_shell(ring);
    Some(match summary.state {
        BotState::Idle => return None,
        BotState::Working => {
            let dot_color = theme.text_secondary;
            shell
                .w(size::BADGE_WORKING_WIDTH)
                .h(size::BADGE)
                .gap(space::S0_5)
                .bg(theme.surface)
                .children((0..3_usize).map(|dot| {
                    div()
                        .flex_none()
                        .size(size::BADGE_DOT)
                        .rounded_full()
                        .bg(dot_color)
                        .relative()
                        .with_animation(
                            ("working", id * 3 + dot),
                            Animation::new(duration::WORKING).repeat(),
                            move |ball, turn| {
                                let phase =
                                    (turn + 1. - dot as f32 * duration::WORKING_STAGGER).fract();
                                ball.top(-size::BADGE_HOP * hop(phase))
                            },
                        )
                }))
                .into_any_element()
        }
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

pub fn divider(theme: &Theme) -> Div {
    div()
        .flex_none()
        .w(size::HAIRLINE)
        .h(size::DIVIDER_HEIGHT)
        .bg(theme.border_strong)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_working_dot_rises_once_per_cycle_and_rests() {
        assert_eq!(hop(0.), 0.);
        assert!((hop(0.2) - 1.).abs() < 0.001);
        assert_eq!(hop(0.4), 0.);
        assert_eq!(hop(0.9), 0.);
    }
}
