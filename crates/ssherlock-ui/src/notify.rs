//! Telling the user about failures (and the occasional confirmation) with toasts.
//!
//! The one place errors become user-facing messages (CLAUDE.md §5): code that hits an error it
//! can't recover from calls [`error`], and a toast shows in the corner of the window for a while.
//! Messages are the error's chain of causes, so "could not save the host: could not query the
//! database: UNIQUE constraint failed" reads top-down from what the user did to why it failed.

use std::error::Error;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div,
};

use crate::ssh::status::capitalise;
use crate::theme::ActiveTheme as _;

/// How long a toast stays before it hides itself.
const TOAST_LIFETIME: Duration = Duration::from_secs(8);

/// How serious a toast is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Something the user asked for didn't happen.
    Error,
    /// Something happened that the user might not notice otherwise.
    Info,
}

/// One message in the corner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    id: u64,
    /// How serious it is.
    pub level: Level,
    /// The text, a full sentence.
    pub message: SharedString,
}

/// The toasts on screen. One per app, installed by [`init`]; each window draws it over its
/// content.
#[derive(Default)]
pub struct Notifications {
    toasts: Vec<Toast>,
    next_id: u64,
}

struct GlobalNotifications(Entity<Notifications>);

impl Global for GlobalNotifications {}

/// Installs the app's notifications. Call once from `ssherlock_ui::init`.
pub fn init(cx: &mut App) {
    let notifications = cx.new(|_| Notifications::default());
    cx.set_global(GlobalNotifications(notifications));
}

/// The app's notifications, for a window to draw. `None` before [`init`].
pub fn notifications(cx: &App) -> Option<Entity<Notifications>> {
    cx.try_global::<GlobalNotifications>().map(|g| g.0.clone())
}

/// Shows that `error` stopped something. Also logged, so it is in the log file.
pub fn error(error: &(dyn Error + 'static), cx: &mut App) {
    let message = chain(error);
    tracing::warn!(error = %message, "shown to the user");
    push(Level::Error, &message, cx);
}

/// Shows an informational message.
pub fn info(message: impl Into<String>, cx: &mut App) {
    push(Level::Info, &message.into(), cx);
}

fn push(level: Level, message: &str, cx: &mut App) {
    let Some(notifications) = notifications(cx) else {
        tracing::warn!(%message, "no notifications installed to show this");
        return;
    };
    notifications.update(cx, |n, cx| {
        let id = n.next_id;
        n.next_id += 1;
        n.toasts.push(Toast {
            id,
            level,
            message: capitalise(message).into(),
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_LIFETIME).await;
            // The app may be quitting; then there's nothing to hide.
            let _ = this.update(cx, |n, cx| n.dismiss(id, cx));
        })
        .detach();
    });
}

/// `error` and its sources joined: "could not save the host: could not query the database".
fn chain(error: &(dyn Error + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        // `#[error(transparent)]` repeats the inner message; don't say it twice.
        if parts.last() != Some(&text) {
            parts.push(text);
        }
        source = cause.source();
    }
    parts.join(": ")
}

impl Notifications {
    /// The toasts on screen, oldest first.
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.id != id);
        if self.toasts.len() != before {
            cx.notify();
        }
    }
}

impl Render for Notifications {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .absolute()
            .bottom(theme.spacing.xl)
            .right(theme.spacing.xl)
            .flex()
            .flex_col()
            .gap(theme.spacing.md)
            .children(self.toasts.iter().map(|toast| {
                let id = toast.id;
                let hover = theme.colors.control_hover;
                div()
                    .id(("toast", id))
                    // Toasts float over terminals, which take focus on any mouse-down they see.
                    .occlude()
                    .flex()
                    .items_start()
                    .gap(theme.spacing.md)
                    .w(theme.metrics.toast_width)
                    .p(theme.spacing.lg)
                    .rounded(theme.radius.md)
                    .bg(theme.colors.surface)
                    .border(theme.metrics.border_width)
                    .border_color(theme.colors.border)
                    .text_size(theme.text.ui_size)
                    .text_color(theme.colors.text)
                    .when(toast.level == Level::Error, |d| {
                        d.border_l(theme.metrics.tab_indicator * 2.)
                            .border_color(theme.colors.danger)
                    })
                    .child(div().flex_1().min_w_0().child(toast.message.clone()))
                    .child(
                        div()
                            .id(("toast-close", id))
                            .px(theme.spacing.sm)
                            .rounded(theme.radius.sm)
                            .text_color(theme.colors.text_muted)
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(move |n, _, _, cx| n.dismiss(id, cx)))
                            .child("\u{00d7}"),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use gpui::TestAppContext;
    use pretty_assertions::assert_eq;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("could not save the host")]
    struct Outer(#[source] Inner);

    #[derive(Debug, thiserror::Error)]
    #[error("could not query the database")]
    struct Inner(#[source] std::io::Error);

    #[test]
    fn messages_follow_the_chain_of_causes() {
        let error = Outer(Inner(std::io::Error::other("disk full")));
        assert_eq!(
            chain(&error),
            "could not save the host: could not query the database: disk full"
        );
    }

    #[gpui::test]
    fn toasts_show_and_hide_themselves(cx: &mut TestAppContext) {
        cx.update(init);
        cx.update(|cx| error(&Inner(std::io::Error::other("disk full")), cx));
        let shown = cx.update(|cx| {
            notifications(cx)
                .unwrap()
                .read(cx)
                .toasts()
                .iter()
                .map(|t| t.message.to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(shown, ["Could not query the database: disk full"]);
        cx.executor().advance_clock(TOAST_LIFETIME);
        cx.run_until_parked();
        assert_eq!(
            cx.update(|cx| notifications(cx).unwrap().read(cx).toasts().len()),
            0
        );
    }
}
