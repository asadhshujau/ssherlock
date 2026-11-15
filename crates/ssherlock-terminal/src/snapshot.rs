//! An immutable picture of a terminal's visible screen, ready to paint.
//!
//! Colours are already resolved to [`Rgb`] (bold-as-bright, dim, inverse and hidden applied), so
//! the view only lays out text and rectangles. A snapshot owns its data and doesn't borrow the
//! terminal, so it can cross from the terminal's task to the UI thread.

use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;

use crate::palette::Rgb;

/// The visible screen at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub(crate) columns: usize,
    pub(crate) cells: Vec<SnapshotCell>,
    /// Where to draw the cursor, or `None` when it is hidden or scrolled out of view.
    pub cursor: Option<Cursor>,
    /// How many lines the view is scrolled up into the scrollback; zero at the bottom.
    pub display_offset: usize,
    /// Lines of scrollback above the screen.
    pub history_size: usize,
    /// Modes that change how input is encoded and the screen behaves.
    pub modes: TerminalModes,
    /// Default background, for the area around the cells.
    pub background: Rgb,
    /// Colour of the cursor.
    pub cursor_color: Rgb,
}

impl Snapshot {
    /// Width in cells.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// Height in cells.
    pub fn rows(&self) -> usize {
        self.cells.len().checked_div(self.columns).unwrap_or(0)
    }

    /// The cells of each visible row, top to bottom.
    pub fn lines(&self) -> impl ExactSizeIterator<Item = &[SnapshotCell]> {
        // `columns` is never zero for a snapshot built by `Terminal`; `max(1)` keeps this total.
        self.cells.chunks_exact(self.columns.max(1))
    }

    /// The cell at `row`, `column`, counted from the top-left of the visible screen.
    pub fn cell(&self, row: usize, column: usize) -> Option<&SnapshotCell> {
        if column >= self.columns {
            return None;
        }
        self.cells
            .get(row.checked_mul(self.columns)?.checked_add(column)?)
    }
}

/// One character cell, with its colours resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotCell {
    /// The character; a space for an empty cell.
    pub ch: char,
    /// Combining characters drawn on top of `ch`, usually empty.
    pub zerowidth: Vec<char>,
    /// Text colour.
    pub fg: Rgb,
    /// Background colour.
    pub bg: Rgb,
    /// Weight, slant, decorations and width.
    pub style: CellStyle,
}

/// How a cell's text is drawn and how many columns it covers.
///
/// Wraps the emulator's flags so the UI doesn't depend on the emulator crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellStyle(pub(crate) Flags);

/// The kind of line drawn under a cell's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Underline {
    /// One straight line.
    Single,
    /// Two straight lines.
    Double,
    /// A wavy line.
    Curly,
    /// A dotted line.
    Dotted,
    /// A dashed line.
    Dashed,
}

impl CellStyle {
    /// Bold weight.
    pub fn is_bold(self) -> bool {
        self.0.contains(Flags::BOLD)
    }

    /// Italic slant.
    pub fn is_italic(self) -> bool {
        self.0.contains(Flags::ITALIC)
    }

    /// The underline, if any.
    pub fn underline(self) -> Option<Underline> {
        let flags = self.0;
        if flags.contains(Flags::DOUBLE_UNDERLINE) {
            Some(Underline::Double)
        } else if flags.contains(Flags::UNDERCURL) {
            Some(Underline::Curly)
        } else if flags.contains(Flags::DOTTED_UNDERLINE) {
            Some(Underline::Dotted)
        } else if flags.contains(Flags::DASHED_UNDERLINE) {
            Some(Underline::Dashed)
        } else if flags.contains(Flags::UNDERLINE) {
            Some(Underline::Single)
        } else {
            None
        }
    }

    /// Struck through.
    pub fn is_strikeout(self) -> bool {
        self.0.contains(Flags::STRIKEOUT)
    }

    /// A full-width character that covers this cell and the next.
    pub fn is_wide(self) -> bool {
        self.0.contains(Flags::WIDE_CHAR)
    }

    /// A placeholder cell covered by a wide character; draw nothing here.
    pub fn is_spacer(self) -> bool {
        self.0
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
    }
}

/// The cursor's position on the visible screen and how to draw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Row from the top of the visible screen.
    pub row: usize,
    /// Column from the left.
    pub column: usize,
    /// What it looks like.
    pub shape: CursorShape,
    /// Whether the program asked for a blinking cursor.
    pub is_blinking: bool,
}

/// Cursor shapes a program can choose with `DECSCUSR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CursorShape {
    /// A filled block over the cell.
    Block,
    /// A line under the cell.
    Underline,
    /// A vertical bar before the cell.
    Beam,
    /// An outlined block, e.g. for an unfocused terminal.
    HollowBlock,
}

/// Terminal modes that programs switch on and off, which change how keys, mouse and paste are
/// sent to the program.
///
/// Wraps the emulator's mode flags so the UI doesn't depend on the emulator crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerminalModes(pub(crate) TermMode);

/// Which mouse events the program asked to receive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseReporting {
    /// None: the mouse selects text.
    Off,
    /// Button presses and releases (`?1000`).
    Click,
    /// Presses, releases and motion while a button is held (`?1002`).
    Drag,
    /// All of the above plus motion with no button held (`?1003`).
    Motion,
}

/// How mouse reports are encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseEncoding {
    /// The original X10 encoding, limited to 223 columns.
    Default,
    /// X10 with UTF-8 coordinates (`?1005`).
    Utf8,
    /// `CSI < … M/m` (`?1006`), unlimited and the most common today.
    Sgr,
}

impl TerminalModes {
    /// Cursor keys send `ESC O A` rather than `ESC [ A` (`DECCKM`).
    pub fn is_app_cursor(self) -> bool {
        self.0.contains(TermMode::APP_CURSOR)
    }

    /// The keypad sends application sequences (`DECKPAM`).
    pub fn is_app_keypad(self) -> bool {
        self.0.contains(TermMode::APP_KEYPAD)
    }

    /// Pasted text is wrapped in `ESC [200~ … ESC [201~`.
    pub fn is_bracketed_paste(self) -> bool {
        self.0.contains(TermMode::BRACKETED_PASTE)
    }

    /// The program wants `ESC [I` / `ESC [O` when the terminal gains or loses focus.
    pub fn is_focus_reporting(self) -> bool {
        self.0.contains(TermMode::FOCUS_IN_OUT)
    }

    /// The alternate screen (used by full-screen programs) is showing; it has no scrollback.
    pub fn is_alt_screen(self) -> bool {
        self.0.contains(TermMode::ALT_SCREEN)
    }

    /// On the alternate screen, the scroll wheel should send arrow keys.
    pub fn is_alternate_scroll(self) -> bool {
        self.0.contains(TermMode::ALTERNATE_SCROLL)
    }

    /// Enter sends `CR LF` rather than `CR` (`LNM`).
    pub fn is_line_feed_new_line(self) -> bool {
        self.0.contains(TermMode::LINE_FEED_NEW_LINE)
    }

    /// Which mouse events the program wants.
    pub fn mouse_reporting(self) -> MouseReporting {
        if self.0.contains(TermMode::MOUSE_MOTION) {
            MouseReporting::Motion
        } else if self.0.contains(TermMode::MOUSE_DRAG) {
            MouseReporting::Drag
        } else if self.0.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseReporting::Click
        } else {
            MouseReporting::Off
        }
    }

    /// How mouse reports are encoded.
    pub fn mouse_encoding(self) -> MouseEncoding {
        if self.0.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if self.0.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Default
        }
    }
}

impl Default for TerminalModes {
    /// The modes a freshly reset terminal starts in.
    fn default() -> Self {
        Self(TermMode::default())
    }
}
