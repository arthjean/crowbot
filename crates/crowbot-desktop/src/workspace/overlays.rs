use gpui::{
    AnyElement, Context, Div, FontWeight, KeyDownEvent, Role, SharedString, Stateful, anchored,
    deferred, div, point, prelude::*,
};

use super::{Confirm, MenuTarget, Workspace};
use crate::assets::Icon;
use crate::theme::{Theme, control, icon as icon_size, layout, menu, radius, space, text};
use crate::ui::{self, ButtonKind};

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
        let (hover, danger_hover) = (theme.menu_highlight, theme.danger_soft);
        let items = self
            .menu_entries()
            .into_iter()
            .enumerate()
            .flat_map(|(index, entry)| {
                let separator = entry
                    .separated
                    .then(|| ui::menu_separator(theme).into_any_element());
                let action = entry.action;
                let fill = if entry.danger { danger_hover } else { hover };
                let item = ui::menu_row_shape(("menu-item", index), false)
                    .debug_selector(move || format!("menu-item-{index}"))
                    .role(Role::MenuItem)
                    .aria_label(entry.label)
                    .when(entry.danger, |item| item.text_color(theme.danger_text))
                    .when(highlighted == Some(index), |item| item.bg(fill))
                    .when(!entry.enabled, |item| {
                        item.opacity(menu::DISABLED_OPACITY)
                            .cursor_default()
                            .aria_description("Unavailable")
                    })
                    .when(entry.enabled, |item| {
                        item.hover(move |style| style.bg(fill))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.perform(action, window, cx);
                            }))
                    })
                    .child(entry.label)
                    .into_any_element();
                separator.into_iter().chain(std::iter::once(item))
            })
            .collect::<Vec<_>>();
        ui::menu_panel(false, theme)
            .id("context-menu")
            .debug_selector(|| "context-menu".into())
            .occlude()
            .track_focus(&self.menu_focus)
            .role(Role::Menu)
            .aria_label(SharedString::from(label))
            .min_w(layout::MENU_MIN_WIDTH)
            .max_w(layout::MENU_MAX_WIDTH)
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
            self.frame
                .round_inner(div())
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
            self.frame
                .round_inner(div())
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

    pub(super) fn render_confirm_dialog(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let confirm = self.confirm?;
        let (Confirm::DeleteHistory(bot_id) | Confirm::FullAccess(bot_id)) = confirm;
        let name = self.bot(bot_id)?.bot.name.clone();
        let (title, description, action, kind) = match confirm {
            Confirm::DeleteHistory(_) => (
                format!("Delete the history of {name}?"),
                format!(
                    "This removes every conversation of {name}, with all of their messages. The bot, its settings, and its folder stay. This cannot be undone."
                ),
                "Delete history",
                ButtonKind::Danger,
            ),
            Confirm::FullAccess(_) => (
                format!("Give {name} full access?"),
                format!(
                    "{name} then runs every tool without asking, in this conversation and in the next ones. The requests that wait now are allowed."
                ),
                "Move to Full access",
                ButtonKind::Primary,
            ),
        };
        let (title, description) = (SharedString::from(title), SharedString::from(description));
        let dialog = div()
            .id("confirm-dialog")
            .debug_selector(|| "confirm-dialog".into())
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
                    this.cancel_confirm(window, cx);
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
                            .debug_selector(|| "confirm-description".into())
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
                        ui::button("confirm-cancel", "Cancel", ButtonKind::Ghost, theme)
                            .track_focus(&self.confirm_focus)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.cancel_confirm(window, cx)),
                            ),
                    )
                    .child(
                        ui::button("confirm-ok", action, kind, theme)
                            .track_focus(&self.confirm_ok_focus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirmed(window, cx);
                            })),
                    ),
            );
        Some(
            self.frame
                .round_inner(div())
                .id("confirm-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .p(space::S6)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.overlay)
                .on_click(cx.listener(|this, _, window, cx| this.cancel_confirm(window, cx)))
                .child(dialog)
                .into_any_element(),
        )
    }
}
