//! Everything on screen: gpui windows, views, components, theme and keymap.
//!
//! This is the **view** layer of each feature slice. Views hold state and render; business rules
//! live in `ssherlock-core` and I/O lives in the service and store crates.
#![forbid(unsafe_code)]

pub mod keymap;
pub mod main_window;
pub mod platform;
pub mod runtime;
pub mod terminal;
pub mod theme;
mod window_frame;

use gpui::App;
pub use main_window::open_main_window;
pub use runtime::TokioHandle;
pub use theme::{ActiveTheme, Theme};

use crate::theme::Appearance;

/// Sets up app-wide UI state. Call once, before opening any window.
pub fn init(tokio: tokio::runtime::Handle, cx: &mut App) {
    cx.set_global(TokioHandle::new(tokio));
    cx.set_global(Theme::for_appearance(Appearance::from_window(
        cx.window_appearance(),
    )));
    terminal::init(cx);
    // Last, so every feature's actions exist when their bindings are built.
    keymap::init(cx);

    // Closing the last window quits, on every platform. Lives as long as the app, so detached.
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use gpui::TestAppContext;
    use ssherlock_terminal::{ShellConfig, ShellProgram};

    use super::*;

    #[gpui::test]
    fn init_installs_globals_and_the_main_window_opens(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        cx.update(|cx| {
            init(runtime.handle().clone(), cx);
            assert!(cx.has_global::<Theme>());
            assert!(cx.has_global::<TokioHandle>());
            // Never start the user's real shell from a test: this program doesn't exist, so
            // the terminal shows its "could not start" message instead.
            cx.set_global(terminal::DefaultShell(ShellConfig {
                program: ShellProgram::Command {
                    program: "/nonexistent/ssherlock-test-shell".into(),
                    args: Vec::new(),
                },
                ..ShellConfig::default()
            }));

            open_main_window(cx).unwrap();
            assert_eq!(cx.windows().len(), 1);
        });
        // Let the first frame render, which exercises `MainWindow::render` and the frame.
        cx.run_until_parked();
    }
}
