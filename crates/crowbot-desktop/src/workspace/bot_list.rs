use crate::chrome::WindowDrag;
use chrono::Local;
use crowbot_protocol::{BotState, BotSummary};
use gpui::accesskit::Orientation;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FontWeight, KeyDownEvent, MouseButton,
    MouseDownEvent, Role, SharedString, Stateful, anchored, deferred, div, point, prelude::*, px,
};

use super::{DragGhost, MenuTarget, ResizeSidebar, Workspace, row_id};
use crate::assets::Icon;
use crate::layout::Shown;
use crate::sidebar::{self, Group, GroupKind};
use crate::theme::{Theme, control, icon as icon_size, layout, radius, size, space, text};
use crate::ui;

const ROW_GROUP: &str = "bot-row";
const RESIZER_GROUP: &str = "sidebar-resizer";

fn opens_menu(event: &KeyDownEvent) -> bool {
    let keystroke = &event.keystroke;
    keystroke.key == "menu" || (keystroke.key == "f10" && keystroke.modifiers.shift)
}

impl Workspace {
    pub(super) fn render_sidebar_topbar(
        &self,
        shown: Shown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = if shown.compact {
            "Expand sidebar"
        } else {
            "Collapse sidebar"
        };
        div()
            .id("sidebar-topbar")
            .window_drag()
            .flex_none()
            .w(px(shown.width))
            .h_full()
            .flex()
            .items_center()
            .when(cfg!(target_os = "macos"), |bar| {
                bar.pl(layout::MAC_TRAFFIC_LIGHTS - layout::RAIL_WIDTH)
            })
            .when(shown.compact, |bar| bar.justify_center())
            .when(!shown.compact, |bar| bar.pl(layout::TOPBAR_PADDING_START))
            .child(
                div().occlude().child(
                    ui::icon_button("toggle-sidebar", Icon::PanelLeft, label, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                ),
            )
    }

    pub(super) fn render_sidebar(
        &self,
        shown: Shown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let content: Vec<AnyElement> = if self.bots.is_empty() {
            vec![self.render_empty_list(shown, theme, cx)]
        } else {
            sidebar::groups(&self.bots)
                .iter()
                .map(|group| self.render_group(group, shown, theme, cx))
                .collect()
        };
        div()
            .id("sidebar")
            .debug_selector(|| "sidebar".into())
            .flex_none()
            .w(px(shown.width))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme.border)
            .child(self.render_sidebar_actions(shown, theme, cx))
            .child(
                div()
                    .id("bots")
                    .debug_selector(|| "bots".into())
                    .role(Role::List)
                    .aria_label("Bots")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(space::S2)
                    .pt(space::S1)
                    .pb(space::S3)
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.open_menu(MenuTarget::Area, Some(event.position), window, cx);
                        }),
                    )
                    .children(content),
            )
    }

    /// The actions over the list, as round buttons: search everything on the
    /// leading edge, a new bot on the trailing edge, stacked in a compact
    /// sidebar.
    fn render_sidebar_actions(&self, shown: Shown, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let search = ui::round_button("open-search", Icon::Search, "Search everything", theme)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_palette(window, cx)));
        let create = ui::round_button("new-bot", Icon::Plus, "New bot", theme)
            .on_click(cx.listener(|this, _, window, cx| this.open_creation(window, cx)));
        div()
            .flex_none()
            .pt(layout::SIDEBAR_ACTIONS_TOP)
            // The same inset as the rows under them, so their edges line up.
            .px(space::S2)
            .pb(space::S1_5)
            .flex()
            .items_center()
            .gap(space::S2)
            .when(shown.compact, |row| row.flex_col().justify_center())
            .when(!shown.compact, |row| row.justify_between())
            .child(search)
            .child(create)
    }

    fn render_empty_list(&self, shown: Shown, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (hover, focus) = (theme.control_hover, theme.border_focus);
        let create = div()
            .id("create-first-bot")
            .debug_selector(|| "create-first-bot".into())
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label("Create your first bot")
            .h(layout::ROW_HEIGHT)
            .p(space::S2)
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded(radius::MD)
            .border_1()
            .border_color(gpui::transparent_black())
            .bg(theme.surface)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(focus))
            .when(shown.compact, |row| {
                row.justify_center()
                    .tooltip(ui::tooltip("Create your first bot"))
            })
            .on_click(cx.listener(|this, _, window, cx| this.open_creation(window, cx)))
            // The bot being made: the avatar follows the form as it changes.
            .child(if self.creation.is_some() {
                ui::living_avatar(&self.preview_bot(cx), size::AVATAR_ROW, theme, cx)
            } else {
                div()
                    .flex_none()
                    .size(size::AVATAR_ROW)
                    .rounded_full()
                    .border_1()
                    .border_dashed()
                    .border_color(theme.text_dim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::icon(Icon::Plus, icon_size::LG, theme.text_secondary))
            })
            .when(!shown.compact, |row| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(text::LG.size)
                        .line_height(text::LG.leading)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(crate::truncated::truncated("Create your first bot")),
                )
            });
        create.into_any_element()
    }

    fn render_group(
        &self,
        group: &Group,
        shown: Shown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let members: Vec<&BotSummary> = group
            .bots
            .iter()
            .filter_map(|bot_id| self.bot(*bot_id))
            .collect();
        let count = members.len();
        let heading_text = |title: &'static str| {
            div()
                .min_w_0()
                .truncate()
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .text_color(theme.text_readable)
                .child(crate::truncated::truncated(title))
        };
        let heading = |children: Div| {
            children
                .h(control::XS)
                .px(layout::SECTION_HEADER_PADDING)
                .flex()
                .items_center()
                .gap(space::S2)
        };
        let rows = |members: &[&BotSummary], cx: &mut Context<Self>| -> Vec<AnyElement> {
            members
                .iter()
                .map(|summary| self.render_row(summary, shown, theme, cx))
                .collect()
        };
        let section = div()
            .id(group.kind.title())
            .role(Role::Group)
            .aria_label(group.kind.title())
            .flex()
            .flex_col()
            .gap(space::S1);
        match group.kind {
            GroupKind::Pinned => section
                .child(
                    div()
                        .w_full()
                        .max_w(layout::PINNED_MAX_WIDTH)
                        .mx_auto()
                        .flex()
                        .flex_wrap()
                        .justify_center()
                        .gap(space::S2)
                        .children(
                            members
                                .iter()
                                .map(|summary| self.render_tile(summary, shown, theme, cx)),
                        ),
                )
                .into_any_element(),
            GroupKind::NeedsYou | GroupKind::Unread => {
                let badge = if group.kind == GroupKind::NeedsYou {
                    ui::count_badge(count, theme.warning, theme.on_status)
                } else {
                    ui::count_badge(count, theme.control_active, theme.text_secondary)
                };
                section
                    .pb(space::S2)
                    .border_b_1()
                    .border_color(theme.border)
                    .when(!shown.compact, |section| {
                        section.child(heading(
                            div().child(heading_text(group.kind.title())).child(badge),
                        ))
                    })
                    .children(rows(&members, cx))
                    .into_any_element()
            }
            GroupKind::Bots => section
                .when(!shown.compact, |section| {
                    section.child(heading(div().child(heading_text(group.kind.title()))))
                })
                .children(rows(&members, cx))
                .into_any_element(),
            GroupKind::Hidden => {
                let open = self.hidden_open;
                let (hover, focus) = (theme.text_secondary, theme.border_focus);
                let label = SharedString::from(format!("Hidden ({count})"));
                let action = if open {
                    format!("Collapse hidden bots ({count})")
                } else {
                    format!("Show hidden bots ({count})")
                };
                let toggle = div()
                    .id("toggle-hidden")
                    .debug_selector(|| "toggle-hidden".into())
                    .focusable()
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label(SharedString::from(action))
                    .h(control::XS)
                    .px(layout::SECTION_HEADER_PADDING)
                    .flex()
                    .items_center()
                    .gap(space::S1)
                    .rounded(radius::MD)
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_readable)
                    .cursor_pointer()
                    .hover(move |style| style.text_color(hover))
                    .focus_visible(move |style| style.border_color(focus))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.hidden_open = !this.hidden_open;
                        cx.notify();
                    }))
                    .child(ui::icon(
                        if open {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        icon_size::XS,
                        theme.text_muted,
                    ))
                    .when(!shown.compact, |toggle| toggle.child(label))
                    .when(shown.compact, |toggle| {
                        toggle
                            .justify_center()
                            .tooltip(ui::tooltip(format!("Hidden ({count})")))
                    });
                section
                    .child(toggle)
                    .when(open, |section| section.children(rows(&members, cx)))
                    .into_any_element()
            }
        }
    }

    fn avatar_with_badge(
        &self,
        summary: &BotSummary,
        size: gpui::Pixels,
        theme: &Theme,
        cx: &App,
    ) -> Div {
        div()
            .relative()
            .flex_none()
            .size(size)
            .child(ui::live_avatar(summary, size, theme, cx))
            .children(ui::state_badge(summary, theme.sidebar, theme))
    }

    fn bot_target(
        &self,
        id: impl Into<gpui::ElementId>,
        summary: &BotSummary,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let bot_id = summary.bot.id;
        let selected = self.selected_bot == Some(bot_id);
        let (hover, focus) = (theme.control_hover, theme.border_focus);
        let keyboard_menu = self
            .menu
            .as_ref()
            .is_some_and(|menu| menu.target == MenuTarget::Bot(bot_id) && menu.position.is_none());
        div()
            .id(id)
            .debug_selector(move || format!("bot-{bot_id}"))
            .group(ROW_GROUP)
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(SharedString::from(sidebar::row_label(summary)))
            .flex_none()
            .rounded(radius::MD)
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .focus_visible(move |style| style.border_color(focus))
            .when(selected, |row| row.bg(theme.surface))
            .when(!selected, |row| row.hover(move |style| style.bg(hover)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.open_bot(bot_id, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_menu(MenuTarget::Bot(bot_id), Some(event.position), window, cx);
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if opens_menu(event) {
                    cx.stop_propagation();
                    this.open_menu(MenuTarget::Bot(bot_id), None, window, cx);
                }
            }))
            .when(keyboard_menu, |row| {
                row.child(deferred(
                    anchored()
                        .snap_to_window_with_margin(space::S2)
                        .offset(point(size::AVATAR_ROW, size::AVATAR_ROW))
                        .child(self.render_menu(theme, cx)),
                ))
            })
    }

    fn render_row(
        &self,
        summary: &BotSummary,
        shown: Shown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = self
            .bot_target(("bot", row_id(summary.bot.id)), summary, theme, cx)
            .h(layout::ROW_HEIGHT)
            .p(space::S2)
            .flex()
            .items_center()
            .gap(space::S2)
            .child(self.avatar_with_badge(summary, size::AVATAR_ROW, theme, cx));
        if shown.compact {
            return row
                .justify_center()
                .tooltip(ui::tooltip(sidebar::row_label(summary)))
                .into_any_element();
        }
        let detail: AnyElement = if summary.state == BotState::Waiting {
            ui::wait_chip(theme).into_any_element()
        } else {
            div()
                .flex_none()
                .text_color(theme.text_readable)
                .child(SharedString::from(sidebar::activity_label(
                    &Local::now(),
                    summary.last_activity_ms,
                )))
                .into_any_element()
        };
        let preview: SharedString = if summary.preview.is_empty() {
            "No messages yet".into()
        } else {
            summary.preview.clone().into()
        };
        row.child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(text::TITLE.size)
                                .line_height(text::TITLE.leading)
                                .font_weight(FontWeight::MEDIUM)
                                .child(crate::truncated::truncated(summary.bot.name.clone())),
                        )
                        .child(detail),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(text::LG.size)
                        .line_height(text::LG.leading)
                        .text_color(theme.text_muted)
                        .child(crate::truncated::truncated(preview)),
                ),
        )
        .into_any_element()
    }

    fn render_tile(
        &self,
        summary: &BotSummary,
        shown: Shown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tile = self
            .bot_target(("pinned", row_id(summary.bot.id)), summary, theme, cx)
            .flex()
            .flex_col()
            .items_center();
        if shown.compact {
            return tile
                .size(layout::TILE_COMPACT)
                .justify_center()
                .tooltip(ui::tooltip(sidebar::row_label(summary)))
                .child(self.avatar_with_badge(summary, size::AVATAR_TILE_COMPACT, theme, cx))
                .into_any_element();
        }
        tile.w(layout::TILE_WIDTH)
            .h(layout::TILE_HEIGHT)
            .pt(space::S2)
            .px(space::S1)
            .gap(space::S1_5)
            .child(self.avatar_with_badge(summary, size::AVATAR_TILE, theme, cx))
            .child(
                div()
                    .w_full()
                    .truncate()
                    .text_center()
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .child(crate::truncated::truncated(summary.bot.name.clone())),
            )
            .into_any_element()
    }

    pub(super) fn render_resizer(
        &self,
        shown: Shown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (line, focus) = (theme.text_dim, theme.border_focus);
        let value_text: SharedString = if shown.compact {
            "Compact".into()
        } else {
            format!("{} pixels", shown.width.round()).into()
        };
        div()
            .id("sidebar-resizer")
            .debug_selector(|| "sidebar-resizer".into())
            .group(RESIZER_GROUP)
            .occlude()
            .focusable()
            .tab_stop(true)
            .role(Role::Splitter)
            .aria_label("Resize sidebar")
            .aria_orientation(Orientation::Vertical)
            .aria_numeric_value(f64::from(shown.width))
            .aria_min_numeric_value(f64::from(layout::SIDEBAR_COMPACT))
            .aria_max_numeric_value(f64::from(layout::SIDEBAR_MAX))
            .aria_value(value_text)
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(shown.width) - layout::RESIZER_HIT / 2.)
            .w(layout::RESIZER_HIT)
            .flex()
            .justify_center()
            .cursor_col_resize()
            .border_1()
            .border_color(gpui::transparent_black())
            .focus_visible(move |style| style.border_color(focus))
            .on_drag(ResizeSidebar, |_, _, _, cx| cx.new(|_| DragGhost))
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.sidebar.reset();
                    this.save_sidebar();
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.resizer_key(event, window, cx);
            }))
            .child(
                div()
                    .w(size::HAIRLINE)
                    .h_full()
                    .group_hover(RESIZER_GROUP, move |style| style.bg(line))
                    .when(self.resizing, |handle| handle.bg(line)),
            )
    }
}
