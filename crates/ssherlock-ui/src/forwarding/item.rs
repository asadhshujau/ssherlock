//! A forwarding rule drawn as one row: its kind, name and what it does. Stateless; the views
//! that list rules add their own buttons beside it.

use gpui::prelude::FluentBuilder as _;
use gpui::{Div, FontWeight, ParentElement as _, Styled as _, div};
use ssherlock_core::forwarding::ForwardRule;

use crate::theme::Theme;

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
