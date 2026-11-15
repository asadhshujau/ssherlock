//! Terminals on screen: the **view** layer of the terminal feature.
//!
//! [`TerminalView`] shows one terminal and forwards keyboard, mouse and paste input to it.
//! Emulation and I/O live in `ssherlock-terminal`; this module only paints snapshots and turns
//! gpui events into terminal input.

pub mod actions;
mod element;
mod keys;
pub mod view;

use gpui::{App, Global};
use ssherlock_terminal::ShellConfig;
pub use view::{TerminalView, TerminalViewEvent};

use crate::theme::ActiveTheme as _;

/// The shell new local terminals run. Absent means the user's login shell; settings (APP-02)
/// will set it, and tests set it so they never start the user's real shell.
#[derive(Clone, Debug, Default)]
pub struct DefaultShell(pub ShellConfig);

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn zoom_steps_change_the_size_within_limits() {
        let cases = [
            (14., 0, 14.),
            (14., 3, 17.),
            (14., -4, 10.),
            (14., -100, ZOOM_LIMITS.0),
            (14., 100, ZOOM_LIMITS.1),
        ];
        for (base, steps, expected) in cases {
            assert!(
                (zoomed_size(base, steps) - expected).abs() < f32::EPSILON,
                "{steps}"
            );
        }
        // At a limit, further steps are refused so zooming back is immediate.
        assert_eq!(zoom_by(14., 58, 1), 58);
        assert_eq!(zoom_by(14., 58, -1), 57);
        assert_eq!(zoom_by(14., -8, -1), -8);
    }
}

impl Global for DefaultShell {}

/// How many steps terminal text is zoomed in (positive) or out, shared by every terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalZoom(pub i32);

impl Global for TerminalZoom {}

/// Pixels added to the text size per zoom step.
const ZOOM_STEP: f32 = 1.;
/// Smallest and largest terminal text size, whatever the zoom.
const ZOOM_LIMITS: (f32, f32) = (6., 72.);

/// The terminal text size for a theme size of `base` pixels zoomed by `steps`.
pub fn zoomed_size(base: f32, steps: i32) -> f32 {
    #[allow(
        clippy::cast_precision_loss,
        reason = "zoom steps are a few dozen at most"
    )]
    let size = base + steps as f32 * ZOOM_STEP;
    size.clamp(ZOOM_LIMITS.0, ZOOM_LIMITS.1)
}

/// The zoom step `steps + delta` would land on, or `steps` if the size can't change further.
pub fn zoom_by(base: f32, steps: i32, delta: i32) -> i32 {
    let next = steps.saturating_add(delta);
    if (zoomed_size(base, next) - zoomed_size(base, steps)).abs() < f32::EPSILON {
        steps
    } else {
        next
    }
}

/// Sets up the terminal feature: zooming is app-wide, so its actions are handled here; the
/// rest are handled by each [`TerminalView`]. All are bound in `keymap/default.json`.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &actions::ZoomIn, cx| zoom(1, cx));
    cx.on_action(|_: &actions::ZoomOut, cx| zoom(-1, cx));
    cx.on_action(|_: &actions::ResetZoom, cx| zoom(0, cx));
}

/// Zooms every terminal by `delta` steps, or back to the theme's size when `delta` is 0.
fn zoom(delta: i32, cx: &mut App) {
    let base = f32::from(cx.theme().text.terminal_size);
    let current = cx
        .try_global::<TerminalZoom>()
        .copied()
        .unwrap_or_default()
        .0;
    let next = if delta == 0 {
        0
    } else {
        zoom_by(base, current, delta)
    };
    if next != current {
        cx.set_global(TerminalZoom(next));
    }
}
