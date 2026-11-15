use crowbot_protocol::{
    Avatar, BOT_INSTRUCTIONS_LENGTH, Bot, BotPatch, ClientMessage, PermissionLevel,
};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, Role, SharedString, Stateful, Task, Window, div,
    prelude::*,
};

use super::forms::{self, AvatarOwner};
use super::{Confirm, Workspace, grouped};
use crate::assets::Icon;
use crate::composer::Composer;
use crate::theme::{
    Theme, control, duration, font, icon as icon_size, layout, radius, size, space, text,
};
use crate::ui;

const UNREACHABLE: &str = "The daemon cannot be reached.";
/// The share of the limit from which the field shows how full it is.
const COUNTER_FROM: usize = BOT_INSTRUCTIONS_LENGTH / 10 * 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelField {
    Name,
    Title,
    Instructions,
    Model,
}

impl PanelField {
    pub const ALL: [Self; 4] = [Self::Name, Self::Title, Self::Instructions, Self::Model];

    fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Title => "Title",
            Self::Instructions => "Instructions",
            Self::Model => "Model",
        }
    }

    fn selector(self) -> (&'static str, &'static str) {
        match self {
            Self::Name => ("panel-name", "panel-name-error"),
            Self::Title => ("panel-title", "panel-title-error"),
            Self::Instructions => ("panel-instructions", "panel-instructions-error"),
            Self::Model => ("panel-model", "panel-model-error"),
        }
    }

    fn value(self, bot: &Bot) -> String {
        match self {
            Self::Name => bot.name.clone(),
            Self::Title => bot.title.clone(),
            Self::Instructions => bot.instructions.clone(),
            Self::Model => bot.model.clone().unwrap_or_default(),
        }
    }

    /// What the daemon stores for a typed text: it trims every field but the
    /// instructions.
    fn stored(self, typed: &str) -> &str {
        match self {
            Self::Instructions => typed,
            _ => typed.trim(),
        }
    }

    fn patch(self, typed: &str) -> BotPatch {
        let text = self.stored(typed).to_owned();
        match self {
            Self::Name => BotPatch {
                name: Some(text),
                ..BotPatch::default()
            },
            Self::Title => BotPatch {
                title: Some(text),
                ..BotPatch::default()
            },
            Self::Instructions => BotPatch {
                instructions: Some(text),
                ..BotPatch::default()
            },
            Self::Model => BotPatch {
                model: Some((!text.is_empty()).then_some(text)),
                ..BotPatch::default()
            },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum SaveState {
    #[default]
    Idle,
    Saving,
    Failed(SharedString),
}

#[derive(Default)]
pub(super) struct FieldSave {
    /// The value the daemon holds.
    pub saved: String,
    pub state: SaveState,
    timer: Option<Task<()>>,
}

#[derive(Default)]
pub(super) struct BotPanel {
    /// The bot whose values the fields hold.
    pub bot_id: Option<i64>,
    pub fields: [FieldSave; 4],
    pub avatar_open: bool,
}

impl Workspace {
    fn panel_input(&self, field: PanelField) -> &Entity<Composer> {
        match field {
            PanelField::Name => &self.panel_name,
            PanelField::Title => &self.panel_title,
            PanelField::Instructions => &self.panel_instructions,
            PanelField::Model => &self.panel_model,
        }
    }

    fn panel_shown(&self) -> bool {
        self.side_panel && self.creation.is_none() && self.selected_bot.is_some()
    }

    /// Brings the fields in line with the bot on screen. A field that holds
    /// text the daemon does not have yet keeps it.
    pub(super) fn sync_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.panel_shown().then_some(self.selected_bot).flatten();
        if self.panel.bot_id != shown {
            self.flush_panel(cx);
            self.panel = BotPanel {
                bot_id: shown,
                ..BotPanel::default()
            };
            let Some(bot) = self.selected_bot().map(|summary| summary.bot.clone()) else {
                return;
            };
            if shown.is_none() {
                return;
            }
            for field in PanelField::ALL {
                let value = field.value(&bot);
                self.panel_input(field)
                    .update(cx, |input, cx| input.set_text(&value, window, cx));
                self.panel.fields[field as usize].saved = value;
            }
            return;
        }
        let Some(bot) = self.selected_bot().map(|summary| summary.bot.clone()) else {
            return;
        };
        for field in PanelField::ALL {
            let value = field.value(&bot);
            let input = self.panel_input(field).clone();
            let slot = &mut self.panel.fields[field as usize];
            if value == slot.saved {
                continue;
            }
            let clean = field.stored(input.read(cx).text()) == slot.saved;
            slot.saved.clone_from(&value);
            if clean {
                input.update(cx, |input, cx| input.set_text(&value, window, cx));
            }
        }
    }

    /// Text was typed in a field: it is saved once the typing pauses.
    pub(super) fn panel_edited(&mut self, field: PanelField, cx: &mut Context<Self>) {
        if self.panel.bot_id.is_none() {
            return;
        }
        let typed = self.panel_input(field).read(cx).text().to_owned();
        let slot = &mut self.panel.fields[field as usize];
        if field.stored(&typed) == slot.saved {
            slot.timer = None;
            if slot.state != SaveState::Saving {
                slot.state = SaveState::Idle;
            }
        } else {
            slot.timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration::FIELD_SAVE).await;
                this.update(cx, |this, cx| this.save_panel_field(field, None, cx))
                    .ok();
            }));
        }
        cx.notify();
    }

    /// Saves a field whose text differs from what the daemon holds. With a
    /// window, the field lost the focus: an empty name goes back to the
    /// saved one.
    pub(super) fn save_panel_field(
        &mut self,
        field: PanelField,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(bot_id) = self.panel.bot_id else {
            return;
        };
        let input = self.panel_input(field).clone();
        let typed = input.read(cx).text().to_owned();
        let live = self.live();
        let slot = &mut self.panel.fields[field as usize];
        slot.timer = None;
        if field.stored(&typed) == slot.saved {
            return;
        }
        if field == PanelField::Name && typed.trim().is_empty() {
            if let Some(window) = window {
                let saved = slot.saved.clone();
                slot.state = SaveState::Idle;
                input.update(cx, |input, cx| input.set_text(&saved, window, cx));
                cx.notify();
            }
            return;
        }
        if live {
            slot.state = SaveState::Saving;
            self.send_update(bot_id, field.patch(&typed), Some(field));
        } else {
            slot.state = SaveState::Failed(UNREACHABLE.into());
        }
        cx.notify();
    }

    /// Saves what the fields still hold before they show another bot.
    pub(super) fn flush_panel(&mut self, cx: &mut Context<Self>) {
        for field in PanelField::ALL {
            if self.panel.fields[field as usize].timer.is_some() {
                self.save_panel_field(field, None, cx);
            }
        }
    }

    /// The daemon answered the oldest save that waited. The answer is for the
    /// fields on screen only while they still hold the bot it was sent for.
    pub(super) fn panel_saved(&mut self, bot_id: i64, field: PanelField) {
        let slot = &mut self.panel.fields[field as usize];
        if self.panel.bot_id == Some(bot_id) && slot.state == SaveState::Saving {
            slot.state = SaveState::Idle;
        }
    }

    pub(super) fn panel_refused(&mut self, bot_id: i64, field: PanelField, reason: String) {
        if self.panel.bot_id == Some(bot_id) {
            self.panel.fields[field as usize].state = SaveState::Failed(reason.into());
        } else {
            // The panel shows another bot by now: the failure is told apart.
            let label = field.label().to_lowercase();
            self.notice = Some(format!("The {label} was not saved: {reason}").into());
        }
    }

    pub(super) fn panel_link_lost(&mut self) {
        for slot in &mut self.panel.fields {
            if slot.state == SaveState::Saving {
                slot.state = SaveState::Failed(UNREACHABLE.into());
            }
        }
    }

    /// Moves the bot to another provider. The daemon gives it back the model
    /// and the reasoning level it had with that provider.
    fn pick_panel_provider(
        &mut self,
        provider: Option<String>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        let (Some(bot_id), Some(provider)) = (self.panel.bot_id, provider) else {
            return;
        };
        let current = self.bot(bot_id).map(|summary| summary.bot.provider.clone());
        if !self.live() || current.as_deref() == Some(provider.as_str()) {
            return;
        }
        let shown = provider.clone();
        if self.change_bot(bot_id, |summary| {
            summary.bot.provider = shown;
            summary.bot.model = None;
            summary.bot.effort = None;
        }) {
            self.send_update(
                bot_id,
                BotPatch {
                    provider: Some(provider),
                    ..BotPatch::default()
                },
                None,
            );
        }
    }

    fn pick_panel_model(
        &mut self,
        model: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = model.unwrap_or_default();
        self.panel_model
            .update(cx, |input, cx| input.set_text(&model, window, cx));
        self.save_panel_field(PanelField::Model, None, cx);
    }

    fn pick_panel_effort(&mut self, effort: Option<String>, _: &mut Window, _: &mut Context<Self>) {
        let Some(bot_id) = self.panel.bot_id else {
            return;
        };
        let current = self.bot(bot_id).map(|summary| summary.bot.effort.clone());
        if !self.live() || current.as_ref() == Some(&effort) {
            return;
        }
        let shown = effort.clone();
        if self.change_bot(bot_id, |summary| summary.bot.effort = shown) {
            self.send_update(
                bot_id,
                BotPatch {
                    effort: Some(effort),
                    ..BotPatch::default()
                },
                None,
            );
        }
    }

    pub(super) fn save_avatar(&mut self, bot_id: i64, avatar: Avatar) {
        if !self.live() {
            return;
        }
        let shown = avatar.clone();
        if self.change_bot(bot_id, |summary| summary.bot.avatar = shown) {
            self.send_update(
                bot_id,
                BotPatch {
                    avatar: Some(avatar),
                    ..BotPatch::default()
                },
                None,
            );
        }
    }

    fn set_level(
        &mut self,
        bot_id: i64,
        level: PermissionLevel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.bot(bot_id).map(|summary| summary.bot.permission_level);
        if !self.live() || current.is_none() || current == Some(level) {
            return;
        }
        match level {
            PermissionLevel::FullAccess => {
                self.confirm = Some(Confirm::FullAccess(bot_id));
                window.focus(&self.confirm_focus, cx);
            }
            PermissionLevel::AskFirst => self.save_level(bot_id, level),
        }
        cx.notify();
    }

    pub(super) fn save_level(&mut self, bot_id: i64, level: PermissionLevel) {
        if self.change_bot(bot_id, |summary| summary.bot.permission_level = level) {
            self.send_update(
                bot_id,
                BotPatch {
                    permission_level: Some(level),
                    ..BotPatch::default()
                },
                None,
            );
        }
    }

    pub(super) fn delete_history(&mut self, bot_id: i64) {
        if self.live() {
            self.connection
                .send(ClientMessage::DeleteHistory { bot_id });
        }
    }

    fn render_panel_field(
        &self,
        field: PanelField,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let (selector, error_selector) = field.selector();
        let slot = &self.panel.fields[field as usize];
        let failed = match &slot.state {
            SaveState::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let input = self.panel_input(field);
        let typed = input.read(cx).text().chars().count();
        let full = input.read(cx).is_full();
        let counter = (field == PanelField::Instructions && typed >= COUNTER_FROM).then(|| {
            let limit = grouped(BOT_INSTRUCTIONS_LENGTH);
            if full {
                forms::note(
                    "panel-instructions-limit",
                    format!("{limit} / {limit} characters: the limit is reached."),
                    theme.warning_text,
                )
            } else {
                forms::note(
                    "panel-instructions-count",
                    format!("{} / {limit} characters", grouped(typed)),
                    theme.text_muted,
                )
            }
        });
        let boxed = self
            .input_box(selector, input, failed.is_some(), theme, window, cx)
            .when(field == PanelField::Instructions, |boxed| {
                boxed.items_start()
            });
        forms::field(field.label(), boxed, theme)
            .children(counter)
            .children(failed.map(|reason| {
                forms::note(
                    error_selector,
                    format!("Save failed: {reason}"),
                    theme.danger_text,
                )
            }))
    }

    fn panel_row(&self, theme: &Theme) -> Div {
        div()
            .min_h(layout::PANEL_ROW)
            .px(space::S3)
            .flex()
            .items_center()
            .gap(space::S2)
            .border_color(theme.border_strong)
    }

    fn render_level_row(
        &self,
        bot: &Bot,
        level: PermissionLevel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (id, name, detail) = match level {
            PermissionLevel::AskFirst => (
                "level-ask-first",
                "Ask first",
                "Asks before each gated action.",
            ),
            PermissionLevel::FullAccess => (
                "level-full-access",
                "Full access",
                "Runs every tool without asking.",
            ),
        };
        let chosen = bot.permission_level == level;
        let bot_id = bot.id;
        let (hover, focus) = (theme.control_hover, theme.border_focus);
        self.panel_row(theme)
            .id(id)
            .debug_selector(move || id.into())
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(SharedString::from(if chosen {
                format!("{name}, selected. {detail}")
            } else {
                format!("{name}. {detail}")
            }))
            .py(space::S2)
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(focus))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_level(bot_id, level, window, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::MEDIUM).child(name))
                    .child(
                        div()
                            .text_size(text::SM.size)
                            .line_height(text::SM.leading)
                            .text_color(theme.text_muted)
                            .child(detail),
                    ),
            )
            .children(chosen.then(|| {
                ui::icon(Icon::Check, icon_size::MD, theme.text)
                    .debug_selector(move || format!("{id}-selected"))
            }))
    }

    fn render_action_row(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: Icon,
        danger: bool,
        theme: &Theme,
    ) -> Stateful<Div> {
        let color = if danger {
            theme.danger_text
        } else {
            theme.text
        };
        let hover = if danger {
            theme.danger_soft
        } else {
            theme.control_hover
        };
        let focus = theme.border_focus;
        self.panel_row(theme)
            .id(id)
            .debug_selector(move || id.into())
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(label)
            .border_1()
            .border_color(gpui::transparent_black())
            .text_color(color)
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(focus))
            .child(ui::icon(glyph, icon_size::MD, color))
            .child(label)
    }

    pub(super) fn render_bot_panel(
        &self,
        docked: bool,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.panel_shown() {
            return None;
        }
        let bot = self.selected_bot()?.bot.clone();
        let bot_id = bot.id;
        let provider = self
            .providers
            .iter()
            .find(|status| status.id == bot.provider);
        let provider_name: SharedString = provider
            .map_or_else(|| bot.provider.clone(), |status| status.name.clone())
            .into();
        let divider = || div().flex_none().h(size::HAIRLINE).bg(theme.border_strong);
        let focus = theme.border_focus;

        let avatar = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(space::S3)
            .child(
                div()
                    .id("panel-avatar")
                    .debug_selector(|| "panel-avatar".into())
                    .focusable()
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label(if self.panel.avatar_open {
                        "Close the avatar choices"
                    } else {
                        "Change the avatar"
                    })
                    .relative()
                    .rounded_full()
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .cursor_pointer()
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel.avatar_open = !this.panel.avatar_open;
                        cx.notify();
                    }))
                    .child(ui::avatar(
                        &bot,
                        size::AVATAR_PANEL,
                        size::AVATAR_PANEL_GLYPH,
                        theme,
                    ))
                    .child(
                        div()
                            .absolute()
                            .right(-space::S1)
                            .bottom(-space::S0_5)
                            .size(control::XS)
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme.surface_raised)
                            .border_1()
                            .border_color(theme.border_strong)
                            .child(ui::icon(Icon::Pencil, icon_size::XS, theme.text_secondary)),
                    ),
            )
            .children(self.panel.avatar_open.then(|| {
                div()
                    .debug_selector(|| "panel-avatar-choices".into())
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .child(self.render_color_picker(
                        AvatarOwner::Panel,
                        &bot.avatar,
                        forms::is_automatic(&bot.avatar),
                        theme,
                        cx,
                    ))
                    .child(self.render_glyph_picker(
                        AvatarOwner::Panel,
                        &forms::initial(&bot.name),
                        &bot.avatar,
                        theme,
                        cx,
                    ))
            }));

        let levels = self.effort_choices(&bot.provider, bot.effort.as_deref());
        let brain = div()
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(forms::group_heading("Brain", theme))
            .child(
                forms::card(theme).child(
                    self.panel_row(theme)
                        .debug_selector(|| "panel-provider".into())
                        .child(ui::icon(Icon::Plug, icon_size::MD, theme.text_muted))
                        .child(div().font_weight(FontWeight::MEDIUM).child("Provider"))
                        .child(div().flex_1())
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text_secondary)
                                .child(provider_name),
                        )
                        .children(provider.map(|status| {
                            let (label, color) = forms::provider_state(status, theme);
                            div()
                                .flex_none()
                                .text_size(text::SM.size)
                                .line_height(text::SM.leading)
                                .text_color(color)
                                .child(label)
                        })),
                ),
            )
            .children((self.providers.len() > 1).then(|| {
                self.choice_chips(
                    "panel-provider-choice",
                    "Provider",
                    self.provider_choices(&bot.provider),
                    Self::pick_panel_provider,
                    theme,
                    cx,
                )
            }))
            .children(
                provider.and_then(|status| self.render_provider_notice(status, "panel", theme, cx)),
            )
            .child(self.render_panel_field(PanelField::Model, theme, window, cx))
            .child(self.choice_chips(
                "panel-model-choice",
                "Model",
                self.model_choices(&bot.provider, bot.model.as_deref()),
                Self::pick_panel_model,
                theme,
                cx,
            ))
            .children(
                self.models_note(&bot.provider)
                    .map(|reason| forms::note("panel-models-note", reason, theme.text_muted)),
            )
            .children((!levels.is_empty()).then(|| {
                forms::field(
                    "Reasoning",
                    self.choice_chips(
                        "panel-effort",
                        "Reasoning level",
                        levels,
                        Self::pick_panel_effort,
                        theme,
                        cx,
                    ),
                    theme,
                )
            }));

        let permissions = div()
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(forms::group_heading("Permissions", theme))
            .child(
                forms::card(theme)
                    .child(self.render_level_row(&bot, PermissionLevel::AskFirst, theme, cx))
                    .child(divider())
                    .child(self.render_level_row(&bot, PermissionLevel::FullAccess, theme, cx)),
            );

        let folder = div()
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(forms::group_heading("Folder", theme))
            .child(
                forms::card(theme).child(
                    self.panel_row(theme)
                        .debug_selector(|| "panel-folder".into())
                        .py(space::S2)
                        .child(ui::icon(Icon::Folder, icon_size::MD, theme.text_muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font(font::mono())
                                .text_size(text::SM.size)
                                .line_height(text::SM.leading)
                                .text_color(theme.text_secondary)
                                .child(SharedString::from(bot.workspace.clone())),
                        ),
                ),
            );

        let advanced = div()
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(forms::group_heading("Advanced", theme))
            .child(
                forms::card(theme)
                    .child(
                        self.render_action_row(
                            "panel-new-chat",
                            "New chat",
                            Icon::SquarePen,
                            false,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.new_chat();
                            this.focus_composer(window, cx);
                        })),
                    )
                    .child(divider())
                    .child(
                        self.render_action_row(
                            "panel-delete-history",
                            "Delete history",
                            Icon::History,
                            true,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if this.live() {
                                    this.confirm = Some(Confirm::DeleteHistory(bot_id));
                                    window.focus(&this.confirm_focus, cx);
                                    cx.notify();
                                }
                            },
                        )),
                    )
                    .child(divider())
                    .child(
                        self.render_action_row(
                            "panel-delete-bot",
                            "Delete bot",
                            Icon::Trash,
                            true,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if this.live() {
                                    this.confirm_delete = Some(bot_id);
                                    window.focus(&this.dialog_focus, cx);
                                    cx.notify();
                                }
                            },
                        )),
                    ),
            );

        Some(
            div()
                .id("side-panel")
                .debug_selector(|| "side-panel".into())
                .occlude()
                .role(Role::Group)
                .aria_label("Bot settings")
                .map(|panel| {
                    if docked {
                        panel.flex_none().h_full()
                    } else {
                        panel.absolute().top_0().bottom_0().right_0().max_w_full()
                    }
                })
                .w(layout::SIDE_PANEL)
                .flex()
                .flex_col()
                .rounded_r(radius::CARD)
                .bg(theme.canvas)
                .border_l_1()
                .border_color(theme.border_strong)
                .child(
                    div()
                        .flex_none()
                        .h(layout::SIDE_PANEL_HEADER)
                        .px(space::S1)
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(div().flex_none().size(control::SM))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_center()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Bot settings"),
                        )
                        .child(
                            ui::icon_button(
                                "close-side-panel",
                                Icon::X,
                                "Close bot settings",
                                theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_side_panel(cx))),
                        ),
                )
                .child(
                    div()
                        .id("side-panel-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .pt(space::S6)
                        .px(layout::PANEL_PADDING_X)
                        .pb(layout::PANEL_PADDING_BOTTOM)
                        .flex()
                        .flex_col()
                        .gap(space::S4)
                        .child(avatar)
                        .child(self.render_panel_field(PanelField::Name, theme, window, cx))
                        .child(self.render_panel_field(PanelField::Title, theme, window, cx))
                        .child(self.render_panel_field(PanelField::Instructions, theme, window, cx))
                        .child(brain)
                        .child(permissions)
                        .child(folder)
                        .child(advanced),
                )
                .into_any_element(),
        )
    }
}
