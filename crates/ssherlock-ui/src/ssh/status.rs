//! The connection status bar (CONN-27): a slim strip under a remote terminal with a coloured
//! dot, where it's connected, and how it's going. Stateless; the words come from [`describe`],
//! tested without gpui. It sits below the grid rather than over it, so it never hides output.

use std::time::Duration;

use gpui::{IntoElement, ParentElement as _, SharedString, Styled as _, div};
use ssherlock_ssh::ConnectionState;
use ssherlock_ssh::connection::DisconnectReason;

use crate::theme::Theme;

/// The colour of a status dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Working normally.
    Good,
    /// In progress, or recovering.
    Busy,
    /// Failed or finished.
    Bad,
}

/// What the bar says for `state`, or `None` when it shouldn't show (the tab is closing).
pub fn describe(state: &ConnectionState) -> Option<(Tone, String)> {
    Some(match state {
        ConnectionState::Connecting { attempt: 0 } => (Tone::Busy, "Connecting\u{2026}".to_owned()),
        ConnectionState::Connecting { attempt } => (
            Tone::Busy,
            format!("Reconnecting (attempt {attempt})\u{2026}"),
        ),
        ConnectionState::Authenticating => (Tone::Busy, "Logging in\u{2026}".to_owned()),
        ConnectionState::Connected { latency: None } => (Tone::Good, "Connected".to_owned()),
        ConnectionState::Connected {
            latency: Some(latency),
        } => (Tone::Good, format_latency(*latency)),
        ConnectionState::Reconnecting { attempt, retry_in } => (
            Tone::Busy,
            format!(
                "Connection lost \u{2014} retrying in {} (attempt {attempt})",
                format_wait(*retry_in)
            ),
        ),
        ConnectionState::Disconnected(DisconnectReason::ShellExited(exit)) => {
            (Tone::Bad, format!("Disconnected \u{2014} shell {exit}"))
        }
        ConnectionState::Disconnected(DisconnectReason::Failed(message)) => {
            (Tone::Bad, capitalise(message))
        }
        ConnectionState::Disconnected(DisconnectReason::GaveUp { attempts }) => (
            Tone::Bad,
            format!("Disconnected \u{2014} gave up after {attempts} attempts"),
        ),
        ConnectionState::Disconnected(DisconnectReason::Closed) => return None,
    })
}

/// `23 ms`, `<1 ms` on a LAN, or `1.2 s` once it's slow.
fn format_latency(latency: Duration) -> String {
    match latency.as_millis() {
        0 => "<1 ms".to_owned(),
        ms @ 1..1000 => format!("{ms} ms"),
        _ => format!("{:.1} s", latency.as_secs_f32()),
    }
}

/// Error messages are lower-case sentences (CLAUDE.md §5); standing alone they start upper.
pub fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `1 s`, `30 s`, or `500 ms` for short waits.
fn format_wait(wait: Duration) -> String {
    if wait < Duration::from_secs(1) {
        format!("{} ms", wait.as_millis())
    } else {
        format!("{} s", wait.as_secs())
    }
}

/// The bar: `● alice@example.com · 23 ms`.
pub fn bar(tone: Tone, text: String, label: SharedString, theme: &Theme) -> impl IntoElement {
    let dot = match tone {
        Tone::Good => theme.colors.success,
        Tone::Busy => theme.colors.warning,
        Tone::Bad => theme.colors.danger,
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(theme.spacing.sm)
        .px(theme.spacing.md)
        .py(theme.spacing.xs)
        .bg(theme.colors.tab_bar)
        .border_t(theme.metrics.border_width)
        .border_color(theme.colors.border)
        .text_size(theme.text.small_size)
        .text_color(theme.colors.text_muted)
        .child(
            div()
                .flex_none()
                .size(theme.metrics.status_dot)
                .rounded_full()
                .bg(dot),
        )
        .child(div().flex_none().text_color(theme.colors.text).child(label))
        .child(div().flex_none().child("\u{00b7}"))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(text),
        )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use ssherlock_terminal::PtyExit;

    use super::*;

    #[test]
    fn capitalising() {
        assert_eq!(capitalise("could not"), "Could not");
        assert_eq!(capitalise("\u{e9}t\u{e9}"), "\u{c9}t\u{e9}");
        assert_eq!(capitalise(""), "");
    }

    #[test]
    fn descriptions() {
        let ms = Duration::from_millis;
        let cases = [
            (
                ConnectionState::Connecting { attempt: 0 },
                Some((Tone::Busy, "Connecting\u{2026}")),
            ),
            (
                ConnectionState::Connecting { attempt: 3 },
                Some((Tone::Busy, "Reconnecting (attempt 3)\u{2026}")),
            ),
            (
                ConnectionState::Connected {
                    latency: Some(ms(0)),
                },
                Some((Tone::Good, "<1 ms")),
            ),
            (
                ConnectionState::Authenticating,
                Some((Tone::Busy, "Logging in\u{2026}")),
            ),
            (
                ConnectionState::Connected { latency: None },
                Some((Tone::Good, "Connected")),
            ),
            (
                ConnectionState::Connected {
                    latency: Some(ms(23)),
                },
                Some((Tone::Good, "23 ms")),
            ),
            (
                ConnectionState::Connected {
                    latency: Some(ms(1250)),
                },
                Some((Tone::Good, "1.2 s")),
            ),
            (
                ConnectionState::Reconnecting {
                    attempt: 2,
                    retry_in: ms(2000),
                },
                Some((
                    Tone::Busy,
                    "Connection lost \u{2014} retrying in 2 s (attempt 2)",
                )),
            ),
            (
                ConnectionState::Reconnecting {
                    attempt: 1,
                    retry_in: ms(200),
                },
                Some((
                    Tone::Busy,
                    "Connection lost \u{2014} retrying in 200 ms (attempt 1)",
                )),
            ),
            (
                ConnectionState::Disconnected(DisconnectReason::ShellExited(PtyExit::Code(1))),
                Some((
                    Tone::Bad,
                    "Disconnected \u{2014} shell exited with status 1",
                )),
            ),
            (
                ConnectionState::Disconnected(DisconnectReason::Failed("could not connect".into())),
                Some((Tone::Bad, "Could not connect")),
            ),
            (
                ConnectionState::Disconnected(DisconnectReason::GaveUp { attempts: 10 }),
                Some((Tone::Bad, "Disconnected \u{2014} gave up after 10 attempts")),
            ),
            (
                ConnectionState::Disconnected(DisconnectReason::Closed),
                None,
            ),
        ];
        for (state, expected) in cases {
            let got = describe(&state);
            assert_eq!(
                got.as_ref().map(|(tone, text)| (*tone, text.as_str())),
                expected,
                "{state:?}"
            );
        }
    }
}
