//! Painting a terminal [`Snapshot`]: backgrounds, text, decorations and the cursor.
//!
//! Stateless: everything comes in through arguments. Text is shaped in batches of cells that
//! share a style; each batch is drawn at its column with every glyph forced onto the cell grid,
//! so the grid stays aligned even when a fallback font has different advances. Wide and
//! combining characters get a batch of their own, because forcing would misplace them.

use std::sync::Arc;

use gpui::{
    App, BorderStyle, Bounds, Font, FontStyle, FontWeight, Hsla, Pixels, Point, SharedString,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, fill, font, outline, point, px, size,
};
use ssherlock_terminal::{
    CellSide, CellStyle, CursorShape, Rgb, Snapshot, SnapshotCell, TerminalSize,
};

use crate::theme::{Theme, from_rgb};

/// The size of one terminal cell and the font it is drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct CellMetrics {
    /// The regular-weight terminal font.
    pub font: Font,
    /// Text size.
    pub font_size: Pixels,
    /// Width of one cell: the advance of a character in the monospace font.
    pub cell_width: Pixels,
    /// Height of one cell, i.e. of one line.
    pub line_height: Pixels,
}

impl CellMetrics {
    /// Measures `family` at the theme's terminal text size.
    pub fn measure(theme: &Theme, family: SharedString, window: &Window) -> Self {
        let font = font(family);
        let font_size = theme.text.terminal_size;
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        // Monospace fonts usually agree on the advance of every glyph; `m` is the customary one
        // to measure. Fall back to the usual monospace proportion if the font can't say.
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map_or(font_size * 0.6, |advance| advance.width);
        let line_height = (font_size * theme.text.terminal_line_height).round();
        Self {
            font,
            font_size,
            cell_width,
            line_height,
        }
    }

    /// How many cells fit in `area`.
    pub fn grid_size(&self, area: gpui::Size<Pixels>) -> TerminalSize {
        TerminalSize::fit(
            f32::from(area.width),
            f32::from(area.height),
            f32::from(self.cell_width),
            f32::from(self.line_height),
        )
    }

    /// The `(row, column)` of the cell under `position`, where `origin` is the top-left of the
    /// grid; positions outside the grid are clamped onto its edge.
    pub fn cell_at(
        &self,
        position: Point<Pixels>,
        origin: Point<Pixels>,
        rows: usize,
        columns: usize,
    ) -> (usize, usize) {
        let offset = position - origin;
        let index = |distance: Pixels, cell: Pixels, count: usize| {
            let cells = (f32::from(distance) / f32::from(cell)).floor();
            // Clamped into 0..count, so the cast can't truncate or wrap.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "clamped to 0..count before the cast"
            )]
            let clamped = if cells.is_finite() {
                cells.clamp(0., count.saturating_sub(1) as f32) as usize
            } else {
                0
            };
            clamped
        };
        (
            index(offset.y, self.line_height, rows),
            index(offset.x, self.cell_width, columns),
        )
    }

    /// Which half of its cell `position` is over. Left of the grid counts as the left half of
    /// the first column, right of it as the right half of the last.
    pub fn side_at(
        &self,
        position: Point<Pixels>,
        origin: Point<Pixels>,
        columns: usize,
    ) -> CellSide {
        let cells = f32::from(position.x - origin.x) / f32::from(self.cell_width);
        if !cells.is_finite() || cells < 0. {
            CellSide::Left
        } else if cells >= count(columns) || cells.fract() >= 0.5 {
            CellSide::Right
        } else {
            CellSide::Left
        }
    }

    /// Top-left of the cell at `row`, `column`.
    fn cell_origin(&self, origin: Point<Pixels>, row: usize, column: usize) -> Point<Pixels> {
        point(
            origin.x + self.cell_width * count(column),
            origin.y + self.line_height * count(row),
        )
    }
}

/// A cell count as a multiplier for pixel sizes. Grids are at most `u16::MAX` cells across, so
/// the conversion is exact.
#[allow(
    clippy::cast_precision_loss,
    reason = "grid coordinates fit in u16, which f32 represents exactly"
)]
fn count(cells: usize) -> f32 {
    cells as f32
}

/// How the terminal should be drawn this frame.
#[derive(Clone, Debug)]
pub struct PaintStyle {
    /// Cell size and font.
    pub metrics: CellMetrics,
    /// Whether the terminal has keyboard focus; an unfocused terminal shows a hollow cursor.
    pub is_focused: bool,
    /// Thickness of beam and underline cursors.
    pub cursor_thickness: Pixels,
    /// Drawn behind selected text.
    pub selection: Hsla,
}

/// Paints `snapshot` into `bounds`.
pub fn paint(
    bounds: Bounds<Pixels>,
    snapshot: &Arc<Snapshot>,
    style: &PaintStyle,
    window: &mut Window,
    cx: &mut App,
) {
    let metrics = &style.metrics;
    let origin = bounds.origin;
    let default_bg = snapshot.background;

    for (row, line) in snapshot.lines().enumerate() {
        for run in background_runs(line, default_bg) {
            let start = metrics.cell_origin(origin, row, run.column);
            let width = metrics.cell_width * count(run.width);
            window.paint_quad(fill(
                Bounds::new(start, size(width, metrics.line_height)),
                from_rgb(run.color),
            ));
        }
        if let Some(columns) = snapshot
            .selection
            .and_then(|selection| selection.columns_on(row, snapshot.columns()))
        {
            let start = metrics.cell_origin(origin, row, *columns.start());
            let width = metrics.cell_width * count(columns.count());
            window.paint_quad(fill(
                Bounds::new(start, size(width, metrics.line_height)),
                style.selection,
            ));
        }
        for batch in text_batches(line) {
            let start = metrics.cell_origin(origin, row, batch.column);
            paint_text(&batch, start, metrics, window, cx);
        }
    }

    if let Some(cursor) = snapshot.cursor {
        let cell = snapshot.cell(cursor.row, cursor.column);
        let columns = if cell.is_some_and(|c| c.style.is_wide()) {
            2.
        } else {
            1.
        };
        let start = metrics.cell_origin(origin, cursor.row, cursor.column);
        let cell_bounds = Bounds::new(
            start,
            size(metrics.cell_width * columns, metrics.line_height),
        );
        let color = from_rgb(snapshot.cursor_color);
        let shape = if style.is_focused {
            cursor.shape
        } else {
            CursorShape::HollowBlock
        };
        match shape {
            CursorShape::Block => {
                window.paint_quad(fill(cell_bounds, color));
                // Redraw the character under the cursor in the background colour.
                if let Some(cell) = cell.filter(|c| c.ch != ' ' && !c.style.is_spacer()) {
                    let batch = TextBatch {
                        column: cursor.column,
                        text: cell_text(cell),
                        fg: default_bg,
                        style: cell.style,
                        is_isolated: true,
                    };
                    paint_text(&batch, start, metrics, window, cx);
                }
            }
            CursorShape::HollowBlock => {
                window.paint_quad(outline(cell_bounds, color, BorderStyle::Solid));
            }
            CursorShape::Beam => {
                window.paint_quad(fill(
                    Bounds::new(start, size(style.cursor_thickness, metrics.line_height)),
                    color,
                ));
            }
            CursorShape::Underline => {
                let y = start.y + metrics.line_height - style.cursor_thickness;
                window.paint_quad(fill(
                    Bounds::new(
                        point(start.x, y),
                        size(cell_bounds.size.width, style.cursor_thickness),
                    ),
                    color,
                ));
            }
        }
    }
}

fn paint_text(
    batch: &TextBatch,
    origin: Point<Pixels>,
    metrics: &CellMetrics,
    window: &mut Window,
    cx: &mut App,
) {
    let color: Hsla = from_rgb(batch.fg);
    let mut font = metrics.font.clone();
    if batch.style.is_bold() {
        font.weight = FontWeight::BOLD;
    }
    if batch.style.is_italic() {
        font.style = FontStyle::Italic;
    }
    let decoration = px(1.);
    let run = TextRun {
        len: batch.text.len(),
        font,
        color,
        background_color: None,
        underline: batch.style.underline().map(|underline| UnderlineStyle {
            thickness: decoration,
            color: Some(color),
            wavy: underline == ssherlock_terminal::Underline::Curly,
        }),
        strikethrough: batch.style.is_strikeout().then_some(StrikethroughStyle {
            thickness: decoration,
            color: Some(color),
        }),
    };
    let force_width = (!batch.is_isolated).then_some(metrics.cell_width);
    let line = window.text_system().shape_line(
        SharedString::from(batch.text.clone()),
        metrics.font_size,
        &[run],
        force_width,
    );
    if let Err(error) = line.paint(origin, metrics.line_height, window, cx) {
        tracing::debug!(%error, "could not paint a run of terminal text");
    }
}

/// A run of cells on one line with the same background, other than the default.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BackgroundRun {
    column: usize,
    width: usize,
    color: Rgb,
}

/// Runs of non-default background on a line. The default background is painted once for the
/// whole terminal, so most lines produce nothing here.
fn background_runs(line: &[SnapshotCell], default_bg: Rgb) -> Vec<BackgroundRun> {
    let mut runs: Vec<BackgroundRun> = Vec::new();
    for (column, cell) in line.iter().enumerate() {
        if cell.bg == default_bg {
            continue;
        }
        match runs.last_mut() {
            Some(run) if run.color == cell.bg && run.column + run.width == column => run.width += 1,
            _ => runs.push(BackgroundRun {
                column,
                width: 1,
                color: cell.bg,
            }),
        }
    }
    runs
}

/// Cells on one line that can be shaped and painted as one piece of text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TextBatch {
    column: usize,
    text: String,
    fg: Rgb,
    style: CellStyle,
    /// A wide or combining character, painted on its own without forcing glyphs to the grid.
    is_isolated: bool,
}

/// What must match for two cells to share a batch.
type TextKey = (Rgb, bool, bool, Option<ssherlock_terminal::Underline>, bool);

fn text_key(fg: Rgb, style: CellStyle) -> TextKey {
    (
        fg,
        style.is_bold(),
        style.is_italic(),
        style.underline(),
        style.is_strikeout(),
    )
}

fn cell_text(cell: &SnapshotCell) -> String {
    std::iter::once(cell.ch)
        .chain(cell.zerowidth.iter().copied())
        .collect()
}

/// Splits a line into text batches. Blank cells without underline or strikeout draw nothing, so
/// they're trimmed from the end of batches and blank batches are dropped.
fn text_batches(line: &[SnapshotCell]) -> Vec<TextBatch> {
    let mut batches: Vec<TextBatch> = Vec::new();
    let mut next_column = 0;
    for (column, cell) in line.iter().enumerate() {
        if cell.style.is_spacer() {
            continue;
        }
        let is_isolated = cell.style.is_wide() || !cell.zerowidth.is_empty();
        let extends_last = batches.last().is_some_and(|last| {
            !last.is_isolated
                && !is_isolated
                && next_column == column
                && text_key(last.fg, last.style) == text_key(cell.fg, cell.style)
        });
        match batches.last_mut() {
            Some(last) if extends_last => last.text.push(cell.ch),
            _ => batches.push(TextBatch {
                column,
                text: cell_text(cell),
                fg: cell.fg,
                style: cell.style,
                is_isolated,
            }),
        }
        next_column = column + 1;
    }
    for batch in &mut batches {
        if batch.style.underline().is_none() && !batch.style.is_strikeout() {
            let kept = batch.text.trim_end_matches(' ').len();
            batch.text.truncate(kept);
        }
    }
    batches.retain(|batch| !batch.text.is_empty());
    batches
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;
    use ssherlock_terminal::{Terminal, TerminalConfig};

    use super::*;

    /// The first line of a terminal after the program printed `output`.
    fn line(output: &[u8]) -> (Vec<SnapshotCell>, Rgb) {
        let mut terminal = Terminal::new(&TerminalConfig::default(), TerminalSize::new(12, 2));
        terminal.feed(output);
        let snapshot = terminal.snapshot();
        (
            snapshot.lines().next().unwrap().to_vec(),
            snapshot.background,
        )
    }

    fn summary(batches: &[TextBatch]) -> Vec<(usize, &str, bool)> {
        batches
            .iter()
            .map(|b| (b.column, b.text.as_str(), b.is_isolated))
            .collect()
    }

    #[test]
    fn same_style_text_is_one_batch_and_blanks_are_dropped() {
        let (cells, _) = line(b"ls -la   ");
        assert_eq!(summary(&text_batches(&cells)), [(0, "ls -la", false)]);
        let (cells, _) = line(b"");
        assert_eq!(text_batches(&cells), []);
    }

    #[test]
    fn style_changes_split_batches() {
        let (cells, _) = line(b"a\x1b[31mbc\x1b[1md\x1b[0m e");
        assert_eq!(
            summary(&text_batches(&cells)),
            [
                (0, "a", false),
                (1, "bc", false),
                (3, "d", false),
                (4, " e", false)
            ]
        );
    }

    #[test]
    fn wide_and_combining_characters_are_isolated() {
        let (cells, _) = line("a日b e\u{301}c".as_bytes());
        assert_eq!(
            summary(&text_batches(&cells)),
            [
                (0, "a", false),
                (1, "日", true),
                (3, "b", false),
                (5, "e\u{301}", true),
                (6, "c", false),
            ]
        );
    }

    #[test]
    fn underlined_blanks_are_kept() {
        let (cells, _) = line(b"\x1b[4m  \x1b[0m");
        assert_eq!(summary(&text_batches(&cells)), [(0, "  ", false)]);
    }

    #[test]
    fn background_runs_skip_the_default() {
        let (cells, default_bg) = line(b"a\x1b[41mbc\x1b[42md\x1b[0me\x1b[41mf");
        let runs = background_runs(&cells, default_bg);
        let palette = ssherlock_terminal::Palette::default();
        assert_eq!(
            runs,
            [
                BackgroundRun {
                    column: 1,
                    width: 2,
                    color: palette.ansi[1]
                },
                BackgroundRun {
                    column: 3,
                    width: 1,
                    color: palette.ansi[2]
                },
                BackgroundRun {
                    column: 5,
                    width: 1,
                    color: palette.ansi[1]
                },
            ]
        );
    }

    #[test]
    fn cell_at_maps_and_clamps() {
        let metrics = CellMetrics {
            font: font("Test Mono"),
            font_size: px(14.),
            cell_width: px(10.),
            line_height: px(20.),
        };
        let origin = point(px(5.), px(5.));
        let cases = [
            ((5., 5.), (0, 0)),
            ((14.9, 24.9), (0, 0)),
            ((15., 25.), (1, 1)),
            ((104., 64.), (2, 9)),
            ((-50., -50.), (0, 0)),
            ((10_000., 10_000.), (23, 79)),
        ];
        for ((x, y), expected) in cases {
            assert_eq!(
                metrics.cell_at(point(px(x), px(y)), origin, 24, 80),
                expected,
                "({x}, {y})"
            );
        }
        let sides = [
            ("left of the grid", 4., CellSide::Left),
            ("cell start", 5., CellSide::Left),
            ("just before the middle", 9.9, CellSide::Left),
            ("middle", 10., CellSide::Right),
            ("cell end", 14.9, CellSide::Right),
            ("next cell start", 15., CellSide::Left),
            ("right of the grid", 2_000., CellSide::Right),
        ];
        for (name, x, expected) in sides {
            assert_eq!(
                metrics.side_at(point(px(x), px(9.)), origin, 80),
                expected,
                "{name}"
            );
        }
        assert_eq!(
            metrics.grid_size(size(px(805.), px(485.))),
            TerminalSize::new(80, 24).with_cell_size(10, 20)
        );
    }
}
