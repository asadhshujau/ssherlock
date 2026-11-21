//! A forwarding rule drawn as one row: its kind, name and what it does, and for a running one
//! its status and traffic. Stateless; the words come from plain functions tested without gpui,
//! and the views that list rules add their own buttons beside it.

use gpui::prelude::FluentBuilder as _;
use gpui::{Div, FontWeight, ParentElement as _, Styled as _, div};
use ssherlock_core::forwarding::ForwardRule;
use ssherlock_ssh::{ForwardStats, ForwardStatus};

use super::model::{Carrier, ForwardView};
use crate::ssh::status::{Tone, capitalise};
use crate::theme::Theme;

/// What a rule is doing, in a few words, and the colour of its dot; `None` for the dot when
/// it isn't running.
pub fn status_text(rule: &ForwardRule, view: Option<&ForwardView>) -> (Option<Tone>, String) {
    let Some(view) = view else {
        return if rule.is_enabled {
            (None, "Not running".to_owned())
        } else {
            (None, "Disabled".to_owned())
        };
    };
    match (&view.status, view.carrier) {
        (ForwardStatus::Starting, _) => (Some(Tone::Busy), "Starting\u{2026}".to_owned()),
        (ForwardStatus::Active, Carrier::Tunnel) => (Some(Tone::Good), "Active".to_owned()),
        (ForwardStatus::Active, Carrier::Tab) => {
            (Some(Tone::Good), "Active in a terminal tab".to_owned())
        }
        (ForwardStatus::Waiting, _) => (
            Some(Tone::Busy),
            "Waiting for the connection\u{2026}".to_owned(),
        ),
        (ForwardStatus::Failed(reason), _) => (Some(Tone::Bad), capitalise(reason)),
        (ForwardStatus::Stopped, _) => (None, "Not running".to_owned()),
    }
}

/// `↑ 1.2 KB · ↓ 30.4 KB · 2 open`, or with nothing through it yet, `No connections yet`.
pub fn traffic_text(stats: &ForwardStats) -> String {
    if stats.total_connections == 0 {
        return "No connections yet".to_owned();
    }
    format!(
        "\u{2191} {} \u{00b7} \u{2193} {} \u{00b7} {} open",
        format_bytes(stats.uploaded),
        format_bytes(stats.downloaded),
        stats.active_connections
    )
}

/// `512 B`, `1.2 KB`, `3.4 MB`, `1.1 GB` (powers of 1000, as file managers show sizes).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    // Precision lost converting to f64 only matters past petabytes.
    #[allow(
        clippy::cast_precision_loss,
        reason = "a display value with one decimal"
    )]
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS.get(unit).copied().unwrap_or("TB"))
}

/// The words under a rule's name: what it does, unless the name already says so, and how it
/// starts.
pub fn detail(rule: &ForwardRule) -> String {
    let mut parts = Vec::new();
    if !rule.label.trim().is_empty() {
        parts.push(rule.forward.to_string());
    }
    parts.push(
        match (rule.is_enabled, rule.auto_start) {
            (false, _) => "disabled",
            (true, true) => "starts on connect",
            (true, false) => "started by hand",
        }
        .to_owned(),
    );
    parts.join(" \u{00b7} ")
}

/// `[Local] Postgres` over `localhost:5432 → db:5432 · starts on connect`. A disabled rule is
/// drawn faded.
pub fn summary(rule: &ForwardRule, theme: &Theme) -> Div {
    let text = if rule.is_enabled {
        theme.colors.text
    } else {
        theme.colors.text_disabled
    };
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(theme.spacing.sm)
        .child(
            div()
                .flex_none()
                .flex()
                .justify_center()
                .w(theme.metrics.badge_width)
                .px(theme.spacing.xs)
                .rounded(theme.radius.sm)
                .border(theme.metrics.border_width)
                .border_color(theme.colors.border)
                .text_size(theme.text.small_size)
                .text_color(theme.colors.text_muted)
                .child(rule.forward.kind_name()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(text)
                        .when(rule.is_enabled, |d| d.font_weight(FontWeight::MEDIUM))
                        .child(rule.title().into_owned()),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(theme.text.small_size)
                        .text_color(theme.colors.text_muted)
                        .child(detail(rule)),
                ),
        )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;
    use ssherlock_core::forwarding::{Forward, Listen};
    use ssherlock_core::{Id, Port, Timestamp};

    use super::*;

    #[test]
    fn byte_sizes() {
        for (bytes, text) in [
            (0, "0 B"),
            (999, "999 B"),
            (1000, "1.0 KB"),
            (1250, "1.2 KB"),
            (30_400_000, "30.4 MB"),
            (1_100_000_000, "1.1 GB"),
            (5_000_000_000_000_000, "5000.0 TB"),
        ] {
            assert_eq!(format_bytes(bytes), text, "{bytes}");
        }
    }

    #[test]
    fn statuses_and_traffic() {
        let mut rule = ForwardRule::new(
            Id::generate(),
            Forward::Dynamic {
                listen: Listen::localhost(Port::new(1080).unwrap()),
            },
            Timestamp::UNIX_EPOCH,
        );
        let view = |status, carrier| ForwardView {
            status,
            stats: ForwardStats::default(),
            carrier,
        };
        let cases = [
            (None, (None, "Not running")),
            (
                Some(view(ForwardStatus::Starting, Carrier::Tunnel)),
                (Some(Tone::Busy), "Starting\u{2026}"),
            ),
            (
                Some(view(ForwardStatus::Active, Carrier::Tunnel)),
                (Some(Tone::Good), "Active"),
            ),
            (
                Some(view(ForwardStatus::Active, Carrier::Tab)),
                (Some(Tone::Good), "Active in a terminal tab"),
            ),
            (
                Some(view(ForwardStatus::Waiting, Carrier::Tunnel)),
                (Some(Tone::Busy), "Waiting for the connection\u{2026}"),
            ),
            (
                Some(view(
                    ForwardStatus::Failed("could not listen on 127.0.0.1:1080".into()),
                    Carrier::Tunnel,
                )),
                (Some(Tone::Bad), "Could not listen on 127.0.0.1:1080"),
            ),
        ];
        for (view, (tone, text)) in cases {
            let (got_tone, got_text) = status_text(&rule, view.as_ref());
            assert_eq!((got_tone, got_text.as_str()), (tone, text), "{view:?}");
        }
        rule.is_enabled = false;
        assert_eq!(status_text(&rule, None), (None, "Disabled".to_owned()));

        assert_eq!(traffic_text(&ForwardStats::default()), "No connections yet");
        let stats = ForwardStats {
            uploaded: 1250,
            downloaded: 30_400_000,
            active_connections: 2,
            total_connections: 5,
            last_error: None,
        };
        assert_eq!(
            traffic_text(&stats),
            "\u{2191} 1.2 KB \u{00b7} \u{2193} 30.4 MB \u{00b7} 2 open"
        );
    }

    #[test]
    fn details() {
        let mut rule = ForwardRule::new(
            Id::generate(),
            Forward::Dynamic {
                listen: Listen::localhost(Port::new(1080).unwrap()),
            },
            Timestamp::UNIX_EPOCH,
        );
        assert_eq!(detail(&rule), "started by hand");
        rule.auto_start = true;
        assert_eq!(detail(&rule), "starts on connect");
        rule.label = "Browser".into();
        assert_eq!(
            detail(&rule),
            "SOCKS proxy on localhost:1080 \u{00b7} starts on connect"
        );
        rule.is_enabled = false;
        assert_eq!(
            detail(&rule),
            "SOCKS proxy on localhost:1080 \u{00b7} disabled"
        );
    }
}
