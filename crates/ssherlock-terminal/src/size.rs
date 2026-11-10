//! The size of a terminal in cells, and of one cell in pixels.

/// Fewest columns a terminal can have; two are needed to hold one full-width character.
pub const MIN_COLUMNS: u16 = 2;

/// Fewest rows a terminal can have.
pub const MIN_ROWS: u16 = 1;

/// Size of a terminal's visible area.
///
/// Always at least [`MIN_COLUMNS`] × [`MIN_ROWS`], so code that lays out or resizes a terminal
/// never has to handle an empty grid. Cell size in pixels is carried along because programs can
/// ask for it (`CSI 14 t`) and ptys report it to the child.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerminalSize {
    columns: u16,
    rows: u16,
    cell_width: u16,
    cell_height: u16,
}

impl TerminalSize {
    /// A terminal of `columns` × `rows` cells, raised to the minimum if smaller. Cell size is
    /// unknown (zero) until [`with_cell_size`](Self::with_cell_size) sets it.
    pub fn new(columns: u16, rows: u16) -> Self {
        Self {
            columns: columns.max(MIN_COLUMNS),
            rows: rows.max(MIN_ROWS),
            cell_width: 0,
            cell_height: 0,
        }
    }

    /// How many whole cells of `cell_width` × `cell_height` pixels fit in an area of `width` ×
    /// `height` pixels, keeping the cell size.
    ///
    /// Total for any input: negative, zero, NaN or infinite values give the minimum size.
    pub fn fit(width: f32, height: f32, cell_width: f32, cell_height: f32) -> Self {
        Self::new(
            cells_in(width, cell_width, MIN_COLUMNS),
            cells_in(height, cell_height, MIN_ROWS),
        )
        .with_cell_size(pixels(cell_width), pixels(cell_height))
    }

    /// The same grid with each cell `width` × `height` pixels.
    #[must_use]
    pub fn with_cell_size(self, width: u16, height: u16) -> Self {
        Self {
            cell_width: width,
            cell_height: height,
            ..self
        }
    }

    /// Width in cells.
    pub fn columns(self) -> u16 {
        self.columns
    }

    /// Height in cells.
    pub fn rows(self) -> u16 {
        self.rows
    }

    /// Width of one cell in pixels, zero if unknown.
    pub fn cell_width(self) -> u16 {
        self.cell_width
    }

    /// Height of one cell in pixels, zero if unknown.
    pub fn cell_height(self) -> u16 {
        self.cell_height
    }
}

impl Default for TerminalSize {
    /// The classic 80 × 24.
    fn default() -> Self {
        Self::new(80, 24)
    }
}

/// Whole cells of `cell` pixels that fit in `extent` pixels, at least `minimum`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is finite and clamped to the u16 range before the cast"
)]
fn cells_in(extent: f32, cell: f32, minimum: u16) -> u16 {
    let cells = (extent / cell).floor();
    if cells.is_finite() {
        cells.clamp(f32::from(minimum), f32::from(u16::MAX)) as u16
    } else {
        minimum
    }
}

/// A pixel measure rounded to whole pixels, zero if it isn't a usable size.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is finite and clamped to the u16 range before the cast"
)]
fn pixels(value: f32) -> u16 {
    let rounded = value.round();
    if rounded.is_finite() {
        rounded.clamp(0., f32::from(u16::MAX)) as u16
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn new_raises_to_the_minimum() {
        let cases = [
            ((80, 24), (80, 24)),
            ((0, 0), (MIN_COLUMNS, MIN_ROWS)),
            ((1, 5), (MIN_COLUMNS, 5)),
            ((5, 0), (5, MIN_ROWS)),
        ];
        for ((columns, rows), expected) in cases {
            let size = TerminalSize::new(columns, rows);
            assert_eq!((size.columns(), size.rows()), expected, "{columns}x{rows}");
        }
    }

    #[test]
    fn fit_counts_whole_cells() {
        let cases = [
            ("exact", (800., 480., 10., 20.), (80, 24, 10, 20)),
            (
                "partial cells dropped",
                (805., 499., 10., 20.),
                (80, 24, 10, 20),
            ),
            (
                "fractional cell size",
                (100., 100., 8.4, 16.6),
                (11, 6, 8, 17),
            ),
            (
                "too small",
                (5., 5., 10., 20.),
                (MIN_COLUMNS, MIN_ROWS, 10, 20),
            ),
            (
                "zero cell",
                (800., 480., 0., 0.),
                (MIN_COLUMNS, MIN_ROWS, 0, 0),
            ),
            (
                "negative area",
                (-1., -1., 10., 20.),
                (MIN_COLUMNS, MIN_ROWS, 10, 20),
            ),
            (
                "NaN",
                (f32::NAN, 480., f32::NAN, 20.),
                (MIN_COLUMNS, 24, 0, 20),
            ),
            ("huge", (1e9, 1e9, 1., 1.), (u16::MAX, u16::MAX, 1, 1)),
        ];
        for (name, (w, h, cw, ch), expected) in cases {
            let size = TerminalSize::fit(w, h, cw, ch);
            let actual = (
                size.columns(),
                size.rows(),
                size.cell_width(),
                size.cell_height(),
            );
            assert_eq!(actual, expected, "{name}");
        }
    }
}
