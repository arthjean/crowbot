//! The window chrome the client draws itself on Linux.
//!
//! GNOME's compositor draws no title bar for a Wayland window, and GPUI draws
//! none either, so on Linux the client asks for client decorations and draws
//! them: the title band moves the window, the window buttons follow the
//! desktop's layout, and a frame gives the window its rounded outline, its
//! border, its shadow, and its resize edges. When the compositor decorates the
//! window after all (`Decorations::Server`: an X11 session with no compositing
//! manager), the client draws none of it and keeps the band's content.
//!
//! Windows and macOS keep their own chrome: there [`Frame::of`] answers
//! [`Frame::NATIVE`], and [`WindowDrag::window_drag`] only marks the area for
//! the platform's own hit test.

// The buttons and the frame are drawn on Linux only.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use gpui::{
    AnyElement, App, Bounds, BoxShadow, Corners, CursorStyle, Decorations, DispatchPhase, Div,
    Global, Hitbox, HitboxBehavior, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, ResizeEdge, Size, Stateful, Styled, Tiling, Window,
    WindowControlArea, WindowDecorations, canvas, div, point, prelude::*, px, size,
};

use crate::theme::{Theme, chrome, layout};

/// What the client asks of the platform for its window decorations.
pub fn requested_decorations() -> Option<WindowDecorations> {
    cfg!(target_os = "linux").then_some(WindowDecorations::Client)
}

/// The size the window opens at. On Linux the window holds the shadow margin
/// around its surface, so the margin is added: the window shows the default
/// size of `DESIGN.md`.
pub fn default_size() -> Size<Pixels> {
    with_margin(
        size(layout::WINDOW_WIDTH, layout::WINDOW_HEIGHT),
        cfg!(target_os = "linux"),
    )
}

/// The smallest size of the window. Wayland counts it in the window's
/// geometry, which leaves the shadow margin out; X11 counts the whole window,
/// margin included, so on X11 the margin is added and the window never shows
/// smaller than the minimum of `DESIGN.md`.
pub fn min_size(x11: bool) -> Size<Pixels> {
    with_margin(
        size(layout::WINDOW_MIN_WIDTH, layout::WINDOW_MIN_HEIGHT),
        x11,
    )
}

fn with_margin(window: Size<Pixels>, margin: bool) -> Size<Pixels> {
    if margin {
        let both = chrome::SHADOW * 2.;
        size(window.width + both, window.height + both)
    } else {
        window
    }
}

/// How the window is decorated in this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The client draws the chrome: buttons, outline, shadow, resize edges.
    pub client: bool,
    /// The edges that touch a screen edge or another window. A tiled edge
    /// has no shadow, no rounding, and no resize zone; a maximized or
    /// fullscreen window is tiled on every edge.
    pub tiling: Tiling,
}

impl Default for Frame {
    fn default() -> Self {
        Self::NATIVE
    }
}

impl Frame {
    /// The platform decorates the window, or draws its own chrome (Windows,
    /// macOS): the client draws no frame.
    pub const NATIVE: Self = Self {
        client: false,
        tiling: Tiling {
            top: true,
            left: true,
            right: true,
            bottom: true,
        },
    };

    pub fn of(window: &Window) -> Self {
        if !cfg!(target_os = "linux") {
            return Self::NATIVE;
        }
        Self::from_state(
            window.window_decorations(),
            window.is_maximized() || window.is_fullscreen(),
        )
    }

    fn from_state(decorations: Decorations, filled: bool) -> Self {
        match decorations {
            Decorations::Server => Self::NATIVE,
            Decorations::Client { .. } if filled => Self {
                client: true,
                tiling: Tiling::tiled(),
            },
            Decorations::Client { tiling } => Self {
                client: true,
                tiling,
            },
        }
    }

    /// The radius of each corner of the window's outline: rounded where
    /// both of its edges are free.
    pub fn corner_radii(self) -> Corners<Pixels> {
        self.corners(chrome::RADIUS)
    }

    /// The radius of each corner just inside the border, for a fill that
    /// covers the whole window (the scrim of a dialog).
    pub fn inner_radii(self) -> Corners<Pixels> {
        self.corners(chrome::RADIUS - chrome::BORDER)
    }

    fn corners(self, radius: Pixels) -> Corners<Pixels> {
        let Tiling {
            top,
            left,
            right,
            bottom,
        } = self.tiling;
        let at = |free: bool| if self.client && free { radius } else { px(0.) };
        Corners {
            top_left: at(!top && !left),
            top_right: at(!top && !right),
            bottom_right: at(!bottom && !right),
            bottom_left: at(!bottom && !left),
        }
    }

    /// Rounds `element` on the corners a full-window fill must keep inside
    /// the outline.
    pub fn round_inner<E: Styled>(self, element: E) -> E {
        let radii = self.inner_radii();
        element
            .rounded_tl(radii.top_left)
            .rounded_tr(radii.top_right)
            .rounded_br(radii.bottom_right)
            .rounded_bl(radii.bottom_left)
    }

    /// Wraps the content of the window in the frame: the transparent shadow
    /// margin and its resize edges, then the rounded surface with its border
    /// and shadow, on the `frame` color. Content inside must not paint a
    /// fill of its own over the whole window (`DESIGN.md` 4.3).
    pub fn wrap(self, content: AnyElement, theme: &Theme, window: &mut Window) -> AnyElement {
        if !cfg!(target_os = "linux") {
            return content;
        }
        if !self.client {
            window.set_client_inset(px(0.));
            window.set_client_corner_radii(Corners::default());
            set_input_region(window, None);
            return content;
        }
        window.set_client_inset(chrome::SHADOW);
        window.set_client_corner_radii(self.corner_radii());
        let tiling = self.tiling;
        let radii = self.corner_radii();
        let resizable = window.is_resizable();
        let region = input_region(window.viewport_size(), tiling, resizable);
        set_input_region(window, Some(region));
        let surface = div()
            .id("window-surface")
            .debug_selector(|| "window-surface".into())
            .size_full()
            .cursor(CursorStyle::Arrow)
            .rounded_tl(radii.top_left)
            .rounded_tr(radii.top_right)
            .rounded_br(radii.bottom_right)
            .rounded_bl(radii.bottom_left)
            .bg(theme.frame)
            .border_color(theme.dialog_ring)
            .when(!tiling.top, |surface| surface.border_t(chrome::BORDER))
            .when(!tiling.bottom, |surface| surface.border_b(chrome::BORDER))
            .when(!tiling.left, |surface| surface.border_l(chrome::BORDER))
            .when(!tiling.right, |surface| surface.border_r(chrome::BORDER))
            .when(!tiling.is_tiled(), |surface| {
                surface.shadow(vec![shadow(theme)])
            })
            .child(content);
        div()
            .id("window-frame")
            .debug_selector(|| "window-frame".into())
            .size_full()
            .when(!tiling.top, |frame| frame.pt(chrome::SHADOW))
            .when(!tiling.bottom, |frame| frame.pb(chrome::SHADOW))
            .when(!tiling.left, |frame| frame.pl(chrome::SHADOW))
            .when(!tiling.right, |frame| frame.pr(chrome::SHADOW))
            .when(resizable, |frame| {
                frame.on_mouse_move(move |event, window, cx| {
                    let edge = resize_edge(event.position, window.viewport_size(), tiling);
                    let shown = cx.try_global::<ShownEdge>().and_then(|shown| shown.0);
                    if edge != shown {
                        cx.set_global(ShownEdge(edge));
                        window.refresh();
                    }
                })
            })
            .child(surface)
            .when(resizable, |frame| frame.child(resize_cursor(tiling)))
            .into_any_element()
    }
}

/// The shadow of a floating window: it fades out inside the margin.
fn shadow(theme: &Theme) -> BoxShadow {
    let mut color = theme.shadow_menu;
    color.a *= 0.5;
    BoxShadow {
        color: color.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: chrome::SHADOW_BLUR,
        spread_radius: px(0.),
        inset: false,
    }
}

/// The resize edge last shown under the pointer, so that a move redraws
/// only when the cursor must change.
struct ShownEdge(Option<ResizeEdge>);

impl Global for ShownEdge {}

/// A layer over the whole window that sets the resize cursor where the
/// pointer is over a resize zone, and resizes on a press there. The press is
/// taken in the capture phase, before any element under it: a corner reaches
/// into the window, over the close button and the rail's drag zone, and a
/// press where the cursor shows a resize must resize, never act on them.
fn resize_cursor(tiling: Tiling) -> impl IntoElement {
    canvas(
        |_, window, _| {
            window.insert_hitbox(
                Bounds::new(Point::default(), window.viewport_size()),
                HitboxBehavior::Normal,
            )
        },
        move |_, hitbox: Hitbox, window, _| {
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                    return;
                }
                if let Some(edge) = resize_edge(event.position, window.viewport_size(), tiling) {
                    cx.stop_propagation();
                    request(window, cx, WindowRequest::Resize(edge));
                }
            });
            let Some(edge) = resize_edge(window.mouse_position(), window.viewport_size(), tiling)
            else {
                return;
            };
            window.set_cursor_style(resize_cursor_style(edge), &hitbox);
        },
    )
    .absolute()
    .size_full()
}

pub fn resize_cursor_style(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// The input region of the window, in window coordinates: the window and,
/// on its free edges, the resize band. The rest of the shadow margin takes
/// no input, so a click there reaches the window behind. A window that
/// cannot be resized takes input on the window alone.
pub fn input_region(window: Size<Pixels>, tiling: Tiling, resizable: bool) -> Bounds<Pixels> {
    let band = if resizable { chrome::RESIZE } else { px(0.) };
    let outside = |tiled: bool| {
        if tiled { px(0.) } else { chrome::SHADOW - band }
    };
    let (left, top) = (outside(tiling.left), outside(tiling.top));
    let (right, bottom) = (outside(tiling.right), outside(tiling.bottom));
    Bounds::new(
        point(left, top),
        size(
            (window.width - left - right).max(px(0.)),
            (window.height - top - bottom).max(px(0.)),
        ),
    )
}

fn set_input_region(window: &mut Window, region: Option<Bounds<Pixels>>) {
    INPUT_REGION.with(|last| {
        if last.get() != Some(region) {
            last.set(Some(region));
            match region {
                Some(bounds) => window.set_input_region(Some(&[bounds])),
                None => window.set_input_region(None),
            }
        }
    });
}

thread_local! {
    /// The input region last given to the platform (`None` inside: the
    /// whole window), so that a frame sends it only when it changes.
    static INPUT_REGION: std::cell::Cell<Option<Option<Bounds<Pixels>>>> =
        const { std::cell::Cell::new(None) };
}

/// The edge a press at `position` resizes, in a window of `size` that holds
/// the shadow margin on its free edges. The edges resize in the band of the
/// margin next to the window; the corners in a square that reaches into the
/// window. A tiled edge resizes nowhere, and neither does the margin outside
/// the band.
pub fn resize_edge(
    position: Point<Pixels>,
    size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let region = input_region(size, tiling, true);
    if !region.contains(&position) {
        return None;
    }
    let margin = chrome::RESIZE;
    let corner = chrome::RESIZE_CORNER;
    let (x, y) = (position.x - region.origin.x, position.y - region.origin.y);
    let (width, height) = (region.size.width, region.size.height);
    let near_left = !tiling.left && x < corner;
    let near_right = !tiling.right && x >= width - corner;
    let near_top = !tiling.top && y < corner;
    let near_bottom = !tiling.bottom && y >= height - corner;
    match (near_top, near_bottom, near_left, near_right) {
        (true, _, true, _) => return Some(ResizeEdge::TopLeft),
        (true, _, _, true) => return Some(ResizeEdge::TopRight),
        (_, true, true, _) => return Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => return Some(ResizeEdge::BottomRight),
        _ => {}
    }
    if !tiling.top && y < margin {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && y >= height - margin {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && x < margin {
        Some(ResizeEdge::Left)
    } else if !tiling.right && x >= width - margin {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

/// An action on the window that the platform carries out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowRequest {
    Move,
    Zoom,
    Minimize,
    Close,
    Menu(Point<Pixels>),
    Resize(ResizeEdge),
}

/// Carries out `request`. GPUI's test platform implements none of these, so
/// the tests record them instead.
pub fn request(window: &mut Window, cx: &mut App, request: WindowRequest) {
    #[cfg(test)]
    {
        let _ = (&window, &cx);
        tests::RECORDED.with_borrow_mut(|recorded| recorded.push(request));
    }
    #[cfg(not(test))]
    {
        let _ = cx;
        match request {
            WindowRequest::Move => window.start_window_move(),
            WindowRequest::Zoom => window.zoom_window(),
            WindowRequest::Minimize => window.minimize_window(),
            WindowRequest::Close => window.remove_window(),
            WindowRequest::Menu(position) => window.show_window_menu(position),
            WindowRequest::Resize(edge) => window.start_window_resize(edge),
        }
    }
}

/// A press on a drag zone that may become a move once the pointer moves.
/// The move waits for the first motion so that a double click still reaches
/// the zone: a move hands the pointer to the compositor.
struct MoveArmed(bool);

impl Global for MoveArmed {}

fn armed(cx: &App) -> bool {
    cx.try_global::<MoveArmed>().is_some_and(|armed| armed.0)
}

/// The empty parts of the title band, which move the window.
pub trait WindowDrag: InteractiveElement + Sized {
    /// Marks the element as a drag zone of the title band. Windows hit-tests
    /// the area itself. On Linux a press and a drag move the window, a double
    /// click maximizes or restores it, and a right click opens the window
    /// menu of the desktop. A control inside the zone that occludes it (all
    /// the band's controls do) takes its own presses.
    fn window_drag(self) -> Self {
        let zone = self.window_control_area(WindowControlArea::Drag);
        if cfg!(target_os = "linux") {
            linux_drag(zone)
        } else {
            zone
        }
    }
}

impl<E: InteractiveElement> WindowDrag for E {}

fn linux_drag<E: InteractiveElement>(zone: E) -> E {
    zone.on_mouse_down(MouseButton::Left, |event, window, cx| {
        cx.stop_propagation();
        if event.click_count >= 2 {
            cx.set_global(MoveArmed(false));
            if window.window_controls().maximize && window.is_resizable() {
                request(window, cx, WindowRequest::Zoom);
            }
        } else {
            cx.set_global(MoveArmed(true));
        }
    })
    .on_mouse_move(|event, window, cx| {
        if armed(cx) && event.pressed_button == Some(MouseButton::Left) {
            cx.set_global(MoveArmed(false));
            request(window, cx, WindowRequest::Move);
        }
    })
    .on_mouse_up(MouseButton::Left, |_, _, cx| {
        cx.set_global(MoveArmed(false))
    })
    .on_mouse_down(MouseButton::Right, |event, window, cx| {
        cx.stop_propagation();
        if window.window_controls().window_menu {
            request(window, cx, WindowRequest::Menu(event.position));
        }
    })
}

/// A side of the title band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A window button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

/// The Linux window buttons on `side`, in the order of the desktop's button
/// layout, among the ones the compositor allows. A desktop with no layout
/// (no portal) gets minimize, maximize, and close on the right. GNOME's
/// default layout names close alone.
#[cfg(target_os = "linux")]
pub fn buttons(window: &Window, cx: &App, side: Side) -> Vec<Button> {
    let layout = cx
        .button_layout()
        .unwrap_or_else(gpui::WindowButtonLayout::linux_default);
    let named = match side {
        Side::Left => layout.left,
        Side::Right => layout.right,
    };
    allowed(
        named.iter().flatten().copied().map(Button::from),
        window.window_controls(),
    )
}

#[cfg(target_os = "linux")]
impl From<gpui::WindowButton> for Button {
    fn from(button: gpui::WindowButton) -> Self {
        match button {
            gpui::WindowButton::Minimize => Self::Minimize,
            gpui::WindowButton::Maximize => Self::Maximize,
            gpui::WindowButton::Close => Self::Close,
        }
    }
}

/// Keeps the buttons the compositor allows. Close is always allowed.
pub fn allowed(named: impl Iterator<Item = Button>, controls: gpui::WindowControls) -> Vec<Button> {
    named
        .filter(|button| match button {
            Button::Minimize => controls.minimize,
            Button::Maximize => controls.maximize,
            Button::Close => true,
        })
        .collect()
}

/// One Linux window button: a 46 wide slot of the band, with a round hover
/// inside it, so no fill reaches the window's rounded corner.
pub fn button(button: Button, maximized: bool, theme: &Theme) -> Stateful<Div> {
    use crate::assets::Icon;
    use crate::theme::icon as icon_size;
    use crate::ui;

    let (id, label, glyph, request) = match button {
        Button::Minimize => (
            "window-minimize",
            "Minimize",
            Icon::WindowMinimize,
            WindowRequest::Minimize,
        ),
        Button::Maximize if maximized => (
            "window-maximize",
            "Restore",
            Icon::WindowRestore,
            WindowRequest::Zoom,
        ),
        Button::Maximize => (
            "window-maximize",
            "Maximize",
            Icon::WindowMaximize,
            WindowRequest::Zoom,
        ),
        Button::Close => (
            "window-close",
            "Close",
            Icon::WindowClose,
            WindowRequest::Close,
        ),
    };
    let hover = theme.hover;
    let group = id;
    div()
        .id(id)
        .debug_selector(move || id.into())
        .group(group)
        .occlude()
        .role(gpui::Role::Button)
        .aria_label(label)
        .flex_none()
        .w(layout::CAPTION_BUTTON)
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            self::request(window, cx, request);
        })
        .child(
            div()
                .size(chrome::BUTTON_HOVER)
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .group_hover(group, move |style| style.bg(hover))
                .child(ui::icon(glyph, icon_size::MD, theme.text_secondary)),
        )
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;

    use gpui::WindowControls;

    use super::*;

    thread_local! {
        pub(crate) static RECORDED: RefCell<Vec<WindowRequest>> = const { RefCell::new(Vec::new()) };
    }

    /// The window requests made since the last call.
    pub(crate) fn take_requests() -> Vec<WindowRequest> {
        RECORDED.with_borrow_mut(std::mem::take)
    }

    const WINDOW: Size<Pixels> = Size {
        width: px(800.),
        height: px(600.),
    };

    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    #[test]
    fn the_window_radius_follows_the_concentric_rule_from_the_card() {
        use crate::theme::{layout, radius};
        assert_eq!(
            chrome::RADIUS,
            radius::CARD + layout::CARD_INSET + chrome::BORDER
        );
    }

    #[test]
    fn each_edge_and_corner_of_a_floating_window_resizes() {
        let free = Tiling::default();
        // The margin is 16: its outer 6 take no input, the 10 next to the
        // window resize.
        let cases = [
            (at(8., 300.), Some(ResizeEdge::Left)),
            (at(792., 300.), Some(ResizeEdge::Right)),
            (at(400., 8.), Some(ResizeEdge::Top)),
            (at(400., 592.), Some(ResizeEdge::Bottom)),
            (at(8., 8.), Some(ResizeEdge::TopLeft)),
            (at(792., 8.), Some(ResizeEdge::TopRight)),
            (at(8., 592.), Some(ResizeEdge::BottomLeft)),
            (at(792., 592.), Some(ResizeEdge::BottomRight)),
            // A corner reaches 5 px into the window, an edge none.
            (at(20., 20.), Some(ResizeEdge::TopLeft)),
            (at(400., 17.), None),
            (at(17., 300.), None),
            (at(400., 300.), None),
            // The outer part of the margin resizes nowhere.
            (at(2., 300.), None),
            (at(2., 2.), None),
            (at(798., 598.), None),
        ];
        for (position, edge) in cases {
            assert_eq!(resize_edge(position, WINDOW, free), edge, "at {position:?}");
        }
    }

    #[test]
    fn the_input_region_is_the_window_and_its_resize_band() {
        let free = input_region(WINDOW, Tiling::default(), true);
        let outer = chrome::SHADOW - chrome::RESIZE;
        assert_eq!(free.origin, point(outer, outer));
        assert_eq!(
            free.size,
            size(WINDOW.width - outer * 2., WINDOW.height - outer * 2.)
        );
        // A window that cannot be resized takes input on itself alone.
        let fixed = input_region(WINDOW, Tiling::default(), false);
        assert_eq!(fixed.origin, point(chrome::SHADOW, chrome::SHADOW));
        // A tiled edge has no margin: the region reaches it.
        let left = Tiling {
            left: true,
            ..Tiling::default()
        };
        assert_eq!(input_region(WINDOW, left, true).left(), px(0.));
        assert_eq!(
            input_region(WINDOW, Tiling::tiled(), true),
            Bounds::new(Point::default(), WINDOW)
        );
    }

    #[test]
    fn a_tiled_edge_does_not_resize_and_its_free_edges_do() {
        let left = Tiling {
            left: true,
            ..Tiling::default()
        };
        assert_eq!(resize_edge(at(2., 300.), WINDOW, left), None);
        assert_eq!(resize_edge(at(2., 8.), WINDOW, left), Some(ResizeEdge::Top));
        assert_eq!(
            resize_edge(at(792., 300.), WINDOW, left),
            Some(ResizeEdge::Right)
        );
        for position in [at(2., 2.), at(400., 2.), at(798., 598.)] {
            assert_eq!(resize_edge(position, WINDOW, Tiling::tiled()), None);
        }
    }

    #[test]
    fn a_maximized_window_has_no_rounding_and_a_tiled_edge_loses_its_corners() {
        let floating = Frame::from_state(
            Decorations::Client {
                tiling: Tiling::default(),
            },
            false,
        );
        assert_eq!(floating.corner_radii(), Corners::all(chrome::RADIUS));
        assert_eq!(
            floating.inner_radii(),
            Corners::all(chrome::RADIUS - chrome::BORDER)
        );

        let maximized = Frame::from_state(
            Decorations::Client {
                tiling: Tiling::default(),
            },
            true,
        );
        assert!(maximized.client);
        assert_eq!(maximized.tiling, Tiling::tiled());
        assert_eq!(maximized.corner_radii(), Corners::all(px(0.)));

        let right = Frame::from_state(
            Decorations::Client {
                tiling: Tiling {
                    right: true,
                    ..Tiling::default()
                },
            },
            false,
        );
        let radii = right.corner_radii();
        assert_eq!(radii.top_left, chrome::RADIUS);
        assert_eq!(radii.bottom_left, chrome::RADIUS);
        assert_eq!(radii.top_right, px(0.));
        assert_eq!(radii.bottom_right, px(0.));
    }

    #[test]
    fn a_window_the_compositor_decorates_draws_no_chrome() {
        let frame = Frame::from_state(Decorations::Server, false);
        assert!(!frame.client);
        assert_eq!(frame.corner_radii(), Corners::all(px(0.)));
    }

    #[test]
    fn only_the_buttons_the_compositor_allows_show_in_the_layout_order() {
        let all = [Button::Close, Button::Minimize, Button::Maximize];
        assert_eq!(
            allowed(all.into_iter(), WindowControls::default()),
            all.to_vec()
        );
        let close_only = WindowControls {
            minimize: false,
            maximize: false,
            ..WindowControls::default()
        };
        assert_eq!(allowed(all.into_iter(), close_only), vec![Button::Close]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gnome_default_layout_names_close_alone_on_the_right() {
        let layout = gpui::WindowButtonLayout::parse("appmenu:close").unwrap();
        let right: Vec<Button> = layout
            .right
            .iter()
            .flatten()
            .copied()
            .map(Button::from)
            .collect();
        assert_eq!(right, vec![Button::Close]);
        assert!(layout.left.iter().flatten().next().is_none());
        let left = gpui::WindowButtonLayout::parse("close,minimize,maximize:").unwrap();
        let left: Vec<Button> = left
            .left
            .iter()
            .flatten()
            .copied()
            .map(Button::from)
            .collect();
        assert_eq!(
            left,
            vec![Button::Close, Button::Minimize, Button::Maximize]
        );
    }

    #[test]
    fn the_window_shows_the_sizes_of_the_design_inside_its_margin() {
        let margin = chrome::SHADOW * 2.;
        assert_eq!(
            min_size(false),
            size(layout::WINDOW_MIN_WIDTH, layout::WINDOW_MIN_HEIGHT)
        );
        assert_eq!(
            min_size(true),
            size(
                layout::WINDOW_MIN_WIDTH + margin,
                layout::WINDOW_MIN_HEIGHT + margin
            )
        );
        let extra = if cfg!(target_os = "linux") {
            margin
        } else {
            px(0.)
        };
        assert_eq!(
            default_size(),
            size(layout::WINDOW_WIDTH + extra, layout::WINDOW_HEIGHT + extra)
        );
    }

    #[test]
    fn a_window_with_no_flag_asks_for_its_decorations_only_on_linux() {
        assert_eq!(
            requested_decorations(),
            cfg!(target_os = "linux").then_some(WindowDecorations::Client)
        );
    }
}
