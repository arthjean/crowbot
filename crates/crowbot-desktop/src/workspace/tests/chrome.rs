//! The window chrome of Linux: the title band moves the window, the window
//! buttons act, and the frame resizes it, from the real workspace.

use gpui::{Decorations, MouseDownEvent, MouseMoveEvent, Tiling};

use super::*;
use crate::chrome::WindowRequest;
use crate::chrome::tests::take_requests;
use crate::theme::chrome;

fn decorate(cx: &mut VisualTestContext, decorations: Decorations) {
    let window = cx.update(|window, _| window.window_handle());
    cx.simulate_window_decorations_change(window, decorations);
    cx.run_until_parked();
}

fn client(cx: &mut VisualTestContext) {
    decorate(
        cx,
        Decorations::Client {
            tiling: Tiling::default(),
        },
    );
}

fn opened(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (_, mut daemon, cx) = open(cx);
    daemon.connect(vec![summary(1, "Ada", BotState::Idle)], cx);
    client(cx);
    take_requests();
    cx
}

/// A point of the title band that holds no control: between the bot button
/// and the controls of the header.
fn empty_band(cx: &mut VisualTestContext) -> gpui::Point<Pixels> {
    let header = bounds(cx, "conversation-header");
    let bot = bounds(cx, "open-bot-settings");
    let model = bounds(cx, "current-model");
    let x = (bot.right() + model.left()) / 2.;
    assert!(x > bot.right() && x < model.left());
    point(x, header.center().y)
}

fn press(cx: &mut VisualTestContext, at: gpui::Point<Pixels>, clicks: usize) {
    cx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: clicks,
        first_mouse: false,
    });
}

fn drag_to(cx: &mut VisualTestContext, at: gpui::Point<Pixels>) {
    cx.simulate_event(MouseMoveEvent {
        position: at,
        modifiers: Modifiers::none(),
        pressed_button: Some(MouseButton::Left),
    });
}

#[gpui::test]
fn a_press_and_a_drag_on_the_title_band_move_the_window(cx: &mut TestAppContext) {
    let cx = opened(cx);
    let at = empty_band(cx);
    press(cx, at, 1);
    assert_eq!(take_requests(), [], "a press alone does not move");
    drag_to(cx, at + point(px(4.), px(0.)));
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    assert_eq!(take_requests(), [WindowRequest::Move]);

    // The rail's empty rows are drag zones too.
    let logo = bounds(cx, "rail-logo").center();
    press(cx, logo, 1);
    drag_to(cx, logo + point(px(0.), px(3.)));
    cx.simulate_mouse_up(logo, MouseButton::Left, Modifiers::none());
    assert_eq!(take_requests(), [WindowRequest::Move]);

    // A click that does not move does not start a move later.
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_mouse_move(at + point(px(9.), px(0.)), None, Modifiers::none());
    assert_eq!(take_requests(), []);
}

#[gpui::test]
fn a_double_click_on_the_title_band_maximizes_and_a_right_click_opens_the_window_menu(
    cx: &mut TestAppContext,
) {
    let cx = opened(cx);
    let at = empty_band(cx);
    press(cx, at, 1);
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    press(cx, at, 2);
    drag_to(cx, at + point(px(2.), px(0.)));
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    assert_eq!(take_requests(), [WindowRequest::Zoom]);

    right_click(cx, at);
    assert_eq!(take_requests(), [WindowRequest::Menu(at)]);
}

#[gpui::test]
fn a_control_inside_the_title_band_acts_and_does_not_move_the_window(cx: &mut TestAppContext) {
    let cx = opened(cx);
    let new_chat = bounds(cx, "new-chat").center();
    press(cx, new_chat, 1);
    drag_to(cx, new_chat + point(px(3.), px(0.)));
    cx.simulate_mouse_up(new_chat, MouseButton::Left, Modifiers::none());
    assert_eq!(take_requests(), []);

    let toggle = bounds(cx, "toggle-side-panel").center();
    press(cx, toggle, 2);
    cx.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::none());
    assert_eq!(
        take_requests(),
        [],
        "a double click on a control does not zoom"
    );
}

#[gpui::test]
fn the_window_buttons_show_in_the_layout_order_and_act(cx: &mut TestAppContext) {
    let cx = opened(cx);
    // The test platform has no portal: the default layout names minimize,
    // maximize, and close on the right.
    let minimize = bounds(cx, "window-minimize");
    let maximize = bounds(cx, "window-maximize");
    let close = bounds(cx, "window-close");
    assert!(minimize.right() <= maximize.left() && maximize.right() <= close.left());
    for button in [minimize, maximize, close] {
        assert_eq!(button.size.width, layout::CAPTION_BUTTON);
        assert_eq!(button.size.height, layout::TOP_ROW);
    }
    let surface = bounds(cx, "window-surface");
    assert_eq!(close.right(), surface.right() - chrome::BORDER);
    assert!(!shown(cx, "window-buttons-left"));

    for (button, request) in [
        ("window-minimize", WindowRequest::Minimize),
        ("window-maximize", WindowRequest::Zoom),
        ("window-close", WindowRequest::Close),
    ] {
        click(cx, button);
        assert_eq!(take_requests(), [request], "{button}");
    }
}

#[gpui::test]
fn the_frame_resizes_from_its_edges_and_keeps_a_shadow_margin(cx: &mut TestAppContext) {
    let cx = opened(cx);
    let viewport = cx.update(|window, _| window.viewport_size());
    let surface = bounds(cx, "window-surface");
    assert_eq!(surface.origin, point(chrome::SHADOW, chrome::SHADOW));
    assert_eq!(
        surface.size,
        size(
            viewport.width - chrome::SHADOW * 2.,
            viewport.height - chrome::SHADOW * 2.
        )
    );

    let band = chrome::SHADOW - px(3.);
    for (at, edge) in [
        (point(band, px(300.)), gpui::ResizeEdge::Left),
        (point(px(500.), band), gpui::ResizeEdge::Top),
        (
            point(viewport.width - band, viewport.height - band),
            gpui::ResizeEdge::BottomRight,
        ),
    ] {
        press(cx, at, 1);
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        assert_eq!(take_requests(), [WindowRequest::Resize(edge)], "{at:?}");
    }
    // The outer part of the margin is outside the input region.
    press(cx, point(px(2.), px(300.)), 1);
    assert_eq!(take_requests(), []);

    // A window tiled to its left edge has no margin and no resize there.
    decorate(
        cx,
        Decorations::Client {
            tiling: Tiling {
                left: true,
                ..Tiling::default()
            },
        },
    );
    assert_eq!(bounds(cx, "window-surface").left(), px(0.));
    press(cx, point(band, px(300.)), 1);
    assert_eq!(take_requests(), []);
}

#[gpui::test]
fn a_corner_that_reaches_over_a_control_resizes_and_the_control_does_not_act(
    cx: &mut TestAppContext,
) {
    let cx = opened(cx);
    let viewport = cx.update(|window, _| window.viewport_size());
    // A corner square reaches 5 px into the window, over the close button on
    // the right and the rail's logo, a drag zone, on the left.
    let inside = chrome::SHADOW + px(2.);
    let top_right = point(viewport.width - inside, inside);
    assert!(bounds(cx, "window-close").contains(&top_right));
    let top_left = point(inside, inside);
    assert!(bounds(cx, "rail-logo").contains(&top_left));

    for (at, edge) in [
        (top_right, gpui::ResizeEdge::TopRight),
        (top_left, gpui::ResizeEdge::TopLeft),
    ] {
        press(cx, at, 1);
        drag_to(cx, at + point(px(3.), px(3.)));
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        assert_eq!(take_requests(), [WindowRequest::Resize(edge)], "{at:?}");
    }
}

#[gpui::test]
fn the_chrome_follows_the_decorations_with_no_restart(cx: &mut TestAppContext) {
    let cx = opened(cx);
    assert!(shown(cx, "window-frame"));
    assert!(shown(cx, "window-close"));

    // The compositor decorates: the band keeps its content, the client draws
    // no button, no frame, and no resize zone.
    decorate(cx, Decorations::Server);
    assert!(!shown(cx, "window-frame"));
    assert!(!shown(cx, "window-close"));
    assert!(shown(cx, "conversation-header"));
    press(cx, point(px(3.), px(300.)), 1);
    assert_eq!(take_requests(), []);

    client(cx);
    assert!(shown(cx, "window-frame"));
    assert!(shown(cx, "window-close"));
}
