use crowbot_protocol::ToolKind;
use gpui::{
    Animation, AnimationExt, AnyElement, Context, FontWeight, Role, SharedString, div, prelude::*,
    pulsating_between,
};

use super::{OpenConversation, Workspace};
use crate::assets::Icon;
use crate::theme::{
    Theme, control, duration, font, icon as icon_size, layout, opacity, radius, space, text,
};
use crate::transcript::{Row, RowKind, Step, StepKind, StepState};
use crate::ui;

pub(super) fn duration_label(elapsed_ms: i64) -> String {
    let seconds = elapsed_ms.max(0) / 1000;
    match seconds {
        0 => "under 1 s".to_owned(),
        1..60 => format!("{seconds} s"),
        _ => format!("{} min {:02} s", seconds / 60, seconds % 60),
    }
}

fn step_icon(kind: StepKind) -> Icon {
    match kind {
        StepKind::Thinking | StepKind::Tool(Some(ToolKind::Think)) => Icon::Lightbulb,
        StepKind::Tool(Some(ToolKind::Read)) => Icon::FileText,
        StepKind::Tool(Some(ToolKind::Edit | ToolKind::Move)) => Icon::Pencil,
        StepKind::Tool(Some(ToolKind::Delete)) => Icon::Trash,
        StepKind::Tool(Some(ToolKind::Search)) => Icon::Search,
        StepKind::Tool(Some(ToolKind::Execute)) => Icon::Terminal,
        StepKind::Tool(Some(ToolKind::Fetch)) => Icon::Globe,
        StepKind::Tool(Some(ToolKind::Other) | None) => Icon::Wrench,
    }
}

impl Workspace {
    pub(super) fn render_steps(
        &self,
        open: &OpenConversation,
        row: &Row,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let RowKind::Steps {
            steps,
            live,
            started_ms,
            ended_ms,
        } = &row.kind
        else {
            return div().into_any_element();
        };
        let key = row.key;
        let shown = *live != open.toggled_steps.contains(&key);
        let failed = steps
            .iter()
            .filter(|step| step.state == StepState::Failed)
            .count();
        let count = match steps.len() {
            1 => "1 step".to_owned(),
            many => format!("{many} steps"),
        };
        let took = ended_ms.map(|ended| duration_label(ended - started_ms));
        let mut spoken = count.clone();
        if failed > 0 {
            spoken.push_str(&format!(", {failed} failed"));
        }
        if let Some(took) = &took {
            spoken.push_str(&format!(", {took}"));
        }
        let (hover, focus) = (theme.hover, theme.border_focus);
        let header = div()
            .id("steps")
            .debug_selector(move || format!("steps-{key}"))
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(SharedString::from(spoken))
            .h(control::XS)
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
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_steps(key, cx)))
            .child(ui::icon(
                if shown {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                icon_size::XS,
                theme.text_muted,
            ))
            .child(
                div()
                    .debug_selector(move || format!("steps-count-{key}"))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_secondary)
                    .child(SharedString::from(count)),
            )
            .children((failed > 0).then(|| {
                div()
                    .debug_selector(move || format!("steps-failed-{key}"))
                    .px(space::S1_5)
                    .rounded_full()
                    .bg(theme.danger_soft)
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.danger_text)
                    .child(SharedString::from(format!("{failed} failed")))
            }))
            .children(took.map(|took| {
                div()
                    .debug_selector(move || format!("steps-duration-{key}"))
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(took))
            }))
            .children(live.then(|| ui::spinner("steps-spinner", icon_size::XS, theme.text_muted)));
        let list = shown.then(|| {
            let newest = steps.len().saturating_sub(1);
            let lines = steps.iter().enumerate().map(|(index, step)| {
                self.render_step(open, key, index, step, *live && index == newest, theme, cx)
            });
            let list = div()
                .id("step-list")
                .debug_selector(move || format!("step-list-{key}"))
                .role(Role::List)
                .aria_label("Steps of this turn")
                .max_h(layout::STEPS_MAX_HEIGHT)
                .overflow_y_scroll()
                .pl(space::S1)
                .pr(space::S2)
                .flex()
                .flex_col()
                .gap(space::S1_5)
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .children(lines.collect::<Vec<_>>());
            match open.step_lists.get(&key) {
                Some(scroll) => list.track_scroll(scroll),
                None => list,
            }
        });
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(space::S1)
            .child(header)
            .children(list.map(|list| div().w_full().child(list)))
            .into_any_element()
    }

    #[expect(clippy::too_many_arguments)]
    fn render_step(
        &self,
        open: &OpenConversation,
        key: i64,
        index: usize,
        step: &Step,
        shining: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let thinking = step.kind == StepKind::Thinking;
        let glyph = ui::icon(step_icon(step.kind), icon_size::XS, theme.text_muted);
        let (state, color) = match step.state {
            StepState::Running | StepState::Done => (None, theme.text_muted),
            StepState::Failed => (Some("Failed"), theme.danger_text),
            StepState::Interrupted => (Some("Interrupted"), theme.warning_text),
        };
        let output = step.output.as_ref().filter(|output| !output.is_empty());
        let failed = step.state == StepState::Failed;
        let revealed = open.shown_outputs.contains(&(key, index));
        let toggle = output.map(|_| {
            let label = match (revealed, failed) {
                (false, true) => "Show error",
                (true, true) => "Hide error",
                (false, false) => "Show output",
                (true, false) => "Hide output",
            };
            let (hover, focus) = (theme.hover, theme.border_focus);
            div()
                .id(("step-output-toggle", index))
                .debug_selector(move || format!("step-output-toggle-{key}-{index}"))
                .focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(label)
                .flex_none()
                .px(space::S1_5)
                .rounded(radius::SM)
                .border_1()
                .border_color(gpui::transparent_black())
                .text_color(theme.text_secondary)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .focus_visible(move |style| style.border_color(focus))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_output(key, index, cx)))
                .child(label)
        });
        let title = if thinking {
            SharedString::from(step.title.trim().to_owned())
        } else {
            SharedString::from(step.title.clone())
        };
        let spoken = match state {
            Some(state) => format!("{title}, {state}"),
            None => title.to_string(),
        };
        let head = div()
            .min_h(layout::STEP_ROW)
            .flex()
            .items_start()
            .gap(space::S1_5)
            .child(div().flex_none().pt(space::S0_5).child(glyph))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(!thinking, |title| title.truncate())
                    .text_color(if thinking {
                        theme.text_muted
                    } else {
                        theme.text_secondary
                    })
                    .map(|row| {
                        if thinking {
                            row.child(title)
                        } else {
                            row.child(crate::truncated::truncated(title))
                        }
                    }),
            )
            .children(state.map(|state| {
                div()
                    .debug_selector(move || format!("step-state-{key}-{index}"))
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .child(state)
            }))
            .children(toggle);
        let input = step.input.as_ref().filter(|input| !input.is_empty());
        let line = div()
            .id(("step", index))
            .debug_selector(move || {
                if shining {
                    format!("step-shining-{key}-{index}")
                } else {
                    format!("step-{key}-{index}")
                }
            })
            .role(Role::ListItem)
            .aria_label(SharedString::from(spoken))
            .flex()
            .flex_col()
            .gap(space::S0_5)
            .child(head)
            .children(input.map(|input| {
                div()
                    .debug_selector(move || format!("step-input-{key}-{index}"))
                    .pl(space::S4)
                    .truncate()
                    .font(font::mono())
                    .text_color(theme.text_muted)
                    .child(crate::truncated::truncated(
                        input.lines().next().unwrap_or_default().to_owned(),
                    ))
            }))
            .children(step.allowed_by_level.then(|| {
                div()
                    .debug_selector(move || format!("step-allowed-{key}-{index}"))
                    .pl(space::S4)
                    .flex()
                    .items_center()
                    .gap(space::S1)
                    .text_color(theme.text_muted)
                    .child(ui::icon(Icon::ShieldCheck, icon_size::XS, theme.text_muted))
                    .child("Allowed by Full access")
            }))
            .children(output.filter(|_| revealed).map(|output| {
                div()
                    .id(("step-output", index))
                    .debug_selector(move || format!("step-output-{key}-{index}"))
                    .ml(space::S4)
                    .max_h(layout::STEP_ERROR_MAX_HEIGHT)
                    .overflow_y_scroll()
                    .p(space::S2)
                    .rounded(radius::MD)
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.border_strong)
                    .font(font::mono())
                    .text_size(text::CODE.size)
                    .line_height(text::CODE.leading)
                    .text_color(if failed {
                        theme.danger_text
                    } else {
                        theme.text_secondary
                    })
                    .child(SharedString::from(output.clone()))
            }));
        if shining {
            line.with_animation(
                ("step-shine", index),
                Animation::new(duration::SHINE)
                    .repeat()
                    .with_easing(pulsating_between(opacity::SHINE_LOW, opacity::SHOWN)),
                |line, shine| line.opacity(shine),
            )
            .into_any_element()
        } else {
            line.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_duration_reads_in_seconds_then_minutes() {
        assert_eq!(duration_label(400), "under 1 s");
        assert_eq!(duration_label(12_300), "12 s");
        assert_eq!(duration_label(125_000), "2 min 05 s");
        assert_eq!(duration_label(-5), "under 1 s");
    }
}
