use crowbot_protocol::PermissionLevel;
use gpui::{
    AnyElement, Context, Div, FontWeight, KeyDownEvent, Role, SharedString, Stateful, anchored,
    deferred, div, point, prelude::*,
};

use super::{MenuTarget, Workspace, model_label};
use crate::assets::Icon;
use crate::theme::{
    Theme, control, font, icon as icon_size, layout, opacity, radius, size, space, text,
};
use crate::ui::{self, ButtonKind};

fn permission_label(level: PermissionLevel) -> &'static str {
    match level {
        PermissionLevel::AskFirst => "Ask first",
        PermissionLevel::FullAccess => "Full access",
    }
}

impl Workspace {
    pub(super) fn render_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let highlighted = self.menu.as_ref().and_then(|menu| menu.highlighted);
        let label = match self.menu.as_ref().map(|menu| menu.target) {
            Some(MenuTarget::Bot(bot_id)) => self
                .bot(bot_id)
                .map(|summary| format!("Actions for {}", summary.bot.name))
                .unwrap_or_default(),
            _ => "Bot list actions".to_owned(),
        };
        let (hover, danger_hover) = (theme.hover, theme.danger_soft);
        let items = self
            .menu_entries()
            .into_iter()
            .enumerate()
            .flat_map(|(index, entry)| {
                let separator = entry.separated.then(|| {
                    div()
                        .flex_none()
                        .h(size::HAIRLINE)
                        .my(space::S1)
                        .mx(-space::S1_5)
                        .bg(theme.border_strong)
                        .into_any_element()
                });
                let action = entry.action;
                let fill = if entry.danger { danger_hover } else { hover };
                let item = div()
                    .id(("menu-item", index))
                    .debug_selector(move || format!("menu-item-{index}"))
                    .role(Role::MenuItem)
                    .aria_label(entry.label)
                    .flex_none()
                    .h(control::MD)
                    .px(layout::MENU_ITEM_PADDING_X)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .rounded(radius::SM)
                    .when(entry.danger, |item| item.text_color(theme.danger_text))
                    .when(highlighted == Some(index), |item| item.bg(fill))
                    .when(!entry.enabled, |item| {
                        item.opacity(opacity::DISABLED)
                            .aria_description("Unavailable")
                    })
                    .when(entry.enabled, |item| {
                        item.cursor_pointer()
                            .hover(move |style| style.bg(fill))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.perform(action, window, cx);
                            }))
                    })
                    .child(entry.label)
                    .into_any_element();
                separator.into_iter().chain(std::iter::once(item))
            })
            .collect::<Vec<_>>();
        div()
            .id("context-menu")
            .debug_selector(|| "context-menu".into())
            .occlude()
            .track_focus(&self.menu_focus)
            .role(Role::Menu)
            .aria_label(SharedString::from(label))
            .min_w(layout::MENU_MIN_WIDTH)
            .max_w(layout::MENU_MAX_WIDTH)
            .p(space::S1_5)
            .flex()
            .flex_col()
            .rounded(radius::CARD)
            .bg(theme.sheet)
            .shadow(theme.menu_shadow())
            .text_color(theme.text)
            .text_size(text::LG.size)
            .line_height(text::LG.leading)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.menu_key(event, window, cx);
            }))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_menu(window, cx)))
            .children(items)
    }

    pub(super) fn render_floating_menu(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let position = match (menu.position, menu.target) {
            (Some(position), _) => position,
            (None, MenuTarget::Area) => point(space::S3, layout::TOP_ROW + control::XL),
            (None, MenuTarget::Bot(_)) => return None,
        };
        Some(
            deferred(
                anchored()
                    .position(position)
                    .snap_to_window_with_margin(space::S2)
                    .child(self.render_menu(theme, cx)),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    pub(super) fn render_delete_dialog(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let name = self.bot(self.confirm_delete?)?.bot.name.clone();
        let title = SharedString::from(format!("Delete {name}?"));
        let description = SharedString::from(format!(
            "This removes {name} and all of its conversations. The folder of the bot stays on disk. This cannot be undone."
        ));
        let dialog = div()
            .id("delete-dialog")
            .debug_selector(|| "delete-dialog".into())
            .occlude()
            .role(Role::AlertDialog)
            .aria_label(title.clone())
            .aria_description(description.clone())
            .w(layout::DIALOG_WIDTH)
            .max_w_full()
            .flex()
            .flex_col()
            .rounded(radius::XL)
            .bg(theme.canvas)
            .border_1()
            .border_color(theme.dialog_ring)
            .shadow(theme.menu_shadow())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_delete(window, cx);
                }
            }))
            .child(
                div()
                    .p(space::S6)
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .child(
                        div()
                            .text_size(text::XL.size)
                            .line_height(text::XL.leading)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(text::LG.size)
                            .line_height(text::LG.leading)
                            .text_color(theme.text_muted)
                            .child(description),
                    ),
            )
            .child(
                div()
                    .px(space::S6)
                    .py(space::S4)
                    .flex()
                    .justify_end()
                    .gap(space::S2)
                    .border_t_1()
                    .border_color(theme.border_strong)
                    .child(
                        ui::button("cancel-delete", "Cancel", ButtonKind::Ghost, theme)
                            .track_focus(&self.dialog_focus)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.cancel_delete(window, cx)),
                            ),
                    )
                    .child(
                        ui::button("confirm-delete", "Delete", ButtonKind::Danger, theme)
                            .track_focus(&self.dialog_confirm_focus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.delete_confirmed(window, cx);
                            })),
                    ),
            );
        Some(
            div()
                .id("dialog-overlay")
                .debug_selector(|| "dialog-overlay".into())
                .occlude()
                .absolute()
                .inset_0()
                .p(space::S6)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.overlay)
                .on_click(cx.listener(|this, _, window, cx| this.cancel_delete(window, cx)))
                .child(dialog)
                .into_any_element(),
        )
    }

    pub(super) fn render_full_access_dialog(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.confirm_full_access.as_ref()?;
        let name = self.selected_bot()?.bot.name.clone();
        let title = SharedString::from(format!("Always allow {name}?"));
        let description = SharedString::from(format!(
            "{name} moves to Full access. It then runs every tool without asking, in this conversation and in the next ones, and this request is allowed."
        ));
        let dialog = div()
            .id("full-access-dialog")
            .debug_selector(|| "full-access-dialog".into())
            .occlude()
            .role(Role::AlertDialog)
            .aria_label(title.clone())
            .aria_description(description.clone())
            .w(layout::DIALOG_WIDTH)
            .max_w_full()
            .flex()
            .flex_col()
            .rounded(radius::XL)
            .bg(theme.canvas)
            .border_1()
            .border_color(theme.dialog_ring)
            .shadow(theme.menu_shadow())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_full_access(window, cx);
                }
            }))
            .child(
                div()
                    .p(space::S6)
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .child(
                        div()
                            .text_size(text::XL.size)
                            .line_height(text::XL.leading)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .debug_selector(|| "full-access-description".into())
                            .text_size(text::LG.size)
                            .line_height(text::LG.leading)
                            .text_color(theme.text_muted)
                            .child(description),
                    ),
            )
            .child(
                div()
                    .px(space::S6)
                    .py(space::S4)
                    .flex()
                    .justify_end()
                    .gap(space::S2)
                    .border_t_1()
                    .border_color(theme.border_strong)
                    .child(
                        ui::button("cancel-full-access", "Cancel", ButtonKind::Ghost, theme)
                            .track_focus(&self.full_access_focus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_full_access(window, cx);
                            })),
                    )
                    .child(
                        ui::button(
                            "confirm-full-access",
                            "Move to Full access",
                            ButtonKind::Primary,
                            theme,
                        )
                        .track_focus(&self.full_access_confirm_focus)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.full_access_confirmed(window, cx);
                        })),
                    ),
            );
        Some(
            div()
                .id("full-access-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .p(space::S6)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.overlay)
                .on_click(cx.listener(|this, _, window, cx| this.cancel_full_access(window, cx)))
                .child(dialog)
                .into_any_element(),
        )
    }

    pub(super) fn render_toast(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.notice.clone()?;
        Some(
            div()
                .absolute()
                .top(layout::TOP_ROW + space::S2)
                .left_0()
                .right_0()
                .px(space::S4)
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("notice")
                        .debug_selector(|| "notice".into())
                        .occlude()
                        .role(Role::Alert)
                        .aria_label(notice.clone())
                        .w(layout::TOAST_WIDTH)
                        .max_w_full()
                        .p(space::S3)
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .rounded(radius::LG)
                        .bg(theme.sheet)
                        .shadow(theme.menu_shadow())
                        .child(ui::icon(
                            Icon::TriangleAlert,
                            icon_size::MD,
                            theme.danger_text,
                        ))
                        .child(div().flex_1().min_w_0().child(notice))
                        .child(
                            ui::icon_button("dismiss-notice", Icon::X, "Dismiss", theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.notice = None;
                                    cx.notify();
                                }),
                            ),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_side_panel(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.side_panel {
            return None;
        }
        let bot = &self.selected_bot()?.bot;
        let field = |label: &'static str, value: Div| {
            div()
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(
                    div()
                        .text_size(text::SM.size)
                        .line_height(text::SM.leading)
                        .text_color(theme.text_muted)
                        .child(label),
                )
                .child(value)
        };
        let plain = |value: SharedString| div().child(value);
        let or = |value: &str, missing: &'static str| -> SharedString {
            if value.trim().is_empty() {
                missing.into()
            } else {
                value.to_owned().into()
            }
        };
        Some(
            div()
                .id("side-panel")
                .debug_selector(|| "side-panel".into())
                .occlude()
                .role(Role::Group)
                .aria_label("Bot settings")
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(layout::SIDE_PANEL)
                .max_w_full()
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
                        .p(space::S4)
                        .flex()
                        .flex_col()
                        .gap(space::S4)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S3)
                                .child(ui::avatar(bot, size::AVATAR_DIALOG, text::XXL.size, theme))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(text::XL.size)
                                                .line_height(text::XL.leading)
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(SharedString::from(bot.name.clone())),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_color(theme.text_muted)
                                                .child(or(&bot.title, "No title")),
                                        ),
                                ),
                        )
                        .child(field(
                            "Instructions",
                            plain(or(&bot.instructions, "No instructions")),
                        ))
                        .child(field("Model", plain(model_label(&bot.model))))
                        .child(field("Provider", plain(or(&bot.provider, "Not set"))))
                        .child(field(
                            "Permissions",
                            plain(permission_label(bot.permission_level).into()),
                        ))
                        .child(field(
                            "Folder",
                            plain(or(&bot.workspace, "Not set")).font(font::mono()),
                        )),
                )
                .into_any_element(),
        )
    }
}
