//! The main application window and its root view.

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, Focusable as _, IntoElement, ParentElement,
    Pixels, Render, Styled, Subscription, TitlebarOptions, Window, WindowBounds, WindowDecorations,
    WindowOptions, div, px, size,
};
use ssherlock_core::app;

use crate::tabs::Pane;
use crate::theme::{ActiveTheme, Appearance, Theme};
use crate::window_frame::window_frame;

/// Size of the main window the first time it opens. Window geometry, not styling, so it lives
/// here rather than in the theme.
const DEFAULT_WIDTH: f32 = 1100.;
const DEFAULT_HEIGHT: f32 = 720.;
/// Smallest size the window can be resized to while staying usable.
const MIN_WIDTH: f32 = 480.;
const MIN_HEIGHT: f32 = 320.;

/// Root view of the main window: a pane of terminal tabs. The host list joins it in Phase 3.
pub struct MainWindow {
    pane: Entity<Pane>,
    _subscriptions: Vec<Subscription>,
}

impl MainWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Follow the system's light/dark setting while the app is running.
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            cx.set_global(Theme::for_appearance(Appearance::from_window(
                window.appearance(),
            )));
            cx.notify();
        });
        let pane = cx.new(|cx| Pane::new(window, cx));
        pane.focus_handle(cx).focus(window);
        // The window title follows the active tab.
        let title = cx.observe_in(&pane, window, |_, pane, window, cx| {
            let title = window_title(&pane.read(cx).title(cx));
            window.set_window_title(&title);
        });
        Self {
            pane,
            _subscriptions: vec![appearance, title],
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = div()
            .flex()
            .flex_1()
            .min_h_0()
            .bg(theme.colors.background)
            .child(self.pane.clone());

        let title = window_title(&self.pane.read(cx).title(cx));
        window_frame(window, &theme, title.into(), content)
    }
}

/// "`tab title` — SSHerlock".
fn window_title(tab: &str) -> String {
    format!("{tab} \u{2014} {}", app::NAME)
}

/// Opens the main window.
///
/// # Errors
///
/// Fails if the platform can't create a window, e.g. when no display server is reachable.
pub fn open_main_window(cx: &mut App) -> anyhow::Result<()> {
    let bounds = Bounds::centered(None, size(px(DEFAULT_WIDTH), px(DEFAULT_HEIGHT)), cx);
    cx.open_window(main_window_options(bounds), |window, cx| {
        tracing::info!(decorations = ?window.window_decorations(), "main window opened");
        cx.new(|cx| MainWindow::new(window, cx))
    })?;
    Ok(())
}

/// Options for the main window at `bounds`.
///
/// Client-side decorations are requested so that on Linux we always draw the frame in
/// [`window_frame`] ourselves. Asking for server-side ones left GNOME on Wayland with no title
/// bar at all: Mutter doesn't offer them, yet gpui still reported `Decorations::Server`. macOS
/// ignores this option and keeps its native title bar.
fn main_window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(app::NAME.into()),
            ..TitlebarOptions::default()
        }),
        app_id: Some(app::ID.to_owned()),
        window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
        window_decorations: Some(WindowDecorations::Client),
        ..WindowOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::*;

    /// Regression: requesting server-side decorations left GNOME on Wayland without a title bar.
    #[test]
    fn main_window_requests_client_side_decorations() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(800.), px(600.)));
        let options = main_window_options(bounds);
        assert!(matches!(
            options.window_decorations,
            Some(WindowDecorations::Client)
        ));
    }
}
