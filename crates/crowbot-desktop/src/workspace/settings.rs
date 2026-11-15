use crowbot_protocol::{
    Appearance, ClientMessage, PROTOCOL_VERSION, PreferencesPatch, ProviderStatus, SendShortcut,
};
use gpui::{
    AnyElement, Context, Div, Focusable, FontWeight, KeyDownEvent, Role, SharedString, Stateful,
    Window, div, prelude::*,
};

use super::{Link, Workspace, forms};
use crate::assets::Icon;
use crate::theme::{
    Theme, control, font, icon as icon_size, layout, opacity, radius, size, space, text,
};
use crate::ui::{self, ButtonKind};

/// The longest path of an executable the field takes.
pub(super) const EXECUTABLE_LENGTH: usize = 1024;

/// The field that takes the path of the user's own executable for a provider.
pub(super) struct ExecutableEdit {
    pub provider: String,
    pub error: Option<SharedString>,
    /// The path was sent, and the daemon has not answered yet.
    pub saving: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Section {
    General,
    Providers,
    About,
}

impl Section {
    const ALL: [Self; 3] = [Self::General, Self::Providers, Self::About];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Providers => "Providers",
            Self::About => "About",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::General => Icon::Sliders,
            Self::Providers => Icon::Plug,
            Self::About => Icon::Info,
        }
    }

    fn selector(self) -> &'static str {
        match self {
            Self::General => "settings-general",
            Self::Providers => "settings-providers",
            Self::About => "settings-about",
        }
    }
}

#[cfg(target_os = "macos")]
const MOD_ENTER: &str = "Cmd+Enter";
#[cfg(not(target_os = "macos"))]
const MOD_ENTER: &str = "Ctrl+Enter";

impl Workspace {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let section = self.settings.unwrap_or(Section::General);
        self.open_settings_at(section, window, cx);
    }

    pub(super) fn open_settings_at(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.loaded {
            return;
        }
        self.settings = Some(section);
        self.ask_providers();
        if self.live() {
            self.connection.send(ClientMessage::GetDaemonInfo);
        }
        window.focus(&self.settings_focus, cx);
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            if self.live() {
                self.focus_composer(window, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            self.close_settings(window, cx);
        } else {
            self.open_settings(window, cx);
        }
    }

    fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        if !self.live() || self.preferences.appearance == appearance {
            return;
        }
        self.preferences.appearance = appearance;
        self.apply_theme(cx);
        self.connection.send(ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                appearance: Some(appearance),
                ..PreferencesPatch::default()
            },
        });
        cx.notify();
    }

    fn set_send_shortcut(&mut self, shortcut: SendShortcut, cx: &mut Context<Self>) {
        if !self.live() || self.preferences.send_shortcut == shortcut {
            return;
        }
        self.preferences.send_shortcut = shortcut;
        self.connection.send(ClientMessage::SetPreferences {
            patch: PreferencesPatch {
                send_shortcut: Some(shortcut),
                ..PreferencesPatch::default()
            },
        });
        cx.notify();
    }

    pub(super) fn provider_request(&mut self, request: ClientMessage) {
        if self.live() {
            if let ClientMessage::SignIn { provider, .. } = &request {
                self.sign_in_pages.insert(provider.clone());
            }
            self.connection.send(request);
        }
    }

    fn segment(
        &self,
        id: &'static str,
        label: &'static str,
        chosen: bool,
        theme: &Theme,
    ) -> Stateful<Div> {
        let live = self.live();
        let (hover, focus) = (theme.control_hover, theme.border_focus);
        div()
            .id(id)
            .debug_selector(move || id.into())
            .focusable()
            .tab_stop(live)
            .role(Role::Button)
            .aria_label(SharedString::from(if chosen {
                format!("{label}, selected")
            } else {
                label.to_owned()
            }))
            .h(control::SM)
            .px(space::S3)
            .flex()
            .items_center()
            .rounded(radius::SM)
            .border_1()
            .border_color(gpui::transparent_black())
            .text_size(text::SM.size)
            .line_height(text::SM.leading)
            .when(chosen, |segment| {
                segment
                    .bg(theme.control_active)
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!chosen, |segment| segment.text_color(theme.text_secondary))
            .when(live, |segment| {
                segment
                    .cursor_pointer()
                    .focus_visible(move |style| style.border_color(focus))
                    .when(!chosen, |segment| {
                        segment.hover(move |style| style.bg(hover))
                    })
            })
            .child(label)
    }

    fn segments(&self, theme: &Theme) -> Div {
        div()
            .flex_none()
            .p(space::S0_5)
            .flex()
            .gap(space::S0_5)
            .rounded(radius::MD)
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.control)
            .when(!self.live(), |segments| segments.opacity(opacity::DISABLED))
    }

    fn settings_row(&self, title: &'static str, detail: &'static str, theme: &Theme) -> Div {
        div()
            .min_h(layout::SETTINGS_ROW)
            .px(space::S4)
            .py(space::S3)
            .flex()
            .items_center()
            .gap(space::S4)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(text::LG.size)
                            .line_height(text::LG.leading)
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(div().text_color(theme.text_muted).child(detail)),
            )
    }

    fn render_general(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let appearance = self.preferences.appearance;
        let shortcut = self.preferences.send_shortcut;
        let look = |id, label, value: Appearance, cx: &mut Context<Self>| {
            self.segment(id, label, appearance == value, theme)
                .on_click(cx.listener(move |this, _, _, cx| this.set_appearance(value, cx)))
        };
        let send = |id, label, value: SendShortcut, cx: &mut Context<Self>| {
            self.segment(id, label, shortcut == value, theme)
                .on_click(cx.listener(move |this, _, _, cx| this.set_send_shortcut(value, cx)))
        };
        forms::card(theme)
            .rounded(radius::LG)
            .child(
                self.settings_row(
                    "Appearance",
                    "System follows the theme of this computer.",
                    theme,
                )
                .child(
                    self.segments(theme)
                        .child(look("appearance-system", "System", Appearance::System, cx))
                        .child(look("appearance-light", "Light", Appearance::Light, cx))
                        .child(look("appearance-dark", "Dark", Appearance::Dark, cx)),
                ),
            )
            .child(
                div()
                    .mx(space::S4)
                    .h(size::HAIRLINE)
                    .bg(theme.border_strong),
            )
            .child(
                self.settings_row(
                    "Send a message with",
                    "The other key adds a line to the message.",
                    theme,
                )
                .child(
                    self.segments(theme)
                        .child(send("send-enter", "Enter", SendShortcut::Enter, cx))
                        .child(send(
                            "send-mod-enter",
                            MOD_ENTER,
                            SendShortcut::ModEnter,
                            cx,
                        )),
                ),
            )
    }

    /// Opens the field that takes the path of the user's own executable for
    /// a provider.
    fn edit_executable(&mut self, provider: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let current = self
            .providers
            .iter()
            .find(|status| status.id == provider)
            .and_then(|status| status.executable.clone())
            .unwrap_or_default();
        self.provider_executable
            .update(cx, |input, cx| input.set_text(&current, window, cx));
        self.executable_edit = Some(ExecutableEdit {
            provider: provider.to_owned(),
            error: None,
            saving: false,
        });
        window.focus(&self.provider_executable.focus_handle(cx), cx);
        cx.notify();
    }

    pub(super) fn save_executable(&mut self, cx: &mut Context<Self>) {
        let path = self.provider_executable.read(cx).text().trim().to_owned();
        let live = self.live();
        let Some(edit) = self.executable_edit.as_mut() else {
            return;
        };
        if edit.saving || !live {
            return;
        }
        if path.is_empty() {
            edit.error = Some("Give the full path of the executable.".into());
            cx.notify();
            return;
        }
        edit.saving = true;
        edit.error = None;
        let provider = edit.provider.clone();
        self.connection.send(ClientMessage::SetProviderExecutable {
            provider,
            path: Some(path),
        });
        cx.notify();
    }

    /// The daemon now starts the executable that was sent: the field closes.
    pub(super) fn executable_applied(&mut self, status: &ProviderStatus) {
        if self.executable_edit.as_ref().is_some_and(|edit| {
            edit.saving && edit.provider == status.id && status.executable.is_some()
        }) {
            self.executable_edit = None;
        }
    }

    fn clear_executable(&mut self, provider: &str, cx: &mut Context<Self>) {
        self.provider_request(ClientMessage::SetProviderExecutable {
            provider: provider.to_owned(),
            path: None,
        });
        cx.notify();
    }

    /// Where the program of a provider comes from: the runtime the daemon
    /// installs, or the executable the user set in its place.
    fn render_runtime(
        &self,
        status: &ProviderStatus,
        prefix: &'static str,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let source: SharedString = match (&status.executable, &status.runtime) {
            (Some(path), _) => format!("Runs your own executable: {path}").into(),
            (None, Some(runtime)) => format!("Runtime: {runtime}, managed by Crow Bot.").into(),
            (None, None) => return None,
        };
        let editing = self
            .executable_edit
            .as_ref()
            .filter(|edit| edit.provider == status.id);
        let dimmed = |button: Stateful<Div>| {
            if self.live() {
                button
            } else {
                button.opacity(opacity::DISABLED)
            }
        };
        let row = div()
            .debug_selector(move || format!("{prefix}-runtime"))
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(
                div()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_muted)
                    .child(source),
            );
        let Some(edit) = editing else {
            let (own, cleared) = (status.id.clone(), status.id.clone());
            return Some(
                row.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            dimmed(ui::button(
                                (prefix, 20_usize),
                                if status.executable.is_some() {
                                    "Change the executable"
                                } else {
                                    "Use my own executable"
                                },
                                ButtonKind::Ghost,
                                theme,
                            ))
                            .debug_selector(move || format!("{prefix}-own-executable"))
                            .aria_label(SharedString::from(if status.executable.is_some() {
                                format!("Change the executable of {}", status.name)
                            } else {
                                format!("Use my own executable for {}", status.name)
                            }))
                            .on_click(cx.listener(
                                move |this, _, window, cx| this.edit_executable(&own, window, cx),
                            )),
                        )
                        .children(status.executable.is_some().then(|| {
                            dimmed(ui::button(
                                (prefix, 21_usize),
                                "Use the Crow Bot runtime",
                                ButtonKind::Ghost,
                                theme,
                            ))
                            .debug_selector(move || format!("{prefix}-installed-runtime"))
                            .aria_label(SharedString::from(format!(
                                "Use the Crow Bot runtime for {}",
                                status.name
                            )))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.clear_executable(&cleared, cx);
                                },
                            ))
                        })),
                ),
            );
        };
        let (error, saving) = (edit.error.clone(), edit.saving);
        Some(
            row.child(self.input_box(
                "provider-executable",
                &self.provider_executable,
                error.is_some(),
                theme,
                window,
                cx,
            ))
            .children(
                error.map(|reason| {
                    forms::note("provider-executable-error", reason, theme.danger_text)
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        dimmed(ui::button(
                            "save-executable",
                            if saving { "Saving…" } else { "Save" },
                            if saving {
                                ButtonKind::Disabled
                            } else {
                                ButtonKind::Primary
                            },
                            theme,
                        ))
                        .on_click(cx.listener(|this, _, _, cx| this.save_executable(cx))),
                    )
                    .child(
                        ui::button("cancel-executable", "Cancel", ButtonKind::Ghost, theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.executable_edit = None;
                                cx.notify();
                            })),
                    ),
            ),
        )
    }

    fn render_provider(
        &self,
        index: usize,
        status: &ProviderStatus,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        const PREFIXES: [&str; 4] = ["provider-0", "provider-1", "provider-2", "provider-3"];
        let prefix = PREFIXES[index.min(PREFIXES.len() - 1)];
        let (state, color) = forms::provider_state(status, theme);
        let summary: Option<SharedString> = match (&status.detail, &status.account) {
            (Some(detail), _) => Some(detail.clone().into()),
            (None, Some(account)) => Some(format!("Signed in: {account}").into()),
            (None, None) => None,
        };
        div()
            .debug_selector(move || prefix.into())
            .px(space::S4)
            .py(space::S3)
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .text_size(text::LG.size)
                            .line_height(text::LG.leading)
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(status.name.clone())),
                    )
                    .child(
                        div()
                            .id((prefix, 0_usize))
                            .debug_selector(move || format!("{prefix}-state"))
                            .role(Role::Status)
                            .aria_label(SharedString::from(format!("{}: {state}", status.name)))
                            .flex()
                            .items_center()
                            .gap(space::S1_5)
                            .text_size(text::SM.size)
                            .line_height(text::SM.leading)
                            .text_color(color)
                            .child(div().size(size::STATE_DOT).rounded_full().bg(color))
                            .child(state),
                    ),
            )
            .children(summary.map(|summary| {
                div()
                    .debug_selector(move || format!("{prefix}-detail"))
                    .text_color(theme.text_muted)
                    .child(summary)
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(space::S2)
                    .children(self.provider_controls(status, prefix, theme, cx)),
            )
            .children(self.render_runtime(status, prefix, theme, window, cx))
    }

    fn render_providers(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> Div {
        let card = forms::card(theme).rounded(radius::LG);
        if self.providers.is_empty() {
            return card.child(
                div()
                    .debug_selector(|| "providers-loading".into())
                    .p(space::S4)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .text_color(theme.text_muted)
                    .children(
                        self.live().then(|| {
                            ui::spinner("providers-spinner", icon_size::SM, theme.text_muted)
                        }),
                    )
                    .child(if self.live() {
                        "Checking the providers…"
                    } else {
                        "The providers are not known."
                    }),
            );
        }
        let count = self.providers.len();
        card.children(
            self.providers
                .iter()
                .enumerate()
                .flat_map(|(index, status)| {
                    let row = self
                        .render_provider(index, status, theme, window, cx)
                        .into_any_element();
                    let line = (index + 1 < count).then(|| {
                        div()
                            .mx(space::S4)
                            .h(size::HAIRLINE)
                            .bg(theme.border_strong)
                            .into_any_element()
                    });
                    std::iter::once(row).chain(line)
                })
                .collect::<Vec<_>>(),
        )
    }

    fn render_about(&self, theme: &Theme) -> Div {
        let unknown = || SharedString::from("Not known");
        let info = self.daemon_info.as_ref();
        let rows: [(&'static str, &'static str, SharedString, bool); 4] = [
            (
                "about-client",
                "Crow Bot",
                env!("CARGO_PKG_VERSION").into(),
                false,
            ),
            (
                "about-daemon",
                "Daemon",
                info.map_or_else(unknown, |info| info.daemon_version.clone().into()),
                false,
            ),
            (
                "about-protocol",
                "Protocol",
                info.map_or_else(
                    || PROTOCOL_VERSION.to_string().into(),
                    |info| info.protocol_version.to_string().into(),
                ),
                false,
            ),
            (
                "about-database",
                "Database",
                info.map_or_else(unknown, |info| info.database.clone().into()),
                true,
            ),
        ];
        let count = rows.len();
        forms::card(theme)
            .rounded(radius::LG)
            .children(
                rows.into_iter()
                    .enumerate()
                    .map(|(index, (id, label, value, path))| {
                        div()
                            .id(id)
                            .debug_selector(move || id.into())
                            .role(Role::Label)
                            .aria_label(SharedString::from(format!("{label}: {value}")))
                            .px(space::S4)
                            .py(space::S3)
                            .flex()
                            .gap(space::S4)
                            .when(index + 1 < count, |row| {
                                row.border_b_1().border_color(theme.border_strong)
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_right()
                                    .text_color(theme.text_secondary)
                                    .when(path, |value| {
                                        value
                                            .font(font::mono())
                                            .text_size(text::SM.size)
                                            .line_height(text::MD.leading)
                                    })
                                    .child(value),
                            )
                    }),
            )
    }

    pub(super) fn render_settings_dialog(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let section = self.settings?;
        let (hover, focus) = (theme.hover, theme.border_focus);
        let nav = div()
            .flex_none()
            .w(layout::SETTINGS_NAV)
            .p(space::S3)
            .flex()
            .flex_col()
            .gap(space::S0_5)
            .border_r_1()
            .border_color(theme.border_strong)
            .child(
                div()
                    .px(space::S3)
                    .pb(space::S2)
                    .text_size(text::XL.size)
                    .line_height(text::XL.leading)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Settings"),
            )
            .children(Section::ALL.map(|item| {
                let chosen = item == section;
                let id = item.selector();
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .focusable()
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label(SharedString::from(if chosen {
                        format!("{}, selected", item.label())
                    } else {
                        item.label().to_owned()
                    }))
                    .h(control::MD)
                    .px(space::S3)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .rounded(radius::MD)
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .cursor_pointer()
                    .when(chosen, |row| {
                        row.bg(theme.control_hover).font_weight(FontWeight::MEDIUM)
                    })
                    .when(!chosen, |row| row.hover(move |style| style.bg(hover)))
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings = Some(item);
                        cx.notify();
                    }))
                    .child(ui::icon(item.icon(), icon_size::MD, theme.text_muted))
                    .child(item.label())
            }));

        let reason: Option<SharedString> = match &self.link {
            Link::Connected => None,
            Link::Failed(reason) | Link::Reconnecting(reason) if !reason.is_empty() => {
                Some(reason.clone())
            }
            _ => Some("It is starting or reconnecting.".into()),
        };
        let notice = reason.map(|reason| {
            let message = SharedString::from(format!(
                "The daemon cannot be reached, so these settings cannot be changed. {reason}"
            ));
            div()
                .id("settings-notice")
                .debug_selector(|| "settings-notice".into())
                .role(Role::Status)
                .aria_label(message.clone())
                .p(space::S3)
                .flex()
                .gap(space::S2)
                .rounded(radius::MD)
                .bg(theme.warning_soft)
                .child(
                    div()
                        .flex_none()
                        .h(text::MD.leading)
                        .flex()
                        .items_center()
                        .child(ui::icon(
                            Icon::TriangleAlert,
                            icon_size::SM,
                            theme.warning_text,
                        )),
                )
                .child(div().flex_1().min_w_0().child(message))
        });
        let body = match section {
            Section::General => self.render_general(theme, cx),
            Section::Providers => self.render_providers(theme, window, cx),
            Section::About => self.render_about(theme),
        };
        let content = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(layout::PALETTE_INPUT)
                    .pl(space::S6)
                    .pr(space::S3)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(text::XL.size)
                            .line_height(text::XL.leading)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(section.label()),
                    )
                    .child(
                        ui::icon_button("close-settings", Icon::X, "Close settings", theme)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_settings(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(space::S6)
                    .pb(space::S6)
                    .flex()
                    .flex_col()
                    .gap(space::S3)
                    // What scrolls keeps its own height: a card that shrank to
                    // the body would hide its last rows instead of scrolling.
                    .children(notice.map(|notice| notice.flex_none()))
                    .child(body.flex_none()),
            );
        let dialog = div()
            .id("settings-dialog")
            .debug_selector(|| "settings-dialog".into())
            .occlude()
            .track_focus(&self.settings_focus)
            .role(Role::Dialog)
            .aria_label("Settings")
            .w(layout::SETTINGS_WIDTH)
            .max_w_full()
            .h(layout::SETTINGS_HEIGHT)
            .max_h_full()
            .flex()
            .rounded(radius::CARD)
            .bg(theme.canvas)
            .border_1()
            .border_color(theme.dialog_ring)
            .shadow(theme.menu_shadow())
            .overflow_hidden()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_settings(window, cx);
                }
            }))
            .child(nav)
            .child(content);
        Some(
            div()
                .id("settings-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .p(space::S6)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.overlay)
                .on_click(cx.listener(|this, _, window, cx| this.close_settings(window, cx)))
                .child(dialog)
                .into_any_element(),
        )
    }
}
