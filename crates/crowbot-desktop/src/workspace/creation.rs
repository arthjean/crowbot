use crowbot_protocol::{AVATAR_COLORS, Avatar, Bot, BotDraft, ClientMessage, PermissionLevel};
use gpui::{
    AnyElement, Context, Div, Focusable, FontWeight, KeyDownEvent, Role, SharedString, Stateful,
    Window, WindowControlArea, div, prelude::*,
};

use super::forms::{self, AvatarOwner, GLYPHS};
use super::{FocusRequest, Workspace};
use crate::theme::{Theme, control, icon as icon_size, layout, radius, size, space, text};
use crate::ui::{self, ButtonKind};

pub(super) struct Template {
    pub name: &'static str,
    pub purpose: &'static str,
    glyph: usize,
    color: usize,
}

pub(super) const TEMPLATES: [Template; 6] = [
    Template {
        name: "Researcher",
        purpose: "Find sources on a question, read them, and report what they say, with links.",
        glyph: 8,
        color: 0,
    },
    Template {
        name: "Code reviewer",
        purpose: "Read a change, point out bugs and risks, and propose a fix for each one.",
        glyph: 7,
        color: 4,
    },
    Template {
        name: "Writer",
        purpose: "Draft and edit text in a clear, direct voice, and explain each edit.",
        glyph: 9,
        color: 5,
    },
    Template {
        name: "Data analyst",
        purpose: "Explore files and tables, and explain what the numbers show in plain words.",
        glyph: 10,
        color: 1,
    },
    Template {
        name: "Planner",
        purpose: "Break a goal into steps, keep track of them, and flag what blocks progress.",
        glyph: 5,
        color: 2,
    },
    Template {
        name: "Tutor",
        purpose: "Explain a topic step by step, and check understanding with short questions.",
        glyph: 1,
        color: 6,
    },
];

#[derive(Default)]
pub(super) struct Creation {
    pub color: usize,
    /// `None` is the first letter of the name.
    pub glyph: Option<usize>,
    pub template: Option<usize>,
    /// `None` is the first provider of the daemon.
    pub provider: Option<String>,
    /// `None` is the default reasoning level of the provider.
    pub effort: Option<String>,
    pub name_error: Option<SharedString>,
    pub error: Option<SharedString>,
    pub creating: bool,
    /// The fields still hold what an earlier form left in them.
    pub stale: bool,
    /// The form opened by itself, because no bot existed.
    automatic: bool,
}

impl Workspace {
    fn new_creation(&self) -> Creation {
        Creation {
            color: self.bots.len() % AVATAR_COLORS.len(),
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
            self.focus_request = Some(FocusRequest::CreationName);
            self.ask_providers();
        }
    }

    /// A bot now exists: a form that opened by itself, and that nobody
    /// filled, gives way to it.
    pub(super) fn creation_yields(&mut self, cx: &Context<Self>) {
        let untouched = [&self.creation_name, &self.creation_purpose]
            .iter()
            .all(|input| input.read(cx).text().is_empty());
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
        window.focus(&self.creation_name.focus_handle(cx), cx);
        cx.notify();
    }

    pub(super) fn cancel_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bots.is_empty() || self.creation.take().is_none() {
            return;
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn creation_avatar(&self, cx: &Context<Self>) -> Avatar {
        let (color, glyph) = self
            .creation
            .as_ref()
            .map_or((0, None), |creation| (creation.color, creation.glyph));
        Avatar {
            color: AVATAR_COLORS[color % AVATAR_COLORS.len()].to_owned(),
            glyph: match glyph {
                Some(index) => GLYPHS[index % GLYPHS.len()].to_owned(),
                None => forms::initial(self.creation_name.read(cx).text()),
            },
        }
    }

    fn apply_template(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(template), Some(creation)) = (TEMPLATES.get(index), self.creation.as_mut())
        else {
            return;
        };
        if creation.creating {
            return;
        }
        creation.template = Some(index);
        creation.glyph = Some(template.glyph);
        creation.color = template.color;
        creation.name_error = None;
        creation.error = None;
        self.creation_name.update(cx, |input, cx| {
            input.set_text(template.name, window, cx);
        });
        self.creation_purpose.update(cx, |input, cx| {
            input.set_text(template.purpose, window, cx);
        });
        window.focus(&self.creation_name.focus_handle(cx), cx);
        cx.notify();
    }

    pub(super) fn create_bot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.creation_name.read(cx).text().trim().to_owned();
        let instructions = self.creation_purpose.read(cx).text().to_owned();
        let model = self.creation_model.read(cx).text().trim().to_owned();
        let avatar = self.creation_avatar(cx);
        let live = self.live();
        let Some(creation) = self.creation.as_mut() else {
            return;
        };
        if creation.creating || !live {
            return;
        }
        creation.error = None;
        if name.is_empty() {
            creation.name_error = Some("A bot needs a name.".into());
            window.focus(&self.creation_name.focus_handle(cx), cx);
            cx.notify();
            return;
        }
        creation.name_error = None;
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

    fn preview_bot(&self, cx: &Context<Self>) -> Bot {
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
            .pl(layout::HEADER_PADDING_START + space::S1)
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

    fn render_templates(
        &self,
        selected: Option<usize>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let (hover, focus, accent) = (theme.control_hover, theme.border_focus, theme.accent);
        let cards = TEMPLATES.iter().enumerate().map(|(index, template)| {
            let chosen = selected == Some(index);
            let sample = Bot {
                avatar: Avatar {
                    color: AVATAR_COLORS[template.color].to_owned(),
                    glyph: GLYPHS[template.glyph].to_owned(),
                },
                ..self.preview_bot(cx)
            };
            div()
                .id(("template", index))
                .debug_selector(move || format!("template-{index}"))
                .focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(SharedString::from(format!(
                    "{}: {}",
                    template.name, template.purpose
                )))
                .flex_none()
                .w(layout::TEMPLATE_WIDTH)
                .h(layout::TEMPLATE_HEIGHT)
                .p(space::S4)
                .flex()
                .gap(space::S3)
                .rounded(radius::LG)
                .border_1()
                .border_color(if chosen { accent } else { theme.border_strong })
                .bg(if chosen {
                    theme.accent_soft
                } else {
                    theme.surface
                })
                .cursor_pointer()
                .when(!chosen, |card| card.hover(move |style| style.bg(hover)))
                .focus_visible(move |style| style.border_color(focus))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_template(index, window, cx);
                }))
                .child(ui::avatar(
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
                                .truncate()
                                .text_size(text::XL.size)
                                .line_height(text::XL.leading)
                                .font_weight(FontWeight::MEDIUM)
                                .child(template.name),
                        )
                        .child(
                            div()
                                .line_clamp(3)
                                .text_ellipsis()
                                .text_color(theme.text_muted)
                                .child(template.purpose),
                        ),
                )
        });
        div()
            .flex_none()
            .w_full()
            .px(space::S6)
            .pb(space::S4)
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(forms::group_heading("Start from a suggestion", theme))
            .child(
                div()
                    .id("templates")
                    .debug_selector(|| "templates".into())
                    .role(Role::Group)
                    .aria_label("Suggestions")
                    .w_full()
                    .overflow_x_scroll()
                    .flex()
                    .gap(space::S3)
                    .pb(space::S2)
                    .children(cards),
            )
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
            for input in [
                &self.creation_name,
                &self.creation_purpose,
                &self.creation_model,
            ] {
                input.update(cx, |input, cx| input.clear(window, cx));
            }
        }
        let (creating, template) = (creation.creating, creation.template);
        let effort = creation.effort.clone();
        let (name_error, error) = (creation.name_error.clone(), creation.error.clone());
        let first = self.bots.is_empty();
        let preview = self.preview_bot(cx);
        let live = self.live();

        let name = forms::field(
            "Name",
            self.input_box(
                "creation-name",
                &self.creation_name,
                name_error.is_some(),
                theme,
                window,
                cx,
            ),
            theme,
        )
        .children(
            name_error.map(|reason| forms::note("creation-name-error", reason, theme.danger_text)),
        );
        let purpose = forms::field(
            "Purpose",
            self.input_box(
                "creation-purpose",
                &self.creation_purpose,
                false,
                theme,
                window,
                cx,
            )
            .items_start(),
            theme,
        );
        let provider = self.creation_provider();
        let typed = self.creation_model.read(cx).text().trim().to_owned();
        let model = forms::field(
            "Model",
            self.input_box(
                "creation-model",
                &self.creation_model,
                false,
                theme,
                window,
                cx,
            ),
            theme,
        )
        .child(self.choice_chips(
            "creation-model-choice",
            "Model",
            self.model_choices(&provider, (!typed.is_empty()).then_some(typed.as_str())),
            Self::pick_creation_model,
            theme,
            cx,
        ))
        .children(
            self.models_note(&provider)
                .map(|reason| forms::note("creation-models-note", reason, theme.text_muted)),
        );
        let levels = self.effort_choices(&provider, effort.as_deref());
        let reasoning = (!levels.is_empty()).then(|| {
            forms::field(
                "Reasoning",
                self.choice_chips(
                    "creation-effort",
                    "Reasoning level",
                    levels,
                    Self::pick_creation_effort,
                    theme,
                    cx,
                ),
                theme,
            )
        });
        let provider_field = (self.providers.len() > 1).then(|| {
            forms::field(
                "Provider",
                self.choice_chips(
                    "creation-provider",
                    "Provider",
                    self.provider_choices(&provider),
                    Self::pick_creation_provider,
                    theme,
                    cx,
                ),
                theme,
            )
        });
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
                .bg(theme.hover)
                .text_color(theme.text_muted)
                .font_weight(FontWeight::MEDIUM)
                .child(ui::spinner(
                    "creating-spinner",
                    icon_size::SM,
                    theme.text_muted,
                ))
                .child("Creating…")
        } else {
            ui::button(
                "create-bot",
                "Create",
                if live {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Disabled
                },
                theme,
            )
            .h(control::LG)
            .min_w(layout::CREATE_MIN_WIDTH)
            .px(space::S4)
            .rounded(radius::MD)
            .on_click(cx.listener(|this, _, window, cx| this.create_bot(window, cx)))
        };

        let form = div()
            .w(layout::CREATION_WIDTH)
            .max_w_full()
            .py(space::S6)
            .flex()
            .flex_col()
            .gap(space::S3)
            .children(first.then(|| {
                div()
                    .pb(space::S2)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(space::S1)
                    .child(
                        div()
                            .debug_selector(|| "first-bot-heading".into())
                            .text_size(text::XXL.size)
                            .line_height(text::XXL.leading)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Create your first bot"),
                    )
                    .child(div().text_center().text_color(theme.text_muted).child(
                        "A bot is an AI teammate with its own instructions and its own folder.",
                    ))
            }))
            .child(
                div().flex().justify_center().child(
                    ui::avatar(
                        &preview,
                        size::AVATAR_CREATION,
                        size::AVATAR_CREATION_GLYPH,
                        theme,
                    )
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
                        &preview.avatar.color,
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
                        &preview.avatar.glyph,
                        theme,
                        cx,
                    )),
            )
            .child(name)
            .child(purpose)
            .children(provider_field)
            .child(model)
            .children(reasoning)
            .children(notice)
            .children(error.map(|reason| {
                div()
                    .debug_selector(|| "creation-error".into())
                    .text_center()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.danger_text)
                    .child(reason)
            }))
            .child(div().pt(space::S1).flex().justify_center().child(create));

        div()
            .id("creation")
            .debug_selector(|| "creation".into())
            .role(Role::Group)
            .aria_label(if first {
                "Create your first bot"
            } else {
                "Create a bot"
            })
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_creation(window, cx);
                }
            }))
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .px(space::S6)
                    .flex()
                    .justify_center()
                    .child(form),
            )
            .child(self.render_templates(template, theme, cx))
            .into_any_element()
    }
}
