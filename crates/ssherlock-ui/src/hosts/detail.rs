//! The host details panel, the "case file" (HOST-12): everything about the host under the
//! cursor, and a button to connect.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div,
};
use ssherlock_core::hosts::Host;
use ssherlock_core::{Timestamp, time};

use super::item::{HostSummary, mark, tag_chips};
use crate::components::{Button, ButtonStyle};
use crate::theme::Theme;

/// The facts the panel lists, worked out by the list.
pub struct CaseFile {
    /// The row summary: title, target, colour, tags.
    pub summary: HostSummary,
    /// The group path, e.g. `Production / Databases`.
    pub group: Option<String>,
    /// The notes.
    pub notes: String,
    /// When the host was created.
    pub created_at: Timestamp,
    /// When it was last connected to.
    pub last_connected_at: Option<Timestamp>,
    /// How many tabs are open to it.
    pub open_sessions: usize,
}

impl CaseFile {
    /// The case file of `host`.
    pub fn new(
        host: &Host,
        summary: HostSummary,
        group: Option<String>,
        open_sessions: usize,
    ) -> Self {
        Self {
            summary,
            group,
            notes: host.notes.clone(),
            created_at: host.created_at,
            last_connected_at: host.last_connected_at,
            open_sessions,
        }
    }
}

/// The panel for `file`, with Connect and Edit buttons running `on_connect` and `on_edit`.
pub fn case_file(
    file: CaseFile,
    theme: &Theme,
    now: Timestamp,
    on_connect: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_edit: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let row = |label: &'static str, value: SharedString| {
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.xs)
            .child(
                div()
                    .text_size(theme.text.small_size)
                    .text_color(theme.colors.text_muted)
                    .child(label),
            )
            .child(div().child(value))
    };
    let last = file
        .last_connected_at
        .map_or_else(|| "Never".to_owned(), |at| time::ago(at, now));
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(theme.spacing.xl)
        .w(theme.metrics.detail_width)
        .h_full()
        .p(theme.spacing.xl)
        .border_l(theme.metrics.border_width)
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .text_size(theme.text.ui_size)
        .text_color(theme.colors.text)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.md)
                .child(mark(theme, file.summary.colour))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(file.summary.title.clone()),
                ),
        )
        .child(row("Connects to", file.summary.target.clone()))
        .when_some(file.group, |d, group| d.child(row("Group", group.into())))
        .when(!file.summary.tags.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.xs)
                    .child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .child("Tags"),
                    )
                    .child(tag_chips(theme, &file.summary.tags)),
            )
        })
        .when(file.open_sessions > 0, |d| {
            let tabs = match file.open_sessions {
                1 => "1 tab".to_owned(),
                n => format!("{n} tabs"),
            };
            d.child(row("Open now", tabs.into()))
        })
        .child(row("Last connected", last.into()))
        .child(row("Created", time::ago(file.created_at, now).into()))
        .when(!file.notes.trim().is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.xs)
                    .child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .child("Notes"),
                    )
                    .children(file.notes.lines().map(|line| div().child(line.to_owned()))),
            )
        })
        .child(
            div()
                .flex()
                .gap(theme.spacing.md)
                .child(
                    Button::new("case-file-connect", "Connect")
                        .style(ButtonStyle::Primary)
                        .on_click(on_connect),
                )
                .child(Button::new("case-file-edit", "Edit").on_click(on_edit)),
        )
}
