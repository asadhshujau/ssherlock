//! The key details panel: fingerprints, randomart, the public key, and what uses the key
//! (KEY-13, KEY-14).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use ssherlock_core::keychain::{Key, KeyProtection};
use ssherlock_core::{Timestamp, time};
use ssherlock_ssh::keys::Fingerprints;

use crate::components::{Button, ButtonStyle};
use crate::theme::Theme;

/// What the panel shows besides the key itself, worked out by the list.
pub struct KeyFacts {
    /// Fingerprints and randomart, if the public key could be read.
    pub fingerprints: Option<Fingerprints>,
    /// Titles of the hosts that log in with the key.
    pub hosts: Vec<String>,
    /// Titles of the identities that hold it.
    pub identities: Vec<String>,
    /// Names of the groups that pick it.
    pub groups: Vec<String>,
}

/// Handlers for the panel's buttons.
pub struct KeyButtons<C, X, E, D> {
    /// Copy public key.
    pub on_copy: C,
    /// Export.
    pub on_export: X,
    /// Edit.
    pub on_edit: E,
    /// Delete.
    pub on_delete: D,
}

/// How the panel describes `protection`.
pub fn protection_text(protection: &KeyProtection) -> &'static str {
    match protection {
        KeyProtection::Unencrypted => "None",
        KeyProtection::AskPassphrase => "Asked when the key is used, then remembered for a while",
        KeyProtection::SavedPassphrase(_) => "Saved in the vault",
    }
}

/// `n hosts: a, b, c` style lines for what uses a key; "Nothing yet" when nothing does.
pub fn usage_lines(facts: &KeyFacts) -> Vec<String> {
    let line = |one: &str, many: &str, names: &[String]| {
        (!names.is_empty()).then(|| {
            let noun = if names.len() == 1 { one } else { many };
            format!("{} {noun}: {}", names.len(), names.join(", "))
        })
    };
    let lines: Vec<String> = [
        line("host", "hosts", &facts.hosts),
        line("identity", "identities", &facts.identities),
        line("group", "groups", &facts.groups),
    ]
    .into_iter()
    .flatten()
    .collect();
    if lines.is_empty() {
        vec!["Nothing yet".to_owned()]
    } else {
        lines
    }
}

/// The panel for `key`.
pub fn key_panel(
    key: &Key,
    facts: KeyFacts,
    theme: &Theme,
    mono: &SharedString,
    now: Timestamp,
    buttons: KeyButtons<
        impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    >,
) -> Stateful<Div> {
    let label = |text: &'static str| {
        div()
            .text_size(theme.text.small_size)
            .text_color(theme.colors.text_muted)
            .child(text)
    };
    let row = |title: &'static str, value: SharedString| {
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.xs)
            .child(label(title))
            .child(div().child(value))
    };
    let mono_text = |text: String| {
        div()
            .font_family(mono.clone())
            .text_size(theme.text.small_size)
            .child(text)
    };
    let usage = usage_lines(&facts);
    // The scrolling panel holds one column that keeps its natural height; children of a
    // too-tall flex column would otherwise shrink, the title first.
    let column = div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(theme.spacing.lg)
        .p(theme.spacing.xl)
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .font_weight(FontWeight::SEMIBOLD)
                .child(key.title().to_owned()),
        )
        .child(row("Type", key.kind.display_name().into()))
        .when_some(key.comment(), |d, comment| {
            d.child(row("Comment", comment.to_owned().into()))
        })
        .when_some(facts.fingerprints, |d, prints| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.xs)
                    .child(label("Fingerprint"))
                    .child(mono_text(prints.sha256))
                    .child(mono_text(prints.md5)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.xs)
                    .child(label("Randomart"))
                    .children(prints.randomart.lines().map(|line| {
                        div()
                            .font_family(mono.clone())
                            .text_size(theme.text.small_size)
                            .whitespace_nowrap()
                            .child(line.to_owned())
                    })),
            )
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.xs)
                .child(label("Public key"))
                .child(
                    div()
                        .font_family(mono.clone())
                        .text_size(theme.text.small_size)
                        .text_color(theme.colors.text_muted)
                        .child(key.public_key.clone()),
                ),
        )
        .child(row("Passphrase", protection_text(&key.protection).into()))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.xs)
                .child(label("Used by"))
                .children(usage.into_iter().map(|line| div().child(line))),
        )
        .child(row("Added", time::ago(key.created_at, now).into()))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(theme.spacing.md)
                .child(
                    Button::new("key-copy", "Copy public key")
                        .style(ButtonStyle::Primary)
                        .on_click(buttons.on_copy),
                )
                .child(Button::new("key-export", "Export\u{2026}").on_click(buttons.on_export))
                .child(Button::new("key-edit", "Edit").on_click(buttons.on_edit))
                .child(
                    Button::new("key-delete", "Delete")
                        .style(ButtonStyle::Danger)
                        .on_click(buttons.on_delete),
                ),
        );
    div()
        .id("key-panel")
        .flex_none()
        .w(theme.metrics.detail_width)
        .h_full()
        .overflow_y_scroll()
        .border_l(theme.metrics.border_width)
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .text_size(theme.text.ui_size)
        .text_color(theme.colors.text)
        .child(column)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn usage_reads_naturally() {
        let facts = |hosts: &[&str], identities: &[&str]| KeyFacts {
            fingerprints: None,
            hosts: hosts.iter().map(|s| (*s).to_owned()).collect(),
            identities: identities.iter().map(|s| (*s).to_owned()).collect(),
            groups: Vec::new(),
        };
        assert_eq!(usage_lines(&facts(&[], &[])), ["Nothing yet"]);
        assert_eq!(usage_lines(&facts(&["web"], &[])), ["1 host: web"]);
        assert_eq!(
            usage_lines(&facts(&["web", "db"], &["deploy"])),
            ["2 hosts: web, db", "1 identity: deploy"]
        );
    }
}
