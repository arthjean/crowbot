use std::cell::Cell;
use std::rc::Rc;

use crowbot_protocol::{
    Appearance, ClientMessage, PROTOCOL_VERSION, PreferencesPatch, ProviderState, ProviderStatus,
    SendShortcut,
};
use gpui::{
    Anchor, AnyElement, Bounds, Context, Div, Focusable, FontWeight, KeyDownEvent, MouseButton,
    Pixels, Role, SharedString, Stateful, Window, anchored, canvas, deferred, div, point,
    prelude::*, px,
};

use super::rail::provider_glyph;
use super::{Workspace, forms};
use crate::assets::Icon;
use crate::chrome::WindowDrag;
use crate::theme::{
    Theme, control, font, icon as icon_size, layout, menu, opacity, radius, settings, size, space,
    text,
};
use crate::ui::{self, ButtonKind};

/// The longest path of an executable the field takes.
pub(super) const EXECUTABLE_LENGTH: usize = 1024;

/// The longest query of the settings search.
pub(super) const SETTINGS_QUERY_LENGTH: usize = 64;

const NAV_ROW_GROUP: &str = "settings-nav-row";
const PATH_PARENT_GROUP: &str = "settings-path-parent";
/// Shown when the daemon's computer stops the user's services at logout.
const LINGER_NOTICE: &str = "Bots stop when you log out. To keep them running, run:";
const LINGER_COMMAND: &str = "loginctl enable-linger";

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

/// A setting with a short list of values: a row shows it as a select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Select {
    Theme,
    Send,
}

impl Select {
    fn index(self) -> usize {
        match self {
            Self::Theme => 0,
            Self::Send => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme",
            Self::Send => "Send messages with",
        }
    }

    fn selector(self) -> &'static str {
        match self {
            Self::Theme => "settings-theme",
            Self::Send => "settings-send",
        }
    }

    fn options(self) -> &'static [&'static str] {
        match self {
            Self::Theme => &["System", "Light", "Dark"],
            Self::Send => &["Enter", MOD_ENTER],
        }
    }
}

const APPEARANCES: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];
const SHORTCUTS: [SendShortcut; 2] = [SendShortcut::Enter, SendShortcut::ModEnter];

/// The settings page while it is open.
pub(super) struct SettingsPage {
    pub section: Section,
    /// The provider whose own page is open, under Providers.
    pub provider: Option<String>,
    /// The select whose menu is open, with the row the keyboard is on.
    select: Option<(Select, usize)>,
    /// Where each select is in the window, so its menu opens under it.
    select_bounds: [Rc<Cell<Bounds<Pixels>>>; 2],
}

impl SettingsPage {
    fn at(section: Section) -> Self {
        Self {
            section,
            provider: None,
            select: None,
            select_bounds: Default::default(),
        }
    }
}

/// One result of the settings search: a section, a row of one, or a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Hit {
    section: Section,
    provider: Option<String>,
    title: SharedString,
    detail: SharedString,
}

/// The entries whose words hold every word of the query, in any letter case.
/// An entry whose title starts with the query comes first, then one whose
/// title holds it.
fn rank(query: &str, entries: Vec<Hit>) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    let words: Vec<&str> = needle.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut ranked: Vec<(u8, Hit)> = entries
        .into_iter()
        .filter_map(|hit| {
            let title = hit.title.to_lowercase();
            let all = format!(
                "{title} {} {}",
                hit.detail.to_lowercase(),
                hit.section.label().to_lowercase()
            );
            if !words.iter().all(|word| all.contains(word)) {
                return None;
            }
            let rank = if title.starts_with(&needle) {
                0
            } else if title.contains(&needle) {
                1
            } else {
                2
            };
            Some((rank, hit))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, hit)| hit).collect()
}

/// The fill of the badge that tells the state of a provider.
fn state_fill(status: &ProviderStatus, theme: &Theme) -> forms::Tint {
    match status.state {
        ProviderState::Ready => theme.success_soft.into(),
        ProviderState::SignedOut => theme.warning_soft.into(),
        ProviderState::RuntimeMissing => theme.danger_soft.into(),
        ProviderState::NotInstalled | ProviderState::Unknown => theme.badge.into(),
    }
}

/// The state of a provider as a pill: a dot and a word, in the state color.
/// A state with no color of its own reads in the secondary text color: the
/// muted one is too faint on the fill of the pill.
fn state_badge(status: &ProviderStatus, theme: &Theme) -> Div {
    let (state, color) = forms::provider_state(status, theme);
    let color = match status.state {
        ProviderState::NotInstalled | ProviderState::Unknown => theme.text_secondary.into(),
        _ => color,
    };
    div()
        .flex_none()
        .h(settings::BADGE_HEIGHT)
        .px(space::S2)
        .flex()
        .items_center()
        .gap(space::S1_5)
        .rounded_full()
        .bg(state_fill(status, theme))
        .text_size(text::SM.size)
        .line_height(text::SM.leading)
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(
            div()
                .flex_none()
                .size(settings::BADGE_DOT)
                .rounded_full()
                .bg(color),
        )
        .child(state)
}

/// The mark of a provider, in its own color when Crow Bot knows one.
fn provider_mark(status: &ProviderStatus, logo: Pixels, theme: &Theme) -> AnyElement {
    let color = if status.id == "claude" {
        theme.claude_mark
    } else {
        theme.text
    };
    provider_glyph(status, logo, color)
}

fn group_heading(id: &'static str, title: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Heading)
        .aria_label(title)
        .text_size(text::MD.size)
        .line_height(text::MD.leading)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_secondary)
        .child(title)
}

/// A card of rows, with a line between two rows. The card has no edge: its
/// fill is one step above the page.
fn card(rows: Vec<AnyElement>, theme: &Theme) -> Div {
    let count = rows.len();
    div()
        .flex()
        .flex_col()
        .rounded(radius::LG)
        .bg(theme.surface)
        .children(rows.into_iter().enumerate().flat_map(|(index, row)| {
            let line = (index + 1 < count).then(|| {
                div()
                    .flex_none()
                    .mx(space::S4)
                    .h(size::HAIRLINE)
                    .bg(theme.border)
                    .into_any_element()
            });
            std::iter::once(row).chain(line)
        }))
}

fn group(heading: Stateful<Div>, card: Div) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(heading)
        .child(card)
        .into_any_element()
}

/// The title of a row over what it means.
fn row_text(title: impl Into<SharedString>, detail: Option<AnyElement>, theme: &Theme) -> Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(space::S0_5)
        .child(
            div()
                .text_size(text::TITLE.size)
                .line_height(text::TITLE.leading)
                .font_weight(FontWeight::MEDIUM)
                .child(title.into()),
        )
        .children(detail.map(|detail| div().text_color(theme.text_muted).child(detail)))
}

/// A row of a card: its text, then its control at the trailing edge. A
/// narrow pane puts the control under the text.
fn row(text: Div, control: Option<AnyElement>, stacked: bool) -> Div {
    let row = div()
        .flex_none()
        .min_h(settings::ROW_MIN)
        .px(space::S4)
        .py(space::S3)
        .flex();
    if stacked {
        row.flex_col()
            .justify_center()
            .gap(space::S2)
            .child(text)
            .children(control.map(|control| div().flex().child(control)))
    } else {
        row.items_center()
            .gap(space::S4)
            .child(text.flex_1())
            .children(control)
    }
}

/// The controls of a row, side by side, wrapping when the row is short.
fn controls(items: Vec<AnyElement>, stacked: bool) -> Option<AnyElement> {
    if items.is_empty() {
        return None;
    }
    Some(
        div()
            .flex_none()
            .max_w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .when(!stacked, |row| row.justify_end())
            .gap(space::S2)
            .children(items)
            .into_any_element(),
    )
}

#[cfg(target_os = "macos")]
const MOD_ENTER: &str = "Cmd+Enter";
#[cfg(not(target_os = "macos"))]
const MOD_ENTER: &str = "Ctrl+Enter";

impl Workspace {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let section = self
            .settings
            .as_ref()
            .map_or(Section::General, |page| page.section);
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
        match self.settings.as_mut() {
            Some(page) => {
                page.section = section;
                page.provider = None;
                page.select = None;
            }
            None => self.settings = Some(SettingsPage::at(section)),
        }
        self.ask_providers();
        if self.live() {
            self.connection.send(ClientMessage::GetDaemonInfo);
        }
        window.focus(&self.settings_focus, cx);
        cx.notify();
    }

    /// Opens the page of one provider, under Providers.
    pub(super) fn open_provider_settings(
        &mut self,
        provider: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A page that is already open has what the daemon tells it.
        if self.settings.is_none() {
            self.open_settings_at(Section::Providers, window, cx);
        }
        if let Some(page) = self.settings.as_mut() {
            page.section = Section::Providers;
            page.provider = Some(provider.to_owned());
            page.select = None;
            self.executable_edit = None;
            window.focus(&self.settings_focus, cx);
            cx.notify();
        }
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self.executable_edit = None;
            self.settings_query
                .update(cx, |query, cx| query.clear(window, cx));
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

    /// One level up: from the page of a provider to the list of providers,
    /// and from a section to the bots.
    fn settings_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.settings.as_mut() else {
            return;
        };
        if page.provider.take().is_some() {
            page.select = None;
            self.executable_edit = None;
            window.focus(&self.settings_focus, cx);
            cx.notify();
        } else {
            self.close_settings(window, cx);
        }
    }

    fn show_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if let Some(page) = self.settings.as_mut() {
            page.section = section;
            page.provider = None;
            page.select = None;
            self.executable_edit = None;
            cx.notify();
        }
    }

    /// Escape closes, in this order: an open menu, the executable field, the
    /// search, then the page.
    pub(super) fn settings_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_none() || event.keystroke.key != "escape" {
            return;
        }
        cx.stop_propagation();
        if self.close_settings_select(cx) {
            return;
        }
        if self.executable_edit.take().is_some() {
            window.focus(&self.settings_focus, cx);
            cx.notify();
            return;
        }
        if !self.settings_query.read(cx).text().is_empty() {
            self.settings_query
                .update(cx, |query, cx| query.clear(window, cx));
            cx.notify();
            return;
        }
        self.close_settings(window, cx);
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

    /// The value a select shows, as an index into its options.
    fn select_value(&self, select: Select) -> usize {
        match select {
            Select::Theme => APPEARANCES
                .iter()
                .position(|value| *value == self.preferences.appearance),
            Select::Send => SHORTCUTS
                .iter()
                .position(|value| *value == self.preferences.send_shortcut),
        }
        .unwrap_or_default()
    }

    fn pick_setting(&mut self, select: Select, index: usize, cx: &mut Context<Self>) {
        self.close_settings_select(cx);
        match select {
            Select::Theme => {
                if let Some(value) = APPEARANCES.get(index) {
                    self.set_appearance(*value, cx);
                }
            }
            Select::Send => {
                if let Some(value) = SHORTCUTS.get(index) {
                    self.set_send_shortcut(*value, cx);
                }
            }
        }
    }

    /// Closes the open menu of a select. Tells whether one was open.
    pub(super) fn close_settings_select(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self
            .settings
            .as_mut()
            .is_some_and(|page| page.select.take().is_some());
        if open {
            cx.notify();
        }
        open
    }

    /// A press on a select, by the pointer or by Enter or Space: it opens
    /// the menu on the current value, or takes the row the keyboard is on.
    fn select_pressed(&mut self, select: Select, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let current = self.select_value(select);
        let Some(page) = self.settings.as_mut() else {
            return;
        };
        match page.select {
            Some((open, row)) if open == select => self.pick_setting(select, row, cx),
            _ => {
                page.select = Some((select, current));
                cx.notify();
            }
        }
    }

    /// The arrows move through the values of a select, and open its menu
    /// first when it is closed.
    fn select_key(&mut self, select: Select, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let last = select.options().len() - 1;
        let current = self.select_value(select);
        let Some(page) = self.settings.as_mut() else {
            return;
        };
        let row = match page.select {
            Some((open, row)) if open == select => Some(row),
            _ => None,
        };
        let next = match (event.keystroke.key.as_str(), row) {
            ("down" | "up", None) => current,
            ("down", Some(row)) => (row + 1).min(last),
            ("up", Some(row)) => row.saturating_sub(1),
            ("home", Some(_)) => 0,
            ("end", Some(_)) => last,
            _ => return,
        };
        cx.stop_propagation();
        page.select = Some((select, next));
        cx.notify();
    }

    /// What the settings search looks through: the rows of each section, and
    /// each provider.
    fn settings_entries(&self) -> Vec<Hit> {
        let entry = |section, title: &'static str, detail: &'static str| Hit {
            section,
            provider: None,
            title: title.into(),
            detail: detail.into(),
        };
        let theme = Theme::dark();
        let mut entries = vec![
            entry(
                Section::General,
                Select::Theme.label(),
                "Appearance: light or dark colors",
            ),
            entry(
                Section::General,
                Select::Send.label(),
                "Conversations: the key that sends a message",
            ),
            entry(
                Section::Providers,
                "Providers",
                "The accounts and runtimes your bots run on",
            ),
        ];
        entries.extend(self.providers.iter().map(|status| Hit {
            section: Section::Providers,
            provider: Some(status.id.clone()),
            title: status.name.clone().into(),
            detail: forms::provider_state(status, &theme).0.into(),
        }));
        entries.push(entry(
            Section::About,
            "Versions",
            "Crow Bot, the daemon, and the protocol",
        ));
        entries.push(entry(
            Section::About,
            "Database",
            "Where your bots and conversations are stored",
        ));
        entries
    }

    /// The results of the settings search, or `None` while its field is empty.
    pub(super) fn settings_hits(&self, cx: &Context<Self>) -> Option<Vec<Hit>> {
        let query = self.settings_query.read(cx).text().trim().to_owned();
        (!query.is_empty()).then(|| rank(&query, self.settings_entries()))
    }

    pub(super) fn settings_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.settings_hits(cx).map_or(0, |hits| hits.len());
        if count == 0 {
            return;
        }
        self.settings_hit = if forward {
            (self.settings_hit + 1).min(count - 1)
        } else {
            self.settings_hit.saturating_sub(1)
        };
        cx.notify();
    }

    pub(super) fn open_active_setting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hit = self
            .settings_hits(cx)
            .and_then(|hits| hits.into_iter().nth(self.settings_hit));
        if let Some(hit) = hit {
            self.open_hit_setting(hit, window, cx);
        }
    }

    /// Opens the place of a search result, and clears the search.
    fn open_hit_setting(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_query
            .update(cx, |query, cx| query.clear(window, cx));
        self.executable_edit = None;
        if let Some(page) = self.settings.as_mut() {
            page.section = hit.section;
            page.provider = hit.provider;
            page.select = None;
        }
        window.focus(&self.settings_focus, cx);
        cx.notify();
    }

    /// The select of a row: its value, and a mark that it opens a list.
    fn render_select(
        &self,
        select: Select,
        page: &SettingsPage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let live = self.live();
        let value = select.options()[self.select_value(select)];
        let open = page.select.is_some_and(|(shown, _)| shown == select);
        let measured = page.select_bounds[select.index()].clone();
        let (pressed, focus) = (theme.control_active, theme.border_focus);
        let id = select.selector();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .relative()
            .flex_none()
            .w(settings::SELECT_WIDTH)
            .h(control::MD)
            .px(space::S3)
            .flex()
            .items_center()
            .justify_between()
            .gap(space::S2)
            .rounded(radius::MD)
            .border_1()
            .border_color(theme.border_strong)
            .bg(if open { pressed } else { theme.surface_raised })
            .focusable()
            .tab_stop(live)
            .role(Role::ComboBox)
            .aria_label(SharedString::from(format!("{}: {value}", select.label())))
            .when(live, |trigger| {
                trigger
                    .cursor_pointer()
                    .hover(move |style| style.bg(pressed))
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_pressed(select, cx)))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        this.select_key(select, event, cx);
                    }))
            })
            .when(!live, |trigger| trigger.opacity(opacity::DISABLED))
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
                    .font_weight(FontWeight::MEDIUM)
                    .child(crate::truncated::truncated(value)),
            )
            .child(ui::icon(
                Icon::ChevronUpDown,
                icon_size::SM,
                theme.text_muted,
            ))
    }

    /// The menu of the open select, over a layer that closes it on a press
    /// anywhere else.
    pub(super) fn render_settings_select(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let page = self.settings.as_ref()?;
        let (select, highlighted) = page.select?;
        let current = self.select_value(select);
        let bounds = page.select_bounds[select.index()].get();
        let viewport = window.viewport_size();
        let highlight = theme.menu_highlight;
        let rows: Vec<Stateful<Div>> = select
            .options()
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let chosen = index == current;
                let prefix = select.selector();
                ui::menu_row(("settings-select", index), chosen, false, theme)
                    .debug_selector(move || format!("{prefix}-{index}"))
                    .role(Role::ListBoxOption)
                    .aria_label(SharedString::from(if chosen {
                        format!("{option}, selected")
                    } else {
                        (*option).to_owned()
                    }))
                    .text_size(text::MD.size)
                    .line_height(text::MD.leading)
                    .when(index == highlighted && !chosen, |row| row.bg(highlight))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.pick_setting(select, index, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(crate::truncated::truncated(*option)),
                    )
                    .children(chosen.then(|| ui::menu_check(theme)))
            })
            .collect();
        let backdrop = deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("settings-select-backdrop")
                    .debug_selector(|| "settings-select-backdrop".into())
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_settings_select(cx);
                        }),
                    ),
            ),
        )
        .with_priority(1);
        let menu = deferred(
            anchored()
                .attach(bounds, Anchor::BottomRight)
                .anchor(Anchor::TopRight)
                // The measured area stops inside the edge of the select.
                .offset(point(size::HAIRLINE, menu::PADDING + size::HAIRLINE))
                .snap_to_window_with_margin(space::S2)
                .child(ui::menu_reveal(
                    "settings-select-reveal",
                    ui::menu_panel(false, theme)
                        .id("settings-select-menu")
                        .debug_selector(|| "settings-select-menu".into())
                        .occlude()
                        .role(Role::ListBox)
                        .aria_label(select.label())
                        .w(settings::SELECT_WIDTH)
                        .children(rows),
                    cx,
                )),
        )
        .with_priority(2);
        Some(div().child(backdrop).child(menu).into_any_element())
    }

    fn render_general(
        &self,
        page: &SettingsPage,
        stacked: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let setting = |select: Select, detail: &'static str, cx: &mut Context<Self>| {
            row(
                row_text(select.label(), Some(detail.into_any_element()), theme),
                Some(
                    self.render_select(select, page, theme, cx)
                        .into_any_element(),
                ),
                stacked,
            )
            .into_any_element()
        };
        let appearance = setting(
            Select::Theme,
            "System follows the theme of this computer.",
            cx,
        );
        let send = setting(
            Select::Send,
            "The other key adds a line to the message.",
            cx,
        );
        vec![
            group(
                group_heading("settings-appearance", "Appearance", theme),
                card(vec![appearance], theme),
            ),
            group(
                group_heading("settings-conversations", "Conversations", theme),
                card(vec![send], theme),
            ),
        ]
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
            edit.error = Some("Enter the full path of the executable.".into());
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

    /// A button of a card, faded while the daemon is away.
    fn card_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        theme: &Theme,
    ) -> Stateful<Div> {
        let button = ui::button(id, label, ButtonKind::Raised, theme);
        if self.live() {
            button
        } else {
            button.opacity(opacity::DISABLED)
        }
    }

    /// The row that tells which program Crow Bot starts for a provider: the
    /// runtime it installs, or the executable the user set in its place.
    fn render_executable_row(
        &self,
        status: &ProviderStatus,
        prefix: &'static str,
        stacked: bool,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editing = self
            .executable_edit
            .as_ref()
            .filter(|edit| edit.provider == status.id);
        if let Some(edit) = editing {
            let (error, saving) = (edit.error.clone(), edit.saving);
            return div()
                .flex_none()
                .px(space::S4)
                .py(space::S3)
                .flex()
                .flex_col()
                .gap(space::S2)
                .child(forms::field(
                    "Executable path",
                    self.input_box(
                        "provider-executable",
                        &self.provider_executable,
                        error.is_some(),
                        theme,
                        window,
                        cx,
                    ),
                    theme,
                ))
                .children(error.map(|reason| {
                    forms::note("provider-executable-error", reason, theme.danger_text)
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            ui::button(
                                "save-executable",
                                if saving { "Saving…" } else { "Save" },
                                if saving || !self.live() {
                                    ButtonKind::PrimaryDisabled
                                } else {
                                    ButtonKind::Primary
                                },
                                theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_executable(cx))),
                        )
                        .child(
                            ui::button("cancel-executable", "Cancel", ButtonKind::Raised, theme)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.executable_edit = None;
                                    window.focus(&this.settings_focus, cx);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element();
        }
        let (own, cleared) = (status.id.clone(), status.id.clone());
        let name = &status.name;
        let detail = match &status.executable {
            Some(path) => div()
                .font(font::mono())
                .text_size(text::SM.size)
                .child(SharedString::from(path.clone()))
                .into_any_element(),
            None => "Crow Bot starts the runtime it installs.".into_any_element(),
        };
        let mut buttons = vec![
            self.card_button(
                (prefix, 20_usize),
                if status.executable.is_some() {
                    "Change"
                } else {
                    "Use my own"
                },
                theme,
            )
            .debug_selector(move || format!("{prefix}-own-executable"))
            .aria_label(SharedString::from(if status.executable.is_some() {
                format!("Change the executable of {name}")
            } else {
                format!("Use my own executable for {name}")
            }))
            .on_click(
                cx.listener(move |this, _, window, cx| this.edit_executable(&own, window, cx)),
            )
            .into_any_element(),
        ];
        if status.executable.is_some() {
            buttons.push(
                self.card_button((prefix, 21_usize), "Use the Crow Bot runtime", theme)
                    .debug_selector(move || format!("{prefix}-installed-runtime"))
                    .aria_label(SharedString::from(format!(
                        "Use the Crow Bot runtime for {name}"
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.clear_executable(&cleared, cx);
                    }))
                    .into_any_element(),
            );
        }
        row(
            row_text("Executable", Some(detail), theme),
            controls(buttons, stacked),
            stacked,
        )
        .into_any_element()
    }

    /// The sign-in of a provider, as a title and what to do next.
    fn account_text(status: &ProviderStatus) -> (SharedString, SharedString) {
        let name = &status.name;
        if status.signing_in {
            let next = match status.sign_in_url.as_deref().and_then(forms::sign_in_code) {
                Some(code) => format!("Check that the page shows the code {code}."),
                None => "This row updates when the sign-in is done.".to_owned(),
            };
            return ("Finish signing in in your browser".into(), next.into());
        }
        match status.state {
            ProviderState::Ready => (
                status
                    .account
                    .clone()
                    .unwrap_or_else(|| "Signed in".to_owned())
                    .into(),
                format!("Bots on {name} run on this account.").into(),
            ),
            ProviderState::SignedOut if status.can_sign_in => (
                "Not signed in".into(),
                format!("Sign in so that bots on {name} can answer.").into(),
            ),
            ProviderState::SignedOut => (
                "Not signed in".into(),
                format!("Sign in with the {name} program, then check again.").into(),
            ),
            ProviderState::NotInstalled => (
                "Not signed in".into(),
                "Install the runtime first, then sign in.".into(),
            ),
            ProviderState::RuntimeMissing => (
                "Sign-in not known".into(),
                format!("{name} cannot start, so Crow Bot cannot read its sign-in.").into(),
            ),
            ProviderState::Unknown => (
                "Sign-in not known".into(),
                if status.checking {
                    format!("Checking {name}…").into()
                } else {
                    "Check again to read the sign-in.".into()
                },
            ),
        }
    }

    /// The page of one provider: its state, its account, and its runtime.
    fn render_provider_page(
        &self,
        index: usize,
        status: &ProviderStatus,
        stacked: bool,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        const PREFIXES: [&str; 4] = ["provider-0", "provider-1", "provider-2", "provider-3"];
        let prefix = PREFIXES[index.min(PREFIXES.len() - 1)];
        let name = SharedString::from(status.name.clone());
        let (state, _) = forms::provider_state(status, theme);
        let summary: SharedString = match (&status.detail, forms::provider_problem(status)) {
            (Some(detail), _) => detail.clone().into(),
            (None, Some(problem)) => problem,
            (None, None) if status.state == ProviderState::Ready => "Ready to run bots.".into(),
            (None, None) => state.into(),
        };
        let header = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(space::S3)
            .child(
                div()
                    .flex_none()
                    .size(settings::LOGO_TILE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::LG)
                    .border_1()
                    .border_color(theme.border_strong)
                    .bg(theme.surface_raised)
                    .child(provider_mark(status, settings::LOGO, theme)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::S0_5)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .child(
                                div()
                                    .id((prefix, 30_usize))
                                    .role(Role::Heading)
                                    .aria_label(name.clone())
                                    .min_w_0()
                                    .truncate()
                                    .text_size(text::XXL.size)
                                    .line_height(text::XXL.leading)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(crate::truncated::truncated(name.clone())),
                            )
                            .child(
                                state_badge(status, theme)
                                    .id((prefix, 0_usize))
                                    .debug_selector(move || format!("{prefix}-state"))
                                    .role(Role::Status)
                                    .aria_label(SharedString::from(format!("{name}: {state}"))),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("{prefix}-detail"))
                            .text_color(theme.text_muted)
                            .child(summary),
                    ),
            )
            .child(self.check_control(status, prefix, true, theme, cx))
            .into_any_element();

        let (title, next) = Self::account_text(status);
        let code = status
            .signing_in
            .then(|| status.sign_in_url.as_deref().and_then(forms::sign_in_code))
            .flatten();
        let next = match code {
            Some(code) => div()
                .id((prefix, 31_usize))
                .debug_selector(move || format!("{prefix}-sign-in-code"))
                .role(Role::Status)
                .aria_label(SharedString::from(format!(
                    "Sign-in code of {}: {code}",
                    status.name
                )))
                .child(next)
                .into_any_element(),
            None => next.into_any_element(),
        };
        let account = row(
            row_text(title, Some(next), theme),
            controls(
                self.sign_in_controls(status, prefix, true, theme, cx),
                stacked,
            ),
            stacked,
        )
        .into_any_element();
        let mut groups = vec![
            header,
            group(
                group_heading("settings-account", "Account", theme),
                card(vec![account], theme),
            ),
        ];

        let runtime_controls = self.runtime_controls(status, prefix, true, theme, cx);
        let managed = status.runtime.is_some() || status.executable.is_some();
        if managed || !runtime_controls.is_empty() {
            let version: SharedString = match (&status.executable, &status.runtime) {
                (Some(_), _) => "Your own executable runs in place of the Crow Bot runtime.".into(),
                (None, Some(runtime)) if status.state == ProviderState::NotInstalled => {
                    format!("{runtime}. Crow Bot downloads it when a bot first needs it.").into()
                }
                (None, Some(runtime)) => format!("{runtime}, managed by Crow Bot.").into(),
                (None, None) => "Crow Bot installs no runtime for this provider.".into(),
            };
            let mut rows = vec![
                row(
                    row_text("Version", Some(version.into_any_element()), theme),
                    controls(runtime_controls, stacked),
                    stacked,
                )
                .into_any_element(),
            ];
            if managed {
                rows.push(self.render_executable_row(status, prefix, stacked, theme, window, cx));
            }
            groups.push(group(
                group_heading("settings-runtime", "Runtime", theme),
                card(rows, theme).debug_selector(move || format!("{prefix}-runtime")),
            ));
        }
        groups
    }

    /// One provider in the list: its mark, its name, and its account or its
    /// state. It opens the page of the provider.
    fn render_provider_row(
        &self,
        index: usize,
        status: &ProviderStatus,
        first: bool,
        last: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (state, _) = forms::provider_state(status, theme);
        let ready = status.state == ProviderState::Ready;
        let line: SharedString = match (&status.account, ready) {
            (Some(account), true) => account.clone().into(),
            (None, true) => "Signed in".into(),
            _ => forms::provider_problem(status).unwrap_or_else(|| state.into()),
        };
        let id = status.id.clone();
        let (hover, focus) = (theme.control_hover, theme.border_focus);
        div()
            .id(("settings-provider", index))
            .debug_selector(move || format!("provider-{index}"))
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(SharedString::from(format!(
                "{}: {state}. {line}",
                status.name
            )))
            .flex_none()
            .min_h(settings::ROW_MIN)
            // The edge that shows the focus counts in the padding.
            .px(space::S4 - size::HAIRLINE)
            .py(space::S3 - size::HAIRLINE)
            .flex()
            .items_center()
            .gap(space::S4)
            // The row keeps the corners of the card: a fill under the pointer
            // must not paint a square corner over them.
            .when(first, |row| row.rounded_t(radius::LG))
            .when(last, |row| row.rounded_b(radius::LG))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(focus))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_provider_settings(&id, window, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .size(settings::ROW_LOGO)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(provider_mark(status, settings::ROW_LOGO, theme)),
            )
            .child(
                row_text(
                    SharedString::from(status.name.clone()),
                    Some(
                        div()
                            .truncate()
                            .child(crate::truncated::truncated(line))
                            .into_any_element(),
                    ),
                    theme,
                )
                .flex_1(),
            )
            .children((!ready).then(|| state_badge(status, theme)))
            .child(ui::icon(
                Icon::ChevronRight,
                icon_size::MD,
                theme.text_muted,
            ))
            .into_any_element()
    }

    fn render_providers(
        &self,
        page: &SettingsPage,
        stacked: bool,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if self.providers.is_empty() {
            let waiting = div()
                .id("providers-loading")
                .debug_selector(|| "providers-loading".into())
                .role(Role::Status)
                .flex_none()
                .min_h(settings::ROW_MIN)
                .px(space::S4)
                .flex()
                .items_center()
                .gap(space::S2)
                .text_color(theme.text_muted)
                .children(
                    self.live()
                        .then(|| ui::spinner("providers-spinner", icon_size::SM, theme.text_muted)),
                )
                .child(if self.live() {
                    "Checking the providers…"
                } else {
                    "The providers show here once the daemon is back."
                })
                .into_any_element();
            return vec![card(vec![waiting], theme).flex_none().into_any_element()];
        }
        let open = page.provider.as_ref().and_then(|id| {
            self.providers
                .iter()
                .enumerate()
                .find(|(_, status)| &status.id == id)
        });
        if let Some((index, status)) = open {
            return self.render_provider_page(index, status, stacked, theme, window, cx);
        }
        let listed = |ready: bool, cx: &mut Context<Self>| {
            let shown: Vec<(usize, &ProviderStatus)> = self
                .providers
                .iter()
                .enumerate()
                .filter(|(_, status)| (status.state == ProviderState::Ready) == ready)
                .collect();
            let count = shown.len();
            shown
                .into_iter()
                .enumerate()
                .map(|(place, (index, status))| {
                    self.render_provider_row(
                        index,
                        status,
                        place == 0,
                        place + 1 == count,
                        theme,
                        cx,
                    )
                })
                .collect::<Vec<_>>()
        };
        let (connected, others) = (listed(true, cx), listed(false, cx));
        let mut groups = Vec::new();
        if !connected.is_empty() {
            groups.push(group(
                group_heading("settings-connected", "Connected", theme),
                card(connected, theme),
            ));
        }
        if !others.is_empty() {
            groups.push(group(
                group_heading("settings-not-connected", "Not connected", theme),
                card(others, theme),
            ));
        }
        groups
    }

    fn render_about(&self, theme: &Theme) -> Vec<AnyElement> {
        let unknown = || SharedString::from("Not known");
        let info = self.daemon_info.as_ref();
        let value_row = |id: &'static str, label: &'static str, value: SharedString| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .role(Role::Label)
                .aria_label(SharedString::from(format!("{label}: {value}")))
                .flex_none()
                .min_h(layout::PANEL_ROW + space::S2)
                .px(space::S4)
                .py(space::S3)
                .flex()
                .items_center()
                .gap(space::S4)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(text::TITLE.size)
                        .line_height(text::TITLE.leading)
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.text_secondary)
                        .child(value),
                )
                .into_any_element()
        };
        let versions = vec![
            value_row("about-client", "Crow Bot", env!("CARGO_PKG_VERSION").into()),
            value_row(
                "about-daemon",
                "Daemon",
                info.map_or_else(unknown, |info| info.daemon_version.clone().into()),
            ),
            value_row(
                "about-protocol",
                "Protocol",
                info.map_or_else(
                    || PROTOCOL_VERSION.to_string().into(),
                    |info| info.protocol_version.to_string().into(),
                ),
            ),
        ];
        let database = info.map_or_else(unknown, |info| info.database.clone().into());
        let storage = div()
            .id("about-database")
            .debug_selector(|| "about-database".into())
            .role(Role::Label)
            .aria_label(SharedString::from(format!("Database: {database}")))
            .child(row(
                row_text(
                    "Database",
                    Some(
                        div()
                            .font(font::mono())
                            .text_size(text::SM.size)
                            .child(database)
                            .into_any_element(),
                    ),
                    theme,
                ),
                None,
                false,
            ))
            .into_any_element();
        let mut storage = vec![storage];
        // A daemon whose computer does not keep the user's services running
        // with no session ends with the session. The client never changes that.
        if info.is_some_and(|info| info.linger == Some(false)) {
            storage.push(
                div()
                    .id("about-linger")
                    .debug_selector(|| "about-linger".into())
                    .role(Role::Label)
                    .aria_label(SharedString::from(format!(
                        "{LINGER_NOTICE} {LINGER_COMMAND}"
                    )))
                    .child(row(
                        row_text(
                            LINGER_NOTICE,
                            Some(
                                div()
                                    .font(font::mono())
                                    .text_size(text::SM.size)
                                    .child(LINGER_COMMAND)
                                    .into_any_element(),
                            ),
                            theme,
                        ),
                        None,
                        false,
                    ))
                    .into_any_element(),
            );
        }
        vec![
            group(
                group_heading("settings-versions", "Versions", theme),
                card(versions, theme),
            ),
            group(
                group_heading("settings-storage", "Storage", theme),
                card(storage, theme),
            ),
        ]
    }

    /// The column of sections: the title, the search, then the sections or
    /// the results of the search.
    fn render_settings_nav(
        &self,
        width: f32,
        page: &SettingsPage,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let (hover, focus, strong) = (theme.hover, theme.border_focus, theme.text);
        let handle = self.settings_query.focus_handle(cx);
        let search = ui::input_skin(
            div()
                .id("settings-search")
                .debug_selector(|| "settings-search".into())
                .flex_none()
                .h(control::MD)
                .mb(space::S3)
                .px(space::S3)
                .flex()
                .items_center()
                .gap(space::S2)
                .cursor_text(),
            handle.is_focused(window),
            false,
            theme,
        )
        .rounded_full()
        .on_click(move |_, window, cx| window.focus(&handle, cx))
        .child(ui::icon(Icon::Search, icon_size::SM, theme.text_muted))
        .child(div().flex_1().min_w_0().child(self.settings_query.clone()));
        let hits = self.settings_hits(cx);
        let rows: Vec<AnyElement> = match &hits {
            None => Section::ALL
                .into_iter()
                .map(|item| {
                    let chosen = item == page.section;
                    let id = item.selector();
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .group(NAV_ROW_GROUP)
                        .focusable()
                        .tab_stop(true)
                        .role(Role::Button)
                        .aria_label(SharedString::from(if chosen {
                            format!("{}, selected", item.label())
                        } else {
                            item.label().to_owned()
                        }))
                        .flex_none()
                        .h(settings::NAV_ROW)
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
                        .when(!chosen, |row| {
                            row.text_color(theme.text_secondary)
                                .hover(move |style| style.bg(hover).text_color(strong))
                        })
                        .focus_visible(move |style| style.border_color(focus))
                        .on_click(cx.listener(move |this, _, _, cx| this.show_section(item, cx)))
                        .child(
                            ui::icon(
                                item.icon(),
                                icon_size::MD,
                                if chosen { theme.text } else { theme.text_muted },
                            )
                            .group_hover(NAV_ROW_GROUP, move |style| style.text_color(strong)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(crate::truncated::truncated(item.label())),
                        )
                        .into_any_element()
                })
                .collect(),
            Some(hits) => hits
                .iter()
                .enumerate()
                .map(|(index, hit)| {
                    let active = index == self.settings_hit;
                    let opened = hit.clone();
                    div()
                        .id(("settings-result", index))
                        .debug_selector(move || format!("settings-result-{index}"))
                        .focusable()
                        .tab_stop(true)
                        .role(Role::Button)
                        .aria_label(SharedString::from(format!(
                            "{}: {}",
                            hit.section.label(),
                            hit.title
                        )))
                        .flex_none()
                        .px(settings::NAV_TEXT_INSET)
                        .py(settings::RESULT_PADDING_Y)
                        .flex()
                        .flex_col()
                        .gap(space::S0_5)
                        .rounded(radius::MD)
                        .border_1()
                        .border_color(gpui::transparent_black())
                        .cursor_pointer()
                        .when(active, |row| row.bg(theme.control_hover))
                        .when(!active, |row| row.hover(move |style| style.bg(hover)))
                        .focus_visible(move |style| style.border_color(focus))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_hit_setting(opened.clone(), window, cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S2)
                                .text_size(text::SM.size)
                                .line_height(text::SM.leading)
                                .text_color(theme.text_muted)
                                .child(ui::icon(
                                    hit.section.icon(),
                                    icon_size::SM,
                                    theme.text_muted,
                                ))
                                .child(hit.section.label()),
                        )
                        .child(
                            div()
                                .pl(icon_size::SM + space::S2)
                                .truncate()
                                .child(crate::truncated::truncated(hit.title.clone())),
                        )
                        .into_any_element()
                })
                .collect(),
        };
        // What a screen reader hears while the query changes.
        let status = hits.as_ref().map(|hits| {
            let query = self.settings_query.read(cx).text().trim().to_owned();
            let message: SharedString = match hits.len() {
                0 => format!("No setting matches “{query}”.").into(),
                1 => "1 result".into(),
                count => format!("{count} results").into(),
            };
            let empty = hits.is_empty();
            div()
                .id("settings-results-status")
                .debug_selector(move || {
                    if empty {
                        "settings-no-results".into()
                    } else {
                        "settings-results-status".into()
                    }
                })
                .role(Role::Status)
                .aria_label(message.clone())
                .flex_none()
                .when(empty, |status| {
                    status
                        .px(settings::NAV_TEXT_INSET)
                        .pt(space::S1)
                        .text_color(theme.text_muted)
                        .child(message)
                })
        });
        div()
            .flex_none()
            .w(px(width))
            .h_full()
            .px(settings::NAV_PADDING_X)
            .pt(settings::NAV_PADDING_TOP)
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("settings-heading")
                    .role(Role::Heading)
                    .aria_label("Settings")
                    .flex_none()
                    .px(settings::NAV_TEXT_INSET)
                    .pb(space::S3)
                    .text_size(text::XXL.size)
                    .line_height(text::XXL.leading)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Settings"),
            )
            .child(search)
            .child(
                div()
                    .id("settings-sections")
                    .debug_selector(|| "settings-sections".into())
                    .role(Role::List)
                    .aria_label(if hits.is_some() {
                        "Search results"
                    } else {
                        "Sections"
                    })
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb(space::S3)
                    .flex()
                    .flex_col()
                    .gap(settings::NAV_ROW_GAP)
                    .text_size(text::LG.size)
                    .line_height(text::LG.leading)
                    .children(status)
                    .children(rows),
            )
    }

    /// The leading end of the title band, over the section column: one
    /// button that goes one level up.
    pub(super) fn render_settings_topbar(
        &self,
        width: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = if self.settings_provider().is_some() {
            "Back to Providers"
        } else {
            "Back to bots"
        };
        div()
            .id("settings-topbar")
            .window_drag()
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .items_center()
            .pl(if cfg!(target_os = "macos") {
                layout::MAC_TRAFFIC_LIGHTS - layout::RAIL_WIDTH
            } else {
                layout::TOPBAR_PADDING_START
            })
            .child(
                div().occlude().child(
                    ui::icon_button("close-settings", Icon::ArrowLeft, label, theme).on_click(
                        cx.listener(|this, _, window, cx| this.settings_back(window, cx)),
                    ),
                ),
            )
    }

    /// The provider whose page is open, when one is.
    fn settings_provider(&self) -> Option<&ProviderStatus> {
        let page = self.settings.as_ref()?;
        if page.section != Section::Providers {
            return None;
        }
        let id = page.provider.as_ref()?;
        self.providers.iter().find(|status| &status.id == id)
    }

    /// The rest of the title band: the section shown, or the path to the
    /// page of a provider. The band still moves the window.
    pub(super) fn render_settings_path(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let path = div()
            .id("settings-path")
            .debug_selector(|| "settings-path".into())
            .window_drag()
            .flex_1()
            .min_w_0()
            .h_full()
            .pl(settings::PATH_INSET)
            .pr(space::S2)
            .flex()
            .items_center()
            .gap(space::S1)
            .text_size(text::LG.size)
            .line_height(text::LG.leading);
        let Some(page) = self.settings.as_ref() else {
            return path;
        };
        let current = |label: SharedString| {
            div()
                .id("settings-title")
                .debug_selector(|| "settings-title".into())
                .role(Role::Heading)
                .aria_label(label.clone())
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(crate::truncated::truncated(label))
        };
        let Some(status) = self.settings_provider() else {
            return path.child(current(page.section.label().into()));
        };
        let (hover, focus, strong) = (theme.hover, theme.border_focus, theme.text);
        path.child(
            div().occlude().flex_none().ml(-space::S1_5).child(
                div()
                    .id("settings-path-parent")
                    .debug_selector(|| "settings-path-parent".into())
                    .group(PATH_PARENT_GROUP)
                    .focusable()
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label("Providers")
                    .h(control::XS)
                    .px(space::S1_5)
                    .flex()
                    .items_center()
                    .rounded(radius::SM)
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .cursor_pointer()
                    .text_color(theme.text_muted)
                    .hover(move |style| style.bg(hover).text_color(strong))
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(|this, _, window, cx| this.settings_back(window, cx)))
                    .child(Section::Providers.label()),
            ),
        )
        .child(ui::icon(
            Icon::ChevronRight,
            icon_size::SM,
            theme.text_muted,
        ))
        .child(div().w(space::S0_5).flex_none())
        .child(current(status.name.clone().into()))
    }

    /// The settings page, in the card: the section column, then the groups
    /// of the section shown, in a centered column that scrolls.
    pub(super) fn render_settings_page(
        &self,
        nav_width: f32,
        window_width: f32,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let page = self.settings.as_ref()?;
        let pane = window_width
            - f32::from(layout::RAIL_WIDTH)
            - nav_width
            - f32::from(layout::CARD_INSET);
        let padding = if pane < settings::COMPACT_BELOW {
            space::S4
        } else {
            space::S6
        };
        let stacked = pane < settings::STACK_BELOW;
        let groups = match page.section {
            Section::General => self.render_general(page, stacked, theme, cx),
            Section::Providers => self.render_providers(page, stacked, theme, window, cx),
            Section::About => self.render_about(theme),
        };
        let content = div()
            .id("settings-body")
            .debug_selector(|| "settings-body".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(
                // What scrolls keeps its own height: a column that shrank to
                // the pane would hide its last rows instead of scrolling.
                div()
                    .flex_none()
                    .w_full()
                    .max_w(settings::COLUMN + padding * 2.)
                    .p(padding)
                    .flex()
                    .flex_col()
                    .gap(space::S6)
                    .children(groups),
            );
        Some(
            div()
                .id("settings-page")
                .debug_selector(|| "settings-page".into())
                .track_focus(&self.settings_focus)
                .role(Role::Group)
                .aria_label("Settings")
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.render_settings_nav(nav_width, page, theme, window, cx))
                .child(content),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(section: Section, title: &'static str, detail: &'static str) -> Hit {
        Hit {
            section,
            provider: None,
            title: title.into(),
            detail: detail.into(),
        }
    }

    fn titles(hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|hit| hit.title.to_string()).collect()
    }

    #[test]
    fn the_search_ranks_a_title_before_a_detail_and_ignores_letter_case() {
        let entries = || {
            vec![
                hit(Section::About, "Database", "Where the theme is not"),
                hit(Section::General, "Theme", "Light or dark colors"),
                hit(Section::Providers, "Providers", "Accounts and runtimes"),
            ]
        };
        assert_eq!(titles(&rank("THE", entries())), ["Theme", "Database"]);
        assert_eq!(titles(&rank("dark", entries())), ["Theme"]);
        // Every word must be found, and the section counts.
        assert_eq!(titles(&rank("general light", entries())), ["Theme"]);
        assert_eq!(rank("light about", entries()), Vec::<Hit>::new());
        assert_eq!(rank("   ", entries()), Vec::<Hit>::new());
    }

    #[test]
    fn each_select_lists_one_label_per_value() {
        assert_eq!(Select::Theme.options().len(), APPEARANCES.len());
        assert_eq!(Select::Send.options().len(), SHORTCUTS.len());
        assert_ne!(Select::Theme.index(), Select::Send.index());
    }
}
