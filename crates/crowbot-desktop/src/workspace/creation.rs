use std::time::Instant;

use crowbot_protocol::{
    AVATAR_COLORS, Avatar, Bot, BotDraft, ClientMessage, ModelChoice, PermissionLevel,
};
use gpui::{
    AnyElement, App, Context, CursorStyle, DispatchPhase, Div, Focusable, FontWeight, Hsla,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Role,
    ScrollEvent, ScrollHandle, SharedString, Stateful, Window, WindowControlArea, anchored, canvas,
    deferred, div, linear_color_stop, linear_gradient, point, prelude::*, px,
};

use super::forms::{self, AvatarOwner, GLYPHS};
use super::{FocusRequest, Workspace};
use crate::assets::Icon;
use crate::composer::Composer;
use crate::theme::{
    Theme, control, duration, icon as icon_size, layout, menu, motion, opacity, radius, size,
    space, text,
};
use crate::ui::{self, ButtonKind};

/// The name a new bot starts with. It is a real value, not a hint: the user
/// edits it or keeps it.
pub(super) const DEFAULT_NAME: &str = "New bot";

pub(super) struct Template {
    pub name: &'static str,
    /// What the card says, in one short line.
    pub description: &'static str,
    /// What the form's purpose becomes.
    pub purpose: &'static str,
    glyph: usize,
    color: usize,
}

pub(super) const TEMPLATES: [Template; 6] = [
    Template {
        name: "Researcher",
        description: "Finds sources and reports what they say.",
        purpose: "Find sources on a question, read them, and report what they say, with links.",
        glyph: 8,
        color: 7,
    },
    Template {
        name: "Code reviewer",
        description: "Points out bugs and risks in a change.",
        purpose: "Read a change, point out bugs and risks, and propose a fix for each one.",
        glyph: 7,
        color: 10,
    },
    Template {
        name: "Writer",
        description: "Drafts and edits text in a clear voice.",
        purpose: "Draft and edit text in a clear, direct voice, and explain each edit.",
        glyph: 9,
        color: 11,
    },
    Template {
        name: "Data analyst",
        description: "Explains what files and tables show.",
        purpose: "Explore files and tables, and explain what the numbers show in plain words.",
        glyph: 10,
        color: 4,
    },
    Template {
        name: "Planner",
        description: "Breaks a goal into steps and tracks them.",
        purpose: "Break a goal into steps, keep track of them, and flag what blocks progress.",
        glyph: 6,
        color: 2,
    },
    Template {
        name: "Tutor",
        description: "Explains a topic one step at a time.",
        purpose: "Explain a topic step by step, and check understanding with short questions.",
        glyph: 1,
        color: 5,
    },
];

/// A fade at one edge of the suggestion row: shown while the row can scroll
/// past that edge, and eased in or out over `duration::FADE`.
#[derive(Default)]
struct Fade {
    on: bool,
    since: Option<Instant>,
}

impl Fade {
    fn set(&mut self, on: bool, now: Instant) {
        if self.on != on {
            self.on = on;
            self.since = Some(now);
        }
    }

    /// The opacity now, and whether it still moves.
    fn level(&self, now: Instant, still: bool) -> (f32, bool) {
        let done = if self.on {
            opacity::SHOWN
        } else {
            opacity::HIDDEN
        };
        let Some(since) = self.since.filter(|_| !still) else {
            return (done, false);
        };
        let t = (now.duration_since(since).as_secs_f32() / duration::FADE.as_secs_f32()).min(1.);
        let eased = gpui::ease_out_quint()(t);
        let level = if self.on { eased } else { 1. - eased };
        (level, t < 1.)
    }
}

/// A press on the suggestion row, which becomes a drag once it moves far
/// enough.
struct Press {
    start_x: Pixels,
    start_position: Pixels,
    last_x: Pixels,
    last_at: Instant,
    /// Pixels per millisecond, toward the end of the row.
    speed: f32,
    moved: bool,
}

/// The row of suggestions: dragged with the pointer, it glides on after the
/// release and slows down; a vertical wheel scrolls it sideways until it
/// reaches an end.
#[derive(Default)]
pub(super) struct Carousel {
    scroll: ScrollHandle,
    press: Option<Press>,
    /// The speed of a glide, and the time of its last step.
    glide: Option<(f32, Instant)>,
    /// The press that just ended was a drag: the card under it does not take
    /// the click.
    suppress_click: bool,
    back: Fade,
    forward: Fade,
}

impl Carousel {
    /// How far the row has scrolled, and how far it can.
    fn position(&self) -> (Pixels, Pixels) {
        (-self.scroll.offset().x, self.scroll.max_offset().x)
    }

    /// Moves the row, within its ends. False when it could not move.
    fn move_to(&self, position: Pixels) -> bool {
        let (current, end) = self.position();
        let next = position.clamp(px(0.), end.max(px(0.)));
        if (next - current).abs() < layout::SCROLL_STILL {
            return false;
        }
        self.scroll.set_offset(point(-next, px(0.)));
        true
    }

    fn press(&mut self, x: Pixels) {
        self.glide = None;
        self.suppress_click = false;
        self.press = Some(Press {
            start_x: x,
            start_position: self.position().0,
            last_x: x,
            last_at: Instant::now(),
            speed: 0.,
            moved: false,
        });
    }

    /// Follows the pointer. True when the row moved.
    fn drag(&mut self, x: Pixels) -> bool {
        let Some(press) = self.press.as_mut() else {
            return false;
        };
        let distance = x - press.start_x;
        if !press.moved && distance.abs() > layout::DRAG_THRESHOLD {
            press.moved = true;
        }
        if !press.moved {
            return false;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(press.last_at).as_secs_f32() * 1000.;
        let sample = (f32::from(press.last_x - x) / elapsed.max(1.))
            .clamp(-motion::MAX_SPEED, motion::MAX_SPEED);
        press.speed = press.speed * motion::SPEED_MEMORY + sample * (1. - motion::SPEED_MEMORY);
        press.last_x = x;
        press.last_at = now;
        let target = press.start_position - distance;
        self.move_to(target);
        true
    }

    /// Ends a press. A drag glides on unless motion is reduced.
    fn release(&mut self, reduce_motion: bool) -> bool {
        let Some(press) = self.press.take() else {
            return false;
        };
        self.suppress_click = press.moved;
        if press.moved && !reduce_motion && press.speed.abs() >= motion::MIN_SPEED {
            self.glide = Some((press.speed, Instant::now()));
        }
        press.moved
    }

    /// Advances a glide by the time since its last step. True while it goes on.
    fn step(&mut self, now: Instant) -> bool {
        let Some((speed, last)) = self.glide else {
            return false;
        };
        let frame = duration::FRAME.as_secs_f32();
        let elapsed = now.duration_since(last).as_secs_f32().min(frame * 2.);
        let moved = self.move_to(self.position().0 + px(speed * elapsed * 1000.));
        let speed = speed * motion::FRICTION.powf(elapsed / frame);
        if moved && speed.abs() >= motion::MIN_SPEED {
            self.glide = Some((speed, now));
            true
        } else {
            self.glide = None;
            false
        }
    }

    /// A vertical wheel moves the row sideways. False at the end it moves
    /// toward, so the page scrolls instead.
    fn wheel(&mut self, delta: Pixels) -> bool {
        let (current, end) = self.position();
        let forward = delta < px(0.);
        if (forward && current >= end - layout::SCROLL_SLACK)
            || (!forward && current <= layout::SCROLL_SLACK)
        {
            return false;
        }
        self.glide = None;
        self.move_to(current - delta);
        true
    }
}

#[derive(Default)]
pub(super) struct Creation {
    /// `None` is the automatic color of the picture.
    pub color: Option<usize>,
    /// `None` is the first letter of the name.
    pub glyph: Option<usize>,
    /// `None` is the first provider of the daemon.
    pub provider: Option<String>,
    /// `None` is the default reasoning level of the provider.
    pub effort: Option<String>,
    /// The daemon refused the name.
    pub name_error: Option<SharedString>,
    pub error: Option<SharedString>,
    pub creating: bool,
    /// The model picker is open under its row.
    pub picker: bool,
    /// The fields still hold what an earlier form left in them.
    pub stale: bool,
    /// The form opened by itself, because no bot existed.
    automatic: bool,
    suggestions: Carousel,
}

impl Workspace {
    fn new_creation(&self) -> Creation {
        Creation {
            stale: true,
            ..Creation::default()
        }
    }

    /// With no bot, the creation form is what the window shows.
    pub(super) fn creation_follows_bots(&mut self) {
        if self.bots.is_empty() && self.creation.is_none() {
            self.creation = Some(Creation {
                automatic: true,
                ..self.new_creation()
            });
            self.focus_request = Some(FocusRequest::Creation);
            self.ask_providers();
        }
    }

    /// A bot now exists: a form that opened by itself, and that nobody
    /// filled, gives way to it.
    pub(super) fn creation_yields(&mut self, cx: &Context<Self>) {
        let untouched = self.creation_name.read(cx).text().trim() == DEFAULT_NAME
            && self.creation_purpose.read(cx).text().is_empty();
        if untouched
            && self
                .creation
                .as_ref()
                .is_some_and(|creation| creation.automatic && !creation.creating)
        {
            self.creation = None;
            self.focus_request = Some(FocusRequest::Composer);
        }
    }

    pub(super) fn open_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        match self.creation.as_mut() {
            Some(creation) => creation.automatic = false,
            None => self.creation = Some(self.new_creation()),
        }
        self.ask_providers();
        window.focus(&self.creation_focus, cx);
        cx.notify();
    }

    pub(super) fn cancel_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bots.is_empty() || self.creation.take().is_none() {
            return;
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn creation_avatar(&self, cx: &App) -> Avatar {
        let (color, glyph) = self
            .creation
            .as_ref()
            .map_or((None, None), |creation| (creation.color, creation.glyph));
        let cell = glyph.map_or(0, |index| index + 1);
        Avatar {
            color: color
                .map_or_else(
                    || forms::automatic_color(cell),
                    |index| AVATAR_COLORS[index % AVATAR_COLORS.len()],
                )
                .to_owned(),
            glyph: match glyph {
                Some(index) => GLYPHS[index % GLYPHS.len()].to_owned(),
                None => forms::initial(self.creation_name.read(cx).text()),
            },
        }
    }

    /// The suggestion the form holds, as long as nothing was changed since it
    /// was picked.
    pub(super) fn chosen_template(&self, cx: &App) -> Option<usize> {
        let creation = self.creation.as_ref()?;
        let name = self.creation_name.read(cx).text();
        let purpose = self.creation_purpose.read(cx).text();
        TEMPLATES.iter().position(|template| {
            template.name == name
                && template.purpose == purpose
                && creation.glyph == Some(template.glyph)
                && creation.color == Some(template.color)
        })
    }

    fn apply_template(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(template), Some(creation)) = (TEMPLATES.get(index), self.creation.as_mut())
        else {
            return;
        };
        if creation.creating || std::mem::take(&mut creation.suggestions.suppress_click) {
            return;
        }
        creation.glyph = Some(template.glyph);
        creation.color = Some(template.color);
        creation.error = None;
        self.creation_name.update(cx, |input, cx| {
            input.set_text(template.name, window, cx);
        });
        self.creation_purpose.update(cx, |input, cx| {
            input.set_text(template.purpose, window, cx);
        });
        cx.notify();
    }

    /// The form can be sent: it has a name, and nothing is on its way.
    fn creation_ready(&self, cx: &Context<Self>) -> bool {
        self.live()
            && !self.creation_name.read(cx).text().trim().is_empty()
            && self
                .creation
                .as_ref()
                .is_some_and(|creation| !creation.creating)
    }

    pub(super) fn create_bot(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.creation_ready(cx) {
            return;
        }
        let name = self.creation_name.read(cx).text().trim().to_owned();
        let instructions = self.creation_purpose.read(cx).text().to_owned();
        let model = self.creation_model.read(cx).text().trim().to_owned();
        let avatar = self.creation_avatar(cx);
        let Some(creation) = self.creation.as_mut() else {
            return;
        };
        creation.error = None;
        creation.name_error = None;
        creation.picker = false;
        creation.creating = true;
        let (provider, effort) = (creation.provider.clone(), creation.effort.clone());
        self.connection.send(ClientMessage::CreateBot {
            draft: BotDraft {
                name,
                instructions,
                avatar: Some(avatar),
                provider,
                model: (!model.is_empty()).then_some(model),
                effort,
                ..BotDraft::default()
            },
        });
        cx.notify();
    }

    /// The provider the new bot runs on: the one picked, or the first one
    /// the daemon offers.
    pub(super) fn creation_provider(&self) -> String {
        self.creation
            .as_ref()
            .and_then(|creation| creation.provider.clone())
            .or_else(|| self.providers.first().map(|status| status.id.clone()))
            .unwrap_or_default()
    }

    /// Another provider has other models and other reasoning levels: the
    /// ones chosen so far go back to the defaults.
    fn pick_creation_provider(
        &mut self,
        provider: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.creation_provider();
        let Some(creation) = self.creation.as_mut() else {
            return;
        };
        if creation.creating || provider.as_deref() == Some(current.as_str()) {
            return;
        }
        creation.provider = provider;
        creation.effort = None;
        self.creation_model
            .update(cx, |input, cx| input.clear(window, cx));
    }

    fn pick_creation_model(
        &mut self,
        model: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = model.unwrap_or_default();
        self.creation_model
            .update(cx, |input, cx| input.set_text(&model, window, cx));
    }

    fn pick_creation_effort(
        &mut self,
        effort: Option<String>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if let Some(creation) = self.creation.as_mut() {
            creation.effort = effort;
        }
    }

    pub(super) fn preview_bot(&self, cx: &App) -> Bot {
        Bot {
            id: 0,
            name: self.creation_name.read(cx).text().trim().to_owned(),
            title: String::new(),
            instructions: String::new(),
            avatar: self.creation_avatar(cx),
            provider: String::new(),
            model: None,
            effort: None,
            permission_level: PermissionLevel::AskFirst,
            workspace: String::new(),
            pinned: false,
            hidden: false,
            created_at_ms: 0,
        }
    }

    /// Closes the model picker. False when it was not open.
    pub(super) fn close_model_picker(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(creation) = self.creation.as_mut().filter(|creation| creation.picker) else {
            return false;
        };
        creation.picker = false;
        cx.notify();
        true
    }

    fn toggle_model_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(creation) = self.creation.as_mut()
            && !creation.creating
        {
            creation.picker = !creation.picker;
            cx.notify();
        }
    }

    /// What the bot will run on, in words: the provider, the model, and the
    /// reasoning level when the provider has levels.
    fn model_summary(
        &self,
        provider: &str,
        effort: Option<&str>,
        cx: &Context<Self>,
    ) -> (SharedString, SharedString, Option<SharedString>) {
        let provider_name = self
            .providers
            .iter()
            .find(|status| status.id == provider)
            .map_or_else(|| provider.to_owned(), |status| status.name.clone());
        let typed = self.creation_model.read(cx).text().trim().to_owned();
        let models = self.models.get(provider);
        let named = |id: &str, listed: &[ModelChoice]| {
            listed
                .iter()
                .find(|choice| choice.id == id)
                .map_or_else(|| id.to_owned(), |choice| choice.name.clone())
        };
        let model = match (typed.is_empty(), models) {
            (false, Some(models)) => named(&typed, &models.models),
            (false, None) => typed,
            (true, Some(models)) => models.default_model.as_deref().map_or_else(
                || "Default model".to_owned(),
                |id| named(id, &models.models),
            ),
            (true, None) => "Default model".to_owned(),
        };
        let level = models
            .filter(|models| !models.efforts.is_empty())
            .map(|models| match effort.or(models.default_effort.as_deref()) {
                Some(id) => SharedString::from(named(id, &models.efforts)),
                None => SharedString::from("Default"),
            });
        (provider_name.into(), model.into(), level)
    }

    /// The model trigger: a form control like the fields above it, with the
    /// label on the leading edge and the choice on the trailing edge.
    fn render_model_row(
        &self,
        provider: &str,
        effort: Option<&str>,
        open: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let live = self.live();
        let (provider_name, model, level) = self.model_summary(provider, effort, cx);
        let spoken = match &level {
            Some(level) => format!("Model: {provider_name}, {model}, {level}"),
            None => format!("Model: {provider_name}, {model}"),
        };
        let focus = theme.border_focus;
        let row = div()
            .id("creation-model-row")
            .debug_selector(|| "creation-model-row".into())
            .focusable()
            .tab_stop(live)
            .role(Role::Button)
            .aria_label(SharedString::from(spoken))
            .h(layout::MODEL_ROW)
            .px(space::S3)
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded(radius::MD)
            .border_1()
            .border_color(theme.input_edge)
            .bg(theme.control)
            .when(!live, |row| row.opacity(opacity::DISABLED))
            .when(live, |row| {
                row.cursor_pointer()
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_model_picker(cx)))
            })
            .child(
                div()
                    .flex_none()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_secondary)
                    .child("Model"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .text_size(text::MD.size)
                    .line_height(text::MD.leading)
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.text_muted)
                            .child(provider_name),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(model),
                    )
                    .children(level.map(|level| {
                        div()
                            .flex_none()
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!("· {level}")))
                    })),
            )
            .child(ui::icon(Icon::ChevronDown, icon_size::SM, theme.text_muted));
        let picker = open.then(|| {
            deferred(
                anchored()
                    .offset(point(px(0.), layout::MODEL_ROW + menu::PADDING))
                    .snap_to_window_with_margin(space::S2)
                    .child({
                        let picker = self.render_model_picker(provider, effort, theme, window, cx);
                        ui::menu_reveal("creation-model-reveal", picker, cx)
                    }),
            )
            .with_priority(2)
        });
        div().flex().flex_col().child(row).children(picker)
    }

    /// A layer over the window while the picker is open: a click outside the
    /// picker closes it, and does nothing else.
    fn render_picker_backdrop(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let viewport = window.viewport_size();
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("creation-model-backdrop")
                    .debug_selector(|| "creation-model-backdrop".into())
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_model_picker(cx);
                        }),
                    ),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn render_picker_providers(
        &self,
        provider: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let rows: Vec<Stateful<Div>> = self
            .providers
            .iter()
            .enumerate()
            .map(|(index, status)| {
                let chosen = status.id == provider;
                let (state, _) = forms::provider_state(status, theme);
                let id = status.id.clone();
                ui::menu_row(("creation-provider", index), chosen, true, theme)
                    .debug_selector(move || {
                        if chosen {
                            format!("creation-provider-{index}-chosen")
                        } else {
                            format!("creation-provider-{index}")
                        }
                    })
                    .focusable()
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label(SharedString::from(if chosen {
                        format!("{}, {state}, selected", status.name)
                    } else {
                        format!("{}, {state}", status.name)
                    }))
                    .text_color(if chosen {
                        theme.text
                    } else {
                        theme.text_secondary
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_creation_provider(Some(id.clone()), window, cx);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_none()
                            .size(size::STATE_DOT)
                            .rounded_full()
                            .bg(forms::provider_color(status, theme)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(status.name.clone())),
                    )
            })
            .collect();
        div()
            .id("creation-provider")
            .debug_selector(|| "creation-provider".into())
            .role(Role::Group)
            .aria_label("Provider")
            .flex_none()
            .w(layout::PICKER_PROVIDERS)
            .flex()
            .flex_col()
            .child(ui::menu_heading("Providers", theme))
            .children(rows)
    }

    fn render_model_picker(
        &self,
        provider: &str,
        effort: Option<&str>,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let providers =
            (self.providers.len() > 1).then(|| self.render_picker_providers(provider, theme, cx));
        let typed = self.creation_model.read(cx).text().trim().to_owned();
        let models: Vec<Stateful<Div>> = self
            .model_choices(provider, (!typed.is_empty()).then_some(typed.as_str()))
            .into_iter()
            .enumerate()
            .map(|(index, choice)| {
                let forms::Choice {
                    value,
                    label,
                    chosen,
                } = choice;
                ui::menu_row(("creation-model-choice", index), chosen, true, theme)
                    .debug_selector(move || {
                        if chosen {
                            format!("creation-model-choice-{index}-chosen")
                        } else {
                            format!("creation-model-choice-{index}")
                        }
                    })
                    .focusable()
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label(SharedString::from(if chosen {
                        format!("{label}, selected")
                    } else {
                        label.to_string()
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_creation_model(value.clone(), window, cx);
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .children(chosen.then(|| ui::menu_check(theme)))
            })
            .collect();
        let levels = self.effort_choices(provider, effort);
        let reasoning = (!levels.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .child(ui::menu_separator(theme))
                .child(ui::menu_heading("Reasoning", theme))
                .child(div().px(menu::ROW_INSET).child(self.choice_chips(
                    "creation-effort",
                    "Reasoning level",
                    levels,
                    Self::pick_creation_effort,
                    theme,
                    cx,
                )))
        });
        let note = self.models_note(provider).map(|reason| {
            div().px(menu::ROW_INSET).pb(space::S1).child(forms::note(
                "creation-models-note",
                reason,
                theme.text_muted,
            ))
        });
        let other = self.input_box(
            "creation-model",
            &self.creation_model,
            false,
            theme,
            window,
            cx,
        );
        let options = div()
            .id("creation-model-options")
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(ui::menu_heading("Models", theme))
            .children(models)
            .children(note)
            .children(reasoning)
            .child(ui::menu_separator(theme))
            .child(ui::menu_heading("Other model", theme))
            .child(div().p(size::FOCUS_RING).child(other));
        let split = providers.map(|column| {
            div().flex().child(column).child(
                div()
                    .flex_none()
                    .w(size::HAIRLINE)
                    .my(menu::SEPARATOR_MARGIN)
                    .mx(menu::WIDE_PADDING)
                    .bg(theme.menu_separator),
            )
        });
        ui::menu_panel(true, theme)
            .id("creation-model-picker")
            .debug_selector(|| "creation-model-picker".into())
            .occlude()
            .role(Role::Group)
            .aria_label("Choose a model")
            .w(layout::CREATION_WIDTH)
            .max_h(menu::MAX_HEIGHT)
            .flex_row()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.close_model_picker(cx) {
                    cx.stop_propagation();
                }
            }))
            .children(split)
            .child(options)
    }

    pub(super) fn render_creation_header(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let preview = self.preview_bot(cx);
        let name: SharedString = if preview.name.is_empty() {
            "New bot".into()
        } else {
            preview.name.clone().into()
        };
        div()
            .id("creation-header")
            .debug_selector(|| "creation-header".into())
            .window_control_area(WindowControlArea::Drag)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .gap(space::S1_5)
            .pl(layout::HEADER_PADDING_START)
            .pr(space::S2)
            .child(ui::avatar(
                &preview,
                size::AVATAR_HEADER,
                size::AVATAR_HEADER_GLYPH,
                theme,
            ))
            .child(
                div()
                    .id("creation-title")
                    .role(Role::Heading)
                    .aria_label(name.clone())
                    .min_w_0()
                    .truncate()
                    .child(name),
            )
            .child(div().flex_1())
            .children((!self.bots.is_empty()).then(|| {
                div().occlude().child(
                    ui::button("cancel-creation", "Cancel", ButtonKind::Ghost, theme)
                        .h(control::SM)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_creation(window, cx)),
                        ),
                )
            }))
    }

    fn press_suggestions(&mut self, x: Pixels) {
        if let Some(creation) = self.creation.as_mut()
            && !creation.creating
        {
            creation.suggestions.press(x);
        }
    }

    fn drag_suggestions(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if let Some(creation) = self.creation.as_mut()
            && creation.suggestions.drag(x)
        {
            cx.notify();
        }
    }

    fn release_suggestions(&mut self, cx: &mut Context<Self>) {
        let reduce_motion = cx.reduce_motion();
        if let Some(creation) = self.creation.as_mut()
            && creation.suggestions.release(reduce_motion)
        {
            cx.notify();
        }
    }

    /// Moves a glide on by one frame, and eases the edge fades toward what
    /// the row can scroll to. Asks for the next frame while either moves.
    fn animate_suggestions(&mut self, window: &mut Window, cx: &Context<Self>) -> (f32, f32) {
        let still = cx.reduce_motion();
        let Some(creation) = self.creation.as_mut() else {
            return (opacity::HIDDEN, opacity::HIDDEN);
        };
        let now = Instant::now();
        let carousel = &mut creation.suggestions;
        let gliding = carousel.step(now);
        let (position, end) = carousel.position();
        carousel.back.set(position > layout::SCROLL_SLACK, now);
        carousel
            .forward
            .set(position < end - layout::SCROLL_SLACK, now);
        let (back, back_moving) = carousel.back.level(now, still);
        let (forward, forward_moving) = carousel.forward.level(now, still);
        if gliding || back_moving || forward_moving {
            window.request_animation_frame();
        }
        (back, forward)
    }

    /// The row of suggestions. A card fills the form with its bot; the row
    /// scrolls by drag, by wheel, and by keyboard focus.
    fn render_templates(
        &mut self,
        selected: Option<usize>,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let (fade_back, fade_forward) = self.animate_suggestions(window, cx);
        let Some(creation) = self.creation.as_ref() else {
            return div();
        };
        let carousel = &creation.suggestions;
        let dragging = carousel.press.as_ref().is_some_and(|press| press.moved);
        let scroll = carousel.scroll.clone();
        let (edge, edge_hover, hover, focus, accent) = (
            theme.border,
            theme.border_strong,
            theme.hover,
            theme.border_focus,
            theme.accent,
        );
        let cursor = if dragging {
            CursorStyle::ClosedHand
        } else {
            CursorStyle::OpenHand
        };
        let cards = TEMPLATES.iter().enumerate().map(|(index, template)| {
            let chosen = selected == Some(index);
            let sample = Avatar {
                color: AVATAR_COLORS[template.color].to_owned(),
                glyph: GLYPHS[template.glyph].to_owned(),
            };
            div()
                .id(("template", index))
                .debug_selector(move || format!("template-{index}"))
                .focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(SharedString::from(format!(
                    "{}: {}",
                    template.name, template.description
                )))
                .flex_none()
                .w(layout::TEMPLATE_WIDTH)
                .h(layout::TEMPLATE_HEIGHT)
                .p(space::S4)
                .flex()
                .items_center()
                .gap(space::S4)
                .rounded(radius::LG)
                .border_1()
                .border_color(if chosen { accent } else { edge })
                .when(chosen, |card| card.bg(theme.accent_soft))
                .cursor(cursor)
                .when(!chosen && !dragging, |card| {
                    card.hover(move |style| style.border_color(edge_hover).bg(hover))
                })
                .focus_visible(move |style| style.border_color(focus))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_template(index, window, cx);
                }))
                .child(ui::avatar_of(
                    &sample,
                    size::AVATAR_TEMPLATE,
                    size::AVATAR_TEMPLATE_GLYPH,
                    theme,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::S1)
                        .child(
                            div()
                                .line_clamp(2)
                                .text_ellipsis()
                                .text_size(text::XL.size)
                                .line_height(text::XL.leading)
                                .font_weight(FontWeight::MEDIUM)
                                .child(template.name),
                        )
                        .child(
                            div()
                                .line_clamp(3)
                                .text_ellipsis()
                                .text_size(text::MD.size)
                                .line_height(text::MD.leading)
                                .text_color(theme.text_muted)
                                .child(template.description),
                        ),
                )
        });
        let canvas_color = Hsla::from(theme.canvas);
        let fade = |level: f32, leading: bool| {
            let (from, to) = if leading {
                (canvas_color, canvas_color.opacity(opacity::HIDDEN))
            } else {
                (canvas_color.opacity(opacity::HIDDEN), canvas_color)
            };
            div()
                .absolute()
                .top_0()
                .bottom(space::S1)
                .when(leading, |fade| fade.left_0())
                .when(!leading, |fade| fade.right_0())
                .w(layout::TEMPLATE_FADE)
                .opacity(level)
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(from, 0.),
                    linear_color_stop(to, 1.),
                ))
        };
        // A press anywhere on the row may turn into a drag: from then on the
        // pointer moves the row wherever it goes, until it is released.
        let entity = cx.entity().downgrade();
        let pointer = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moves = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        moves
                            .update(cx, |this, cx| this.drag_suggestions(event.position.x, cx))
                            .ok();
                    }
                });
                let releases = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        releases
                            .update(cx, |this, cx| this.release_suggestions(cx))
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        div()
            .flex_none()
            .w_full()
            .px(space::S6)
            .pb(space::S4)
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(
                div()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child("Suggestions"),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(
                        div()
                            .id("templates")
                            .debug_selector(|| "templates".into())
                            .role(Role::Group)
                            .aria_label("Suggestions")
                            .w_full()
                            .overflow_x_scroll()
                            .restrict_scroll_to_axis()
                            .track_scroll(&scroll)
                            .cursor(cursor)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, _| {
                                    this.press_suggestions(event.position.x);
                                }),
                            )
                            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                                let delta = event.delta.pixel_delta(window.line_height());
                                if event.modifiers.control || delta.y.abs() <= delta.x.abs() {
                                    return;
                                }
                                if let Some(creation) = this.creation.as_mut()
                                    && creation.suggestions.wheel(delta.y)
                                {
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
                            .flex()
                            .gap(space::S3)
                            .pt(size::HAIRLINE)
                            .px(size::HAIRLINE)
                            .pb(space::S1)
                            .children(cards),
                    )
                    .child(fade(fade_back, true))
                    .child(fade(fade_forward, false))
                    .child(pointer),
            )
    }

    /// A field of the form: one line of text on the control fill, 32 tall.
    /// An area restyles its padding and alignment.
    fn form_box(
        &self,
        id: &'static str,
        input: &gpui::Entity<Composer>,
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
                .py(layout::FORM_FIELD_PADDING_Y)
                .flex()
                .items_center(),
            focused,
            invalid,
            theme,
        )
        .on_click(move |_, window, cx| window.focus(&handle, cx))
        .child(div().flex_1().min_w_0().child(input.clone()))
    }

    pub(super) fn render_creation(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(creation) = self.creation.as_mut() else {
            return div().into_any_element();
        };
        if std::mem::take(&mut creation.stale) {
            for input in [&self.creation_purpose, &self.creation_model] {
                input.update(cx, |input, cx| input.clear(window, cx));
            }
            self.creation_name.update(cx, |input, cx| {
                input.set_text_selected(DEFAULT_NAME, window, cx)
            });
        }
        let Some(creation) = self.creation.as_ref() else {
            return div().into_any_element();
        };
        let (creating, automatic_color, picker) =
            (creation.creating, creation.color.is_none(), creation.picker);
        let effort = creation.effort.clone();
        let (name_error, error) = (creation.name_error.clone(), creation.error.clone());
        let first = self.bots.is_empty();
        let preview = self.preview_bot(cx);
        let ready = self.creation_ready(cx);
        let template = self.chosen_template(cx);

        let name = forms::labeled(
            forms::required_label("Name", theme),
            self.form_box(
                "creation-name",
                &self.creation_name,
                name_error.is_some(),
                theme,
                window,
                cx,
            ),
        )
        .children(
            name_error.map(|reason| forms::note("creation-name-error", reason, theme.danger_text)),
        );
        let purpose = forms::field(
            "Purpose",
            self.form_box(
                "creation-purpose",
                &self.creation_purpose,
                false,
                theme,
                window,
                cx,
            )
            .py(space::S2)
            .items_start(),
            theme,
        );
        let provider = self.creation_provider();
        let model = self.render_model_row(&provider, effort.as_deref(), picker, theme, window, cx);
        let notice = self
            .providers
            .iter()
            .find(|status| status.id == provider)
            .and_then(|status| self.render_provider_notice(status, "creation", theme, cx));

        let create = if creating {
            div()
                .id("create-bot")
                .debug_selector(|| "creating-bot".into())
                .role(Role::Status)
                .aria_label("Creating the bot")
                .h(control::LG)
                .min_w(layout::CREATE_MIN_WIDTH)
                .px(space::S4)
                .flex()
                .items_center()
                .justify_center()
                .gap(space::S2)
                .rounded(radius::MD)
                .bg(theme.inverse_surface)
                .opacity(opacity::DISABLED)
                .text_color(theme.inverse_text)
                .text_size(text::LG.size)
                .line_height(text::LG.leading)
                .font_weight(FontWeight::MEDIUM)
                .child(ui::spinner(
                    "creating-spinner",
                    icon_size::SM,
                    theme.inverse_text,
                ))
                .child("Creating…")
        } else {
            ui::button(
                "create-bot",
                "Create",
                if ready {
                    ButtonKind::Primary
                } else {
                    ButtonKind::PrimaryDisabled
                },
                theme,
            )
            .h(control::LG)
            .min_w(layout::CREATE_MIN_WIDTH)
            .px(space::S4)
            .rounded(radius::MD)
            .text_size(text::LG.size)
            .line_height(text::LG.leading)
            .when(ready, |button| {
                button.on_click(cx.listener(|this, _, window, cx| this.create_bot(window, cx)))
            })
        };

        let form = div()
            .w(layout::CREATION_WIDTH)
            .max_w_full()
            .py(space::S6)
            .flex()
            .flex_col()
            .gap(space::S3)
            .child(
                div().mb(space::S1).flex().justify_center().child(
                    ui::avatar(
                        &preview,
                        size::AVATAR_CREATION,
                        size::AVATAR_CREATION_GLYPH,
                        theme,
                    )
                    .relative()
                    .top(-size::AVATAR_CREATION_LIFT)
                    .debug_selector(|| "creation-avatar".into()),
                ),
            )
            .child(
                div()
                    .id("creation-colors")
                    .role(Role::Group)
                    .aria_label("Avatar color")
                    .child(self.render_color_picker(
                        AvatarOwner::Creation,
                        &preview.avatar,
                        automatic_color,
                        theme,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("creation-glyphs")
                    .role(Role::Group)
                    .aria_label("Avatar picture")
                    .child(self.render_glyph_picker(
                        AvatarOwner::Creation,
                        &forms::initial(&preview.name),
                        &preview.avatar,
                        theme,
                        cx,
                    )),
            )
            .child(
                div()
                    .mt(space::S1)
                    .flex()
                    .flex_col()
                    .gap(space::S3)
                    .child(name)
                    .child(purpose)
                    .child(model)
                    .children(notice),
            )
            .children(error.map(|reason| {
                div()
                    .debug_selector(|| "creation-error".into())
                    .text_center()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.danger_text)
                    .child(reason)
            }))
            .child(div().mt(space::S1).flex().justify_center().child(create));

        div()
            .id("creation")
            .debug_selector(|| "creation".into())
            .track_focus(&self.creation_focus)
            .role(Role::Group)
            .aria_label(if first {
                "Create your first bot"
            } else {
                "Create a bot"
            })
            .size_full()
            .overflow_y_scroll()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    if !this.close_model_picker(cx) {
                        this.cancel_creation(window, cx);
                    }
                }
            }))
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .px(layout::CREATION_MARGIN)
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .child(form),
                    )
                    .child(self.render_templates(template, theme, window, cx)),
            )
            .children(picker.then(|| self.render_picker_backdrop(window, cx)))
            .into_any_element()
    }
}
