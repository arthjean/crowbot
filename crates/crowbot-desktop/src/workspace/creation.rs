use std::cell::Cell;
use std::f32::consts::PI;
use std::rc::Rc;
use std::time::Instant;

use crowbot_protocol::{
    AVATAR_COLORS, Avatar, Bot, BotDraft, ClientMessage, MascotShape, ModelChoice, ModelsState,
    PermissionLevel, ProviderState, mascot_at,
};
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, Div, Focusable,
    FontWeight, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Role, ScrollEvent, ScrollHandle, SharedString, Stateful, Transformation, Window,
    anchored, canvas, deferred, div, linear_color_stop, linear_gradient, point, prelude::*, px,
    radians,
};

use super::forms::{self, AvatarOwner};
use super::rail::{provider_glyph, provider_logo};
use super::settings::Section;
use super::{FocusRequest, Workspace};
use crate::assets::Icon;
use crate::chrome::WindowDrag;
use crate::composer::Composer;
use crate::theme::{
    Theme, control, duration, icon as icon_size, layout, menu, motion, opacity, picker, radius,
    size, space, text,
};
use crate::ui::{self, ButtonKind};

/// The name a new bot starts with. It is a real value, not a hint: the user
/// edits it or keeps it.
pub(super) const DEFAULT_NAME: &str = "New bot";

/// One row of the model list of a picker tab.
struct ModelRow {
    /// What the row stores: `None` is the default model of the provider.
    value: Option<String>,
    name: SharedString,
    /// The provider runs this model when the bot names none.
    default: bool,
    chosen: bool,
}

/// A small muted pill beside a name.
fn badge(label: &'static str, theme: &Theme) -> Div {
    div()
        .flex_none()
        .px(picker::BADGE_PADDING_X)
        .py(picker::BADGE_PADDING_Y)
        .rounded_full()
        .bg(theme.badge)
        .text_size(text::SM.size)
        .line_height(picker::BADGE_LEADING)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(label)
}

pub(super) struct Template {
    pub name: &'static str,
    /// What the card says, in one short line.
    pub description: &'static str,
    /// What the form's purpose becomes.
    pub purpose: &'static str,
    shape: MascotShape,
    color: usize,
}

pub(super) const TEMPLATES: [Template; 6] = [
    Template {
        name: "Researcher",
        description: "Finds sources and reports what they say.",
        purpose: "Find sources on a question, read them, and report what they say, with links.",
        shape: MascotShape::Plume,
        color: 7,
    },
    Template {
        name: "Code reviewer",
        description: "Points out bugs and risks in a change.",
        purpose: "Read a change, point out bugs and risks, and propose a fix for each one.",
        shape: MascotShape::Spikes,
        color: 10,
    },
    Template {
        name: "Writer",
        description: "Drafts and edits text in a clear voice.",
        purpose: "Draft and edit text in a clear, direct voice, and explain each edit.",
        shape: MascotShape::Curl,
        color: 11,
    },
    Template {
        name: "Data analyst",
        description: "Explains what files and tables show.",
        purpose: "Explore files and tables, and explain what the numbers show in plain words.",
        shape: MascotShape::Flat,
        color: 4,
    },
    Template {
        name: "Planner",
        description: "Breaks a goal into steps and tracks them.",
        purpose: "Break a goal into steps, keep track of them, and flag what blocks progress.",
        shape: MascotShape::Crest,
        color: 2,
    },
    Template {
        name: "Tutor",
        description: "Explains a topic one step at a time.",
        purpose: "Explain a topic step by step, and check understanding with short questions.",
        shape: MascotShape::Smooth,
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
    /// `None` is the automatic color of the head.
    pub color: Option<usize>,
    /// `None` is the head of a bot made after the ones that exist:
    /// `mascot_at`.
    pub shape: Option<MascotShape>,
    /// `None` is the first provider of the daemon.
    pub provider: Option<String>,
    /// `None` is the default reasoning level of the provider.
    pub effort: Option<String>,
    /// The daemon refused the name.
    pub name_error: Option<SharedString>,
    pub error: Option<SharedString>,
    pub creating: bool,
    /// `None` is the default model of the provider.
    pub model: Option<String>,
    /// The model picker is open beside its row.
    pub picker: bool,
    /// The provider tab the picker shows; `None` is the bot's provider.
    tab: Option<String>,
    /// The reasoning menu of the picker is open.
    effort_menu: bool,
    /// A reasoning level chosen on another tab, kept for the model picked
    /// there next.
    queued_effort: Option<(String, Option<String>)>,
    /// Where the model row and the reasoning select were last drawn, so the
    /// picker and its menu open beside them.
    row_bounds: Rc<Cell<Bounds<Pixels>>>,
    effort_bounds: Rc<Cell<Bounds<Pixels>>>,
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

    fn creation_avatar(&self) -> Avatar {
        let (color, shape) = self
            .creation
            .as_ref()
            .map_or((None, None), |creation| (creation.color, creation.shape));
        let shape = shape.unwrap_or_else(|| mascot_at(self.bots.len() as u64));
        Avatar {
            color: color
                .map_or_else(
                    || forms::automatic_color(shape),
                    |index| AVATAR_COLORS[index % AVATAR_COLORS.len()],
                )
                .to_owned(),
            shape,
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
                && creation.shape == Some(template.shape)
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
        creation.shape = Some(template.shape);
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
        let avatar = self.creation_avatar();
        let Some(creation) = self.creation.as_mut() else {
            return;
        };
        creation.error = None;
        creation.name_error = None;
        creation.picker = false;
        creation.creating = true;
        let (provider, model, effort) = (
            creation.provider.clone(),
            creation.model.clone(),
            creation.effort.clone(),
        );
        self.connection.send(ClientMessage::CreateBot {
            draft: BotDraft {
                name,
                instructions,
                avatar: Some(avatar),
                provider,
                model,
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

    pub(super) fn preview_bot(&self, cx: &App) -> Bot {
        Bot {
            id: 0,
            name: self.creation_name.read(cx).text().trim().to_owned(),
            title: String::new(),
            instructions: String::new(),
            avatar: self.creation_avatar(),
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

    /// Closes the reasoning menu when it is open, and the model picker
    /// otherwise. False when neither was open.
    pub(super) fn close_model_picker(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(creation) = self.creation.as_mut().filter(|creation| creation.picker) else {
            return false;
        };
        if !std::mem::take(&mut creation.effort_menu) {
            creation.picker = false;
        }
        cx.notify();
        true
    }

    /// Opens the picker on the tab of the chosen provider, with an empty
    /// search, or closes it.
    fn toggle_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(creation) = self.creation.as_mut().filter(|creation| !creation.creating) else {
            return;
        };
        creation.picker = !creation.picker;
        creation.tab = None;
        creation.effort_menu = false;
        creation.queued_effort = None;
        self.creation_model
            .update(cx, |input, cx| input.clear(window, cx));
        cx.notify();
    }

    /// The provider whose models the picker shows.
    pub(super) fn picker_tab(&self) -> String {
        self.creation
            .as_ref()
            .and_then(|creation| creation.tab.clone())
            .unwrap_or_else(|| self.creation_provider())
    }

    fn show_picker_tab(&mut self, tab: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(creation) = self.creation.as_mut() else {
            return;
        };
        creation.tab = Some(tab);
        creation.effort_menu = false;
        self.creation_model
            .update(cx, |input, cx| input.clear(window, cx));
        cx.notify();
    }

    /// The rows of a tab that match the search. The model the provider runs
    /// by default carries a badge and stands for the default, so picking it
    /// follows the provider when its default changes. A model id typed in the
    /// search that the provider does not list ends the rows, as a choice.
    fn picker_rows(&self, tab: &str, query: &str) -> Vec<ModelRow> {
        let owns = tab == self.creation_provider();
        let chosen = self
            .creation
            .as_ref()
            .and_then(|creation| creation.model.clone());
        let listed = self
            .models
            .get(tab)
            .filter(|models| models.state == ModelsState::Listed);
        let default = listed.and_then(|models| models.default_model.as_deref());
        let models: &[ModelChoice] = listed.map_or(&[], |models| &models.models);
        let needle = query.to_lowercase();
        let matches = |id: &str, name: &str| {
            needle.is_empty()
                || id.to_lowercase().contains(&needle)
                || name.to_lowercase().contains(&needle)
        };
        let mut rows = Vec::new();
        if listed.is_none() && needle.is_empty() {
            rows.push(ModelRow {
                value: None,
                name: "Default model".into(),
                default: false,
                chosen: owns && chosen.is_none(),
            });
        }
        if owns
            && let Some(id) = chosen.as_deref()
            && !models.iter().any(|model| model.id == id)
            && matches(id, id)
        {
            rows.push(ModelRow {
                value: Some(id.to_owned()),
                name: id.to_owned().into(),
                default: false,
                chosen: true,
            });
        }
        for model in models
            .iter()
            .filter(|model| matches(&model.id, &model.name))
        {
            let is_default = default == Some(model.id.as_str());
            rows.push(ModelRow {
                value: (!is_default).then(|| model.id.clone()),
                name: model.name.clone().into(),
                default: is_default,
                chosen: owns
                    && match chosen.as_deref() {
                        Some(id) => id == model.id,
                        None => is_default,
                    },
            });
        }
        let typed = query.trim();
        if !typed.is_empty()
            && !models.iter().any(|model| model.id == typed)
            && !(owns && chosen.as_deref() == Some(typed))
        {
            rows.push(ModelRow {
                value: Some(typed.to_owned()),
                name: format!("Use \u{201c}{typed}\u{201d}").into(),
                default: false,
                chosen: false,
            });
        }
        rows
    }

    /// The reasoning levels of a model, and the one it starts on: its own
    /// when the provider reported them, the provider's otherwise. `None` is
    /// the default model of the provider.
    pub(super) fn model_levels(
        &self,
        provider: &str,
        model: Option<&str>,
    ) -> (Vec<ModelChoice>, Option<String>) {
        let Some(models) = self.models.get(provider) else {
            return (Vec::new(), None);
        };
        let id = model.or(models.default_model.as_deref());
        match id
            .and_then(|id| models.models.iter().find(|choice| choice.id == id))
            .filter(|choice| !choice.efforts.is_empty())
        {
            Some(choice) => (choice.efforts.clone(), choice.default_effort.clone()),
            None => (models.efforts.clone(), models.default_effort.clone()),
        }
    }

    /// The model whose levels a tab shows: the bot's model on the tab of its
    /// provider, and the default model of another provider.
    fn tab_model(&self, tab: &str) -> Option<String> {
        (tab == self.creation_provider())
            .then(|| self.creation.as_ref()?.model.clone())
            .flatten()
    }

    /// A model picked on a tab moves the bot to that provider, with the
    /// reasoning level chosen on that tab. A level the new model does not
    /// have goes back to the model's default. The picker stays open while
    /// the model has levels to choose from.
    fn pick_model(
        &mut self,
        tab: String,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let moves = tab != self.creation_provider();
        let (levels, _) = self.model_levels(&tab, value.as_deref());
        let Some(creation) = self.creation.as_mut().filter(|creation| !creation.creating) else {
            return;
        };
        if moves {
            creation.provider = Some(tab.clone());
            creation.effort = None;
        }
        if let Some((queued, effort)) = creation.queued_effort.take()
            && queued == tab
        {
            creation.effort = effort;
        }
        if creation
            .effort
            .as_ref()
            .is_some_and(|effort| !levels.iter().any(|level| level.id == *effort))
        {
            creation.effort = None;
        }
        creation.model = value;
        creation.tab = None;
        creation.effort_menu = false;
        if levels.is_empty() {
            creation.picker = false;
        }
        self.creation_model
            .update(cx, |input, cx| input.clear(window, cx));
        cx.notify();
    }

    /// Enter in the search picks the first row it shows.
    pub(super) fn submit_model_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .creation
            .as_ref()
            .is_some_and(|creation| creation.picker)
        {
            return;
        }
        let tab = self.picker_tab();
        let query = self.creation_model.read(cx).text().to_owned();
        if let Some(row) = self.picker_rows(&tab, &query).into_iter().next() {
            self.pick_model(tab, row.value, window, cx);
        }
    }

    /// The reasoning level a tab shows: the chosen one on the tab of the
    /// bot's provider, and one waiting for a model on another tab.
    fn tab_effort(&self, tab: &str) -> Option<String> {
        let creation = self.creation.as_ref()?;
        if tab == self.creation_provider() {
            return creation.effort.clone();
        }
        creation
            .queued_effort
            .as_ref()
            .filter(|(queued, _)| queued == tab)
            .and_then(|(_, effort)| effort.clone())
    }

    fn pick_effort(&mut self, tab: String, effort: Option<String>, cx: &mut Context<Self>) {
        let owns = tab == self.creation_provider();
        let Some(creation) = self.creation.as_mut().filter(|creation| !creation.creating) else {
            return;
        };
        if owns {
            creation.effort = effort;
        } else {
            creation.queued_effort = Some((tab, effort));
        }
        creation.effort_menu = false;
        cx.notify();
    }

    fn toggle_effort_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(creation) = self.creation.as_mut() {
            creation.effort_menu = !creation.effort_menu;
            cx.notify();
        }
    }

    /// What the bot will run on, in words: the provider, the model, and the
    /// reasoning level when the provider has levels.
    fn model_summary(
        &self,
        provider: &str,
        effort: Option<&str>,
    ) -> (SharedString, SharedString, Option<SharedString>) {
        let provider_name = self
            .providers
            .iter()
            .find(|status| status.id == provider)
            .map_or_else(|| provider.to_owned(), |status| status.name.clone());
        let chosen = self
            .creation
            .as_ref()
            .and_then(|creation| creation.model.clone());
        let models = self.models.get(provider);
        let named = |id: &str, listed: &[ModelChoice]| {
            listed
                .iter()
                .find(|choice| choice.id == id)
                .map_or_else(|| id.to_owned(), |choice| choice.name.clone())
        };
        let model = match (chosen, models) {
            (Some(id), Some(models)) => named(&id, &models.models),
            (Some(id), None) => id,
            (None, Some(models)) => models.default_model.as_deref().map_or_else(
                || "Default model".to_owned(),
                |id| named(id, &models.models),
            ),
            (None, None) => "Default model".to_owned(),
        };
        let chosen = self
            .creation
            .as_ref()
            .and_then(|creation| creation.model.clone());
        let (levels, default_level) = self.model_levels(provider, chosen.as_deref());
        let level = (!levels.is_empty()).then(|| match effort.or(default_level.as_deref()) {
            Some(id) => SharedString::from(named(id, &levels)),
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
        let (provider_name, model, level) = self.model_summary(provider, effort);
        let spoken = match &level {
            Some(level) => format!("Model: {provider_name}, {model}, {level}"),
            None => format!("Model: {provider_name}, {model}"),
        };
        let focus = theme.border_focus;
        let bounds = self
            .creation
            .as_ref()
            .map(|creation| creation.row_bounds.clone())
            .unwrap_or_default();
        let measured = bounds.clone();
        let chevron = ui::icon(Icon::ChevronDown, icon_size::SM, theme.text_muted);
        let row = div()
            .id("creation-model-row")
            .debug_selector(|| "creation-model-row".into())
            .relative()
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
                    .on_click(
                        cx.listener(|this, _, window, cx| this.toggle_model_picker(window, cx)),
                    )
            })
            .child(
                canvas(move |area, _, _| measured.set(area), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
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
                        match self.providers.iter().find(|status| {
                            status.id == provider && provider_logo(&status.id).is_some()
                        }) {
                            // The logo names the provider; its name stays in the label.
                            Some(status) => div().flex_none().flex().child(provider_glyph(
                                status,
                                picker::TRIGGER_LOGO,
                                if status.id == "claude" {
                                    theme.claude_mark
                                } else {
                                    theme.text
                                },
                            )),
                            None => div()
                                .flex_none()
                                .text_color(theme.text_muted)
                                .child(provider_name),
                        },
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::truncated::truncated(model)),
                    )
                    .children(level.map(|level| {
                        div()
                            .flex_none()
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!("· {level}")))
                    })),
            )
            .child(if open {
                chevron.with_transformation(Transformation::rotate(radians(PI)))
            } else {
                chevron
            });
        let picker = open.then(|| {
            let room = window.viewport_size().height - picker::WINDOW_ROOM;
            deferred(
                anchored()
                    .attach(bounds.get(), Anchor::BottomLeft)
                    .anchor(Anchor::TopLeft)
                    .offset(point(px(0.), picker::GUTTER))
                    .snap_to_window_with_margin(space::S2)
                    .child({
                        let picker = self
                            .render_model_picker(theme, cx)
                            .max_h(room.min(picker::MAX_HEIGHT).max(picker::MIN_HEIGHT));
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
                            if let Some(creation) = this.creation.as_mut() {
                                creation.effort_menu = false;
                            }
                            this.close_model_picker(cx);
                        }),
                    ),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// The column of provider tabs. A tab shows the letters of its provider:
    /// muted at rest, full while its models are shown, and dimmer with a dot
    /// while the provider cannot run a turn yet.
    fn render_picker_tabs(
        &self,
        tab: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let tabs = self.providers.iter().enumerate().map(|(index, status)| {
            let shown = status.id == tab;
            let ready = status.state == ProviderState::Ready;
            let (state, _) = forms::provider_state(status, theme);
            let group = SharedString::from(format!("creation-provider-{index}"));
            let ink = Hsla::from(theme.text);
            let (rest, hover) = match (ready, shown) {
                (true, true) => (1., 1.),
                (true, false) => (picker::MARK_REST, picker::MARK_HOVER),
                (false, true) => (picker::MARK_OFF_SHOWN, picker::MARK_OFF_SHOWN),
                (false, false) => (picker::MARK_OFF, picker::MARK_OFF_SHOWN),
            };
            let dot = (!ready).then(|| {
                div()
                    .absolute()
                    .top(picker::TAB_DOT_INSET - picker::TAB_DOT_RING)
                    .right(picker::TAB_DOT_INSET - picker::TAB_DOT_RING)
                    .size(picker::TAB_DOT + picker::TAB_DOT_RING * 2.)
                    .rounded_full()
                    .border(picker::TAB_DOT_RING)
                    .border_color(theme.picker_rail)
                    .bg(if status.state == ProviderState::SignedOut {
                        theme.warning
                    } else {
                        theme.text_muted
                    })
            });
            let id = status.id.clone();
            let highlight = theme.menu_highlight;
            div()
                .id(("creation-provider", index))
                .group(group.clone())
                .debug_selector(move || {
                    if shown {
                        format!("creation-provider-{index}-chosen")
                    } else {
                        format!("creation-provider-{index}")
                    }
                })
                .relative()
                .flex_none()
                .size(picker::TAB)
                .flex()
                .items_center()
                .justify_center()
                .rounded(picker::ROW_RADIUS)
                .cursor_pointer()
                .focusable()
                .tab_stop(true)
                .role(Role::Tab)
                .aria_label(SharedString::from(format!("{}, {state}", status.name)))
                .when(shown, |tab| {
                    tab.bg(theme.picker_tab)
                        .border_1()
                        .border_color(theme.picker_tab_edge)
                })
                .when(!shown, |tab| tab.hover(move |style| style.bg(highlight)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_picker_tab(id.clone(), window, cx);
                }))
                .child(
                    div()
                        .flex()
                        .opacity(rest)
                        .group_hover(group, move |style| style.opacity(hover))
                        .child(provider_glyph(status, picker::TAB_LOGO, ink)),
                )
                .children(dot)
        });
        div()
            .id("creation-provider")
            .debug_selector(|| "creation-provider".into())
            .role(Role::TabList)
            .aria_label("Providers")
            .flex_none()
            .w(picker::RAIL)
            .flex()
            .flex_col()
            .items_center()
            .gap(space::S1)
            .px(picker::RAIL_PADDING_X)
            .py(picker::RAIL_PADDING_Y)
            .rounded_l(picker::RADIUS - size::HAIRLINE)
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.picker_rail)
            .children(tabs)
            .child(div().flex_1())
            .child(self.render_provider_settings_button(theme, cx))
    }

    /// The foot of the tab column: the provider settings, where a provider
    /// signs in, installs its runtime, or takes another executable.
    fn render_provider_settings_button(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (highlight, muted, ink) = (theme.menu_highlight, theme.text_muted, theme.text);
        div()
            .id("creation-provider-settings")
            .debug_selector(|| "creation-provider-settings".into())
            .group("creation-provider-settings")
            .flex_none()
            .size(picker::TAB)
            .flex()
            .items_center()
            .justify_center()
            .rounded(picker::ROW_RADIUS)
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label("Provider settings")
            .hover(move |style| style.bg(highlight))
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(creation) = this.creation.as_mut() {
                    creation.picker = false;
                    creation.effort_menu = false;
                }
                this.open_settings_at(Section::Providers, window, cx);
            }))
            .child(
                ui::icon(Icon::Settings, icon_size::MD, muted)
                    .group_hover("creation-provider-settings", move |style| {
                        style.text_color(ink)
                    }),
            )
    }

    fn render_model_option(
        &self,
        index: usize,
        tab: &str,
        row: ModelRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let ModelRow {
            value,
            name,
            default,
            chosen,
        } = row;
        let highlight = theme.menu_highlight;
        let radio = div()
            .flex_none()
            .size(picker::RADIO)
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .when(chosen, |radio| {
                radio.bg(theme.text).child(ui::icon(
                    Icon::Check,
                    picker::RADIO_CHECK,
                    theme.picker_surface,
                ))
            })
            .when(!chosen, |radio| {
                radio.border_1().border_color(theme.radio_edge)
            });
        let spoken = match (default, chosen) {
            (true, true) => format!("{name}, default, selected"),
            (true, false) => format!("{name}, default"),
            (false, true) => format!("{name}, selected"),
            (false, false) => name.to_string(),
        };
        let tab = tab.to_owned();
        div()
            .id(("creation-model-choice", index))
            .debug_selector(move || {
                if chosen {
                    format!("creation-model-choice-{index}-chosen")
                } else {
                    format!("creation-model-choice-{index}")
                }
            })
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded(picker::ROW_RADIUS)
            .py(picker::ROW_PADDING_Y)
            .pl(picker::INSET + picker::RADIO + space::S2)
            .pr(picker::INSET)
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .role(Role::RadioButton)
            .aria_label(SharedString::from(spoken))
            .hover(move |style| style.bg(highlight))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.pick_model(tab.clone(), value.clone(), window, cx);
            }))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(picker::INSET)
                    .flex()
                    .items_center()
                    .child(radio),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(picker::NAME_GAP)
                    .child(
                        div()
                            .min_w_0()
                            .text_size(text::MD.size)
                            .line_height(text::MD.leading)
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .children(default.then(|| badge("default", theme))),
            )
    }

    /// The reasoning level of the tab, under a line at the foot of the
    /// picker, as a select that opens its levels over the picker.
    fn render_effort_select(
        &self,
        tab: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let (levels, default_level) = self.model_levels(tab, self.tab_model(tab).as_deref());
        if levels.is_empty() {
            return None;
        }
        let current = self.tab_effort(tab);
        let chosen_id = current.clone().or_else(|| default_level.clone());
        let label = chosen_id.as_deref().map_or_else(
            || SharedString::from("Default"),
            |id| {
                levels
                    .iter()
                    .find(|level| level.id == id)
                    .map_or_else(|| id.to_owned(), |level| level.name.clone())
                    .into()
            },
        );
        let (open, bounds) = self
            .creation
            .as_ref()
            .map(|creation| (creation.effort_menu, creation.effort_bounds.clone()))
            .unwrap_or_default();
        let measured = bounds.clone();
        let fill = theme.menu_highlight;
        let pressed = theme.picker_tab;
        let select = div()
            .id("creation-effort")
            .debug_selector(|| "creation-effort".into())
            .relative()
            .flex_none()
            .w(picker::SELECT_WIDTH)
            .h(control::MD)
            .px(picker::INSET)
            .flex()
            .items_center()
            .justify_between()
            .gap(space::S2)
            .rounded(picker::ROW_RADIUS)
            .bg(if open { pressed } else { fill })
            .hover(move |style| style.bg(pressed))
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .role(Role::ComboBox)
            .aria_label(SharedString::from(format!("Reasoning: {label}")))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_effort_menu(cx)))
            .child(
                canvas(move |area, _, _| measured.set(area), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(text::MD.size)
                    .line_height(text::MD.leading)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(crate::truncated::truncated(label)),
            )
            .child(ui::icon(
                Icon::ChevronUpDown,
                icon_size::SM,
                theme.text_muted,
            ));
        let menu = open.then(|| {
            let entries = std::iter::once((None, None::<SharedString>))
                .take(usize::from(default_level.is_none()))
                .chain(levels.iter().map(|level| {
                    let value = (default_level.as_deref() != Some(level.id.as_str()))
                        .then(|| level.id.clone());
                    (value, Some(SharedString::from(level.name.clone())))
                }));
            let rows =
                entries.enumerate().map(|(index, (value, name))| {
                    let chosen = value == current;
                    let tab = tab.to_owned();
                    ui::menu_row(("creation-effort", index), chosen, false, theme)
                        .debug_selector(move || {
                            if chosen {
                                format!("creation-effort-{index}-chosen")
                            } else {
                                format!("creation-effort-{index}")
                            }
                        })
                        .focusable()
                        .tab_stop(true)
                        .role(Role::ListBoxOption)
                        .text_size(text::MD.size)
                        .line_height(text::MD.leading)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_effort(tab.clone(), value.clone(), cx);
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(
                            crate::truncated::truncated(name.unwrap_or_else(|| "Default".into())),
                        ))
                        .children(chosen.then(|| ui::menu_check(theme)))
                });
            deferred(
                anchored()
                    .attach(bounds.get(), Anchor::BottomRight)
                    .anchor(Anchor::TopRight)
                    .offset(point(px(0.), menu::PADDING))
                    .snap_to_window_with_margin(space::S2)
                    .child(ui::menu_reveal(
                        "creation-effort-reveal",
                        ui::menu_panel(false, theme)
                            .id("creation-effort-menu")
                            .debug_selector(|| "creation-effort-menu".into())
                            .occlude()
                            .role(Role::ListBox)
                            .aria_label("Reasoning")
                            .min_w(picker::SELECT_WIDTH)
                            .children(rows),
                        cx,
                    )),
            )
            .with_priority(3)
        });
        Some(
            div()
                .flex_none()
                .mt(picker::PADDING)
                .mx(-picker::PADDING)
                .pt(picker::PADDING)
                .pl(picker::PADDING + picker::INSET)
                .pr(picker::PADDING)
                .flex()
                .items_center()
                .justify_between()
                .gap(space::S3)
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_size(text::SM.size)
                        .line_height(text::SM.leading)
                        .text_color(theme.text_muted)
                        .child("Reasoning"),
                )
                .child(select)
                .children(menu),
        )
    }

    fn render_model_picker(&self, theme: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let tab = self.picker_tab();
        let status = self.providers.iter().find(|status| status.id == tab);
        let name = status.map_or_else(|| tab.clone(), |status| status.name.clone());
        let summary: SharedString = match status {
            Some(status) if status.state == ProviderState::Ready => status
                .account
                .clone()
                .or_else(|| status.runtime.clone())
                .unwrap_or_else(|| "Ready".to_owned())
                .into(),
            Some(status) => forms::provider_state(status, theme).0.into(),
            None => SharedString::default(),
        };
        let notice = status
            .and_then(forms::provider_notice)
            .map(|(text, _)| text)
            .or_else(|| self.models_note(&tab));
        let query = self.creation_model.read(cx).text().to_owned();
        let rows: Vec<Stateful<Div>> = self
            .picker_rows(&tab, &query)
            .into_iter()
            .enumerate()
            .map(|(index, row)| self.render_model_option(index, &tab, row, theme, cx))
            .collect();
        let heading = div()
            .flex_none()
            .pt(picker::HEADING_TOP)
            .pb(picker::HEADING_BOTTOM)
            .pl(picker::INSET)
            .flex()
            .flex_col()
            .gap(size::HAIRLINE)
            .child(
                div()
                    .truncate()
                    .text_size(text::LG.size)
                    .line_height(text::LG.leading)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::truncated::truncated(name)),
            )
            .child(
                div()
                    .truncate()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_muted)
                    .child(crate::truncated::truncated(summary)),
            );
        let notice = notice.map(|reason| {
            div()
                .flex_none()
                .mb(picker::PADDING)
                .py(space::S2)
                .px(picker::INSET)
                .rounded(picker::ROW_RADIUS)
                .bg(theme.menu_highlight)
                .child(forms::note(
                    "creation-models-note",
                    reason,
                    theme.text_secondary,
                ))
        });
        // The search sits in the picker like a row, on the row highlight,
        // with no edge of its own.
        let handle = self.creation_model.focus_handle(cx);
        let search = div()
            .id("creation-model")
            .debug_selector(|| "creation-model".into())
            .flex_none()
            .h(control::MD)
            .px(picker::INSET)
            .flex()
            .items_center()
            .rounded(picker::ROW_RADIUS)
            .bg(theme.menu_highlight)
            .cursor_text()
            .on_click(move |_, window, cx| window.focus(&handle, cx))
            .child(div().flex_1().min_w_0().child(self.creation_model.clone()));
        let empty = rows.is_empty().then(|| {
            div()
                .px(picker::INSET)
                .py(picker::ROW_PADDING_Y)
                .text_size(text::MD.size)
                .line_height(text::MD.leading)
                .text_color(theme.text_muted)
                .child("No model matches.")
        });
        let options = div()
            .id("creation-model-options")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pt(picker::PADDING)
            .flex()
            .flex_col()
            .children(rows)
            .children(empty);
        let content = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p(picker::PADDING)
            .flex()
            .flex_col()
            .child(heading)
            .children(notice)
            .child(search)
            .child(options)
            .children(self.render_effort_select(&tab, theme, cx));
        div()
            .id("creation-model-picker")
            .debug_selector(|| "creation-model-picker".into())
            .occlude()
            .role(Role::Dialog)
            .aria_label("Choose a model")
            .w(picker::WIDTH)
            .min_h(picker::MIN_HEIGHT)
            .flex()
            .rounded(picker::RADIUS)
            .border_1()
            .border_color(theme.dialog_ring)
            .bg(theme.picker_surface)
            .shadow(theme.menu_shadow())
            .text_color(theme.text)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(creation) = this.creation.as_mut()
                        && std::mem::take(&mut creation.effort_menu)
                    {
                        cx.notify();
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.close_model_picker(cx) {
                    cx.stop_propagation();
                }
            }))
            .child(self.render_picker_tabs(&tab, theme, cx))
            .child(content)
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
            .window_drag()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .gap(space::S1_5)
            .pl(layout::HEADER_PADDING_START)
            .pr(space::S2)
            .child(ui::living_avatar(&preview, size::AVATAR_HEADER, theme, cx))
            .child(
                div()
                    .id("creation-title")
                    .role(Role::Heading)
                    .aria_label(name.clone())
                    .min_w_0()
                    .truncate()
                    .child(crate::truncated::truncated(name)),
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
                shape: template.shape,
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
                .child(ui::avatar_of(&sample, size::AVATAR_TEMPLATE, theme))
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
                    ui::living_avatar(&preview, size::AVATAR_CREATION, theme, cx)
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
                    .id("creation-shapes")
                    .role(Role::Group)
                    .aria_label("Avatar head")
                    .child(self.render_shape_picker(
                        AvatarOwner::Creation,
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
