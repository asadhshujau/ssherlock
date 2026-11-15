//! Window chrome for compositors that make the app draw its own (client-side decorations).
//!
//! The main window asks for client-side decorations, so on Linux (X11 and Wayland alike) gpui
//! reports [`Decorations::Client`] and we draw a title bar, window controls, a shadow and
//! invisible resize grips around the content. macOS keeps its native title bar and reports
//! [`Decorations::Server`], which gets a plain root.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Bounds, BoxShadow, CursorStyle, Decorations, Div, HitboxBehavior,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, Point,
    ResizeEdge, SharedString, Size, StatefulInteractiveElement as _, Styled as _, Tiling, Window,
    canvas, div,
};

use crate::theme::Theme;

/// Wraps a window's content in whatever chrome the platform needs.
pub fn window_frame(
    window: &mut Window,
    theme: &Theme,
    title: SharedString,
    content: impl IntoElement,
) -> AnyElement {
    let root = div()
        .size_full()
        .flex()
        .flex_col()
        .font_family(theme.text.ui_family.clone())
        .text_size(theme.text.ui_size);

    match window.window_decorations() {
        Decorations::Server => root.child(content).into_any_element(),
        Decorations::Client { tiling } => {
            client_frame(window, theme, title, tiling, root.child(content)).into_any_element()
        }
    }
}

/// Shadow, border, rounded corners, resize grips and title bar around `content`.
fn client_frame(
    window: &mut Window,
    theme: &Theme,
    title: SharedString,
    tiling: Tiling,
    content: impl IntoElement,
) -> impl IntoElement {
    let inset = theme.metrics.window_inset;
    let border = theme.metrics.border_width;
    let radius = theme.radius.window;
    let colors = &theme.colors;
    window.set_client_inset(inset);

    div()
        .id("window-backdrop")
        .size_full()
        // The inset is drawn only on sides that aren't tiled against a screen edge or another
        // window; tiled sides have no shadow and no grip.
        .when(!tiling.top, |d| d.pt(inset))
        .when(!tiling.bottom, |d| d.pb(inset))
        .when(!tiling.left, |d| d.pl(inset))
        .when(!tiling.right, |d| d.pr(inset))
        .child(resize_grip_cursor(inset, tiling))
        // The cursor is chosen at paint time, so repaint as the pointer crosses the grips.
        .on_mouse_move(|_, window, _| window.refresh())
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(event.position, inset, size, tiling) {
                window.start_window_resize(edge);
            }
        })
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .cursor(CursorStyle::Arrow)
                .bg(colors.background)
                .border_color(colors.border)
                .when(!tiling.top, |d| d.border_t(border))
                .when(!tiling.bottom, |d| d.border_b(border))
                .when(!tiling.left, |d| d.border_l(border))
                .when(!tiling.right, |d| d.border_r(border))
                .when(!(tiling.top || tiling.left), |d| d.rounded_tl(radius))
                .when(!(tiling.top || tiling.right), |d| d.rounded_tr(radius))
                .when(!(tiling.bottom || tiling.left), |d| d.rounded_bl(radius))
                .when(!(tiling.bottom || tiling.right), |d| d.rounded_br(radius))
                .when(!tiling.is_tiled(), |d| {
                    d.shadow(vec![BoxShadow {
                        color: colors.window_shadow,
                        offset: Point::default(),
                        blur_radius: inset / 2.,
                        spread_radius: Pixels::ZERO,
                    }])
                })
                // Inside the window there are no grips, so no need to repaint on every move.
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(title_bar(window, theme, title))
                .child(content),
        )
}

/// Paints nothing; shows a resize cursor while the pointer is over a grip.
fn resize_grip_cursor(inset: Pixels, tiling: Tiling) -> impl IntoElement {
    canvas(
        |_bounds, window, _cx| {
            let size = window.window_bounds().get_bounds().size;
            window.insert_hitbox(Bounds::new(Point::default(), size), HitboxBehavior::Normal)
        },
        move |_bounds, hitbox, window, _cx| {
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(window.mouse_position(), inset, size, tiling) {
                window.set_cursor_style(resize_cursor(edge), &hitbox);
            }
        },
    )
    .size_full()
    .absolute()
}

/// Title bar: drag to move, double-click to maximise, right-click for the window menu.
fn title_bar(window: &Window, theme: &Theme, title: SharedString) -> impl IntoElement {
    let controls = window.window_controls();
    let colors = &theme.colors;
    let background = if window.is_window_active() {
        colors.title_bar
    } else {
        colors.title_bar_inactive
    };

    div()
        .id("title-bar")
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .h(theme.metrics.title_bar_height)
        .pl(theme.spacing.lg)
        .pr(theme.spacing.sm)
        .bg(background)
        .border_b(theme.metrics.border_width)
        .border_color(colors.border)
        .text_size(theme.text.small_size)
        .text_color(colors.text_muted)
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .when(controls.window_menu, |d| {
            d.on_mouse_down(MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position);
            })
        })
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(title),
        )
        .child(
            div()
                .flex()
                .gap(theme.spacing.xs)
                .when(controls.minimize, |d| {
                    d.child(window_control(WindowControl::Minimize, theme))
                })
                .when(controls.maximize, |d| {
                    d.child(window_control(WindowControl::Maximize, theme))
                })
                .child(window_control(WindowControl::Close, theme)),
        )
}

/// The buttons at the end of a client-drawn title bar.
#[derive(Clone, Copy, Debug)]
enum WindowControl {
    Minimize,
    Maximize,
    Close,
}

fn window_control(control: WindowControl, theme: &Theme) -> impl IntoElement {
    let colors = &theme.colors;
    let metrics = &theme.metrics;
    let (id, hover_background, hover_text) = match control {
        WindowControl::Minimize => ("minimize", colors.control_hover, colors.text),
        WindowControl::Maximize => ("maximize", colors.control_hover, colors.text),
        WindowControl::Close => (
            "close",
            colors.control_close_hover,
            colors.control_close_text,
        ),
    };
    // Glyphs are drawn with boxes rather than font characters, which the system UI font may
    // not contain. Close uses U+00D7, present in every Latin font.
    let glyph: Div = match control {
        WindowControl::Minimize => div()
            .w(metrics.window_control_glyph)
            .h(metrics.border_width)
            .bg(colors.text_muted),
        WindowControl::Maximize => div()
            .size(metrics.window_control_glyph)
            .border(metrics.border_width)
            .border_color(colors.text_muted),
        WindowControl::Close => div().text_size(theme.text.ui_size).child("\u{d7}"),
    };

    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(metrics.window_control_size)
        .rounded(theme.radius.sm)
        .text_color(colors.text_muted)
        .hover(move |style| style.bg(hover_background).text_color(hover_text))
        // Keep the title bar from starting a window move under the button.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, _| match control {
            WindowControl::Minimize => window.minimize_window(),
            WindowControl::Maximize => window.zoom_window(),
            WindowControl::Close => window.remove_window(),
        })
        .child(glyph)
}

/// Which resize grip, if any, is under `position`.
///
/// Grips are the `inset`-wide band around the window on every side that isn't tiled; corners
/// win over edges so diagonal resizing is easy to hit.
fn resize_edge(
    position: Point<Pixels>,
    inset: Pixels,
    size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let top = !tiling.top && position.y < inset;
    let bottom = !tiling.bottom && position.y > size.height - inset;
    let left = !tiling.left && position.x < inset;
    let right = !tiling.right && position.x > size.width - inset;

    match (top, bottom, left, right) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (true, _, _, true) => Some(ResizeEdge::TopRight),
        (_, true, true, _) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, ..) => Some(ResizeEdge::Top),
        (_, true, ..) => Some(ResizeEdge::Bottom),
        (_, _, true, _) => Some(ResizeEdge::Left),
        (_, _, _, true) => Some(ResizeEdge::Right),
        _ => None,
    }
}

fn resize_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn resize_edge_finds_grips_on_an_untiled_window() {
        let window = size(px(800.), px(600.));
        let inset = px(10.);
        let cases = [
            ("top-left corner", (2., 2.), Some(ResizeEdge::TopLeft)),
            ("top-right corner", (798., 2.), Some(ResizeEdge::TopRight)),
            (
                "bottom-left corner",
                (2., 598.),
                Some(ResizeEdge::BottomLeft),
            ),
            (
                "bottom-right corner",
                (798., 598.),
                Some(ResizeEdge::BottomRight),
            ),
            ("top edge", (400., 5.), Some(ResizeEdge::Top)),
            ("bottom edge", (400., 595.), Some(ResizeEdge::Bottom)),
            ("left edge", (5., 300.), Some(ResizeEdge::Left)),
            ("right edge", (795., 300.), Some(ResizeEdge::Right)),
            ("centre", (400., 300.), None),
            ("just inside the inset", (10., 10.), None),
        ];

        for (name, (x, y), expected) in cases {
            let edge = resize_edge(point(px(x), px(y)), inset, window, Tiling::default());
            assert_eq!(edge, expected, "{name}");
        }
    }

    #[test]
    fn tiled_sides_have_no_grip() {
        let window = size(px(800.), px(600.));
        let inset = px(10.);
        let left_half = Tiling {
            top: true,
            left: true,
            bottom: true,
            right: false,
        };
        let cases = [
            ("top-left corner of a tiled window", (2., 2.), None),
            ("title bar along a tiled top", (400., 5.), None),
            ("free right edge", (795., 300.), Some(ResizeEdge::Right)),
            (
                "right edge near a tiled corner",
                (795., 2.),
                Some(ResizeEdge::Right),
            ),
        ];

        for (name, (x, y), expected) in cases {
            let edge = resize_edge(point(px(x), px(y)), inset, window, left_half);
            assert_eq!(edge, expected, "{name}");
        }
        assert_eq!(
            resize_edge(point(px(2.), px(2.)), inset, window, Tiling::tiled()),
            None
        );
    }
}
