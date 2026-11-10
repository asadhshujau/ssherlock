//! The main application window and its root view.

use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, ParentElement, Render, Styled,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowDecorations, WindowOptions, div, px,
    size,
};
use ssherlock_core::app;

use crate::theme::{ActiveTheme, Appearance, Theme};
use crate::window_frame::window_frame;

/// Size of the main window the first time it opens. Window geometry, not styling, so it lives
/// here rather than in the theme.
const DEFAULT_WIDTH: f32 = 1100.;
const DEFAULT_HEIGHT: f32 = 720.;
/// Smallest size the window can be resized to while staying usable.
const MIN_WIDTH: f32 = 480.;
const MIN_HEIGHT: f32 = 320.;

/// Root view of the main window. For now it shows the app name; later phases put the host list
/// and terminal tabs here.
pub struct MainWindow {
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
        Self {
            _subscriptions: vec![appearance],
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .bg(theme.colors.background)
            .text_color(theme.colors.text)
            .text_size(theme.text.display_size)
            .child(app::NAME);

        window_frame(window, &theme, content)
    }
}

/// Opens the main window.
///
/// Server-side decorations are requested; compositors that don't offer them (GNOME on Wayland)
/// fall back to the frame drawn by [`window_frame`].
///
/// # Errors
///
/// Fails if the platform can't create a window, e.g. when no display server is reachable.
pub fn open_main_window(cx: &mut App) -> anyhow::Result<()> {
    let bounds = Bounds::centered(None, size(px(DEFAULT_WIDTH), px(DEFAULT_HEIGHT)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(app::NAME.into()),
            ..TitlebarOptions::default()
        }),
        app_id: Some(app::ID.to_owned()),
        window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
        window_decorations: Some(WindowDecorations::Server),
        ..WindowOptions::default()
    };
    cx.open_window(options, |window, cx| {
        cx.new(|cx| MainWindow::new(window, cx))
    })?;
    Ok(())
}
