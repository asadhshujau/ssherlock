//! The terminal emulator: bytes from a program in, a [`Snapshot`] and [`TerminalEvent`]s out.
//!
//! [`Terminal`] wraps `alacritty_terminal`'s parser and grid and does no I/O, so it behaves the
//! same whether the bytes come from a local pty or an SSH channel, and tests can drive it with
//! plain byte strings. Nothing from `alacritty_terminal` appears in this crate's public API.

use std::sync::mpsc;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::{self, Colors};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Osc52, Term, point_to_viewport, viewport_to_point};
use alacritty_terminal::vte::ansi::{self, Color, CursorShape as VteCursorShape, NamedColor};

use crate::palette::{Palette, Rgb};
use crate::pty::PtyExit;
use crate::search::{MAX_COUNTED_MATCHES, SearchDirection, SearchStatus, literal_pattern};
use crate::selection::{CellSide, GridPoint, SelectionKind, VisibleRange};
use crate::size::TerminalSize;
use crate::snapshot::{CellStyle, Cursor, CursorShape, Snapshot, SnapshotCell, TerminalModes};

/// How to set up a new terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalConfig {
    /// Lines kept above the screen once they scroll off the top. Fixed for the terminal's life.
    pub scrollback_lines: usize,
    /// Colours to start with; [`Terminal::set_palette`] changes them later.
    pub palette: Palette,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            palette: Palette::default(),
        }
    }
}

/// Something the UI may want to show about a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// The program set the title (`OSC 0/2`), or reset it to the default (`None`).
    TitleChanged(Option<String>),
    /// The program rang the bell (`BEL`).
    Bell,
    /// The program exited. Sent by [`TerminalHandle`](crate::TerminalHandle), which watches
    /// the pty; a bare [`Terminal`] never produces it.
    Exited(PtyExit),
}

/// How far to scroll the view through the scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScrollAmount {
    /// This many lines; positive scrolls up, towards older output.
    Lines(i32),
    /// One screen up.
    PageUp,
    /// One screen down.
    PageDown,
    /// To the oldest line.
    Top,
    /// Back to the live screen.
    Bottom,
}

/// A search the user is running.
struct ActiveSearch {
    regex: RegexSearch,
    /// The match the user is on. Points are fixed in the grid, so new output can leave it
    /// pointing at older text; the next search step starts from wherever it now is.
    focused: Option<Match>,
    status: SearchStatus,
}

/// A terminal emulator for one program.
pub struct Terminal {
    term: Term<EventQueue>,
    parser: ansi::Processor,
    events: mpsc::Receiver<Event>,
    pending_events: Vec<TerminalEvent>,
    pending_reply: Vec<u8>,
    search: Option<ActiveSearch>,
    size: TerminalSize,
    palette: Palette,
}

impl Terminal {
    /// An empty terminal of `size`.
    pub fn new(config: &TerminalConfig, size: TerminalSize) -> Self {
        let (sender, events) = mpsc::channel();
        let term_config = Config {
            scrolling_history: config.scrollback_lines,
            // OSC 52 clipboard access is opt-in with a prompt (TERM-04, Phase 9); off until then.
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        Self {
            term: Term::new(term_config, &GridSize(size), EventQueue(sender)),
            parser: ansi::Processor::new(),
            events,
            pending_events: Vec::new(),
            pending_reply: Vec::new(),
            search: None,
            size,
            palette: config.palette.clone(),
        }
    }

    /// Processes output from the program.
    ///
    /// Afterwards, send [`take_reply`](Self::take_reply) back to the program and show
    /// [`drain_events`](Self::drain_events) to the user.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Takes the events produced since the last call.
    pub fn drain_events(&mut self) -> Vec<TerminalEvent> {
        self.collect();
        std::mem::take(&mut self.pending_events)
    }

    /// Takes the bytes to send back to the program since the last call: answers to queries
    /// such as cursor position (`DSR`), device attributes (`DA`) or colours (`OSC 11`). Empty
    /// when there is nothing to send.
    pub fn take_reply(&mut self) -> Vec<u8> {
        self.collect();
        std::mem::take(&mut self.pending_reply)
    }

    /// Sorts the emulator's queued events into replies and events for the UI.
    fn collect(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match self.translate(event) {
                Some(Effect::Reply(bytes)) => self.pending_reply.extend_from_slice(&bytes),
                Some(Effect::Event(event)) => self.pending_events.push(event),
                None => {}
            }
        }
    }

    /// When a synchronized update (`?2026`) times out, if one is in progress.
    ///
    /// The program promised to end the update; if it doesn't by this instant, the owner calls
    /// [`end_sync`](Self::end_sync) so the screen doesn't stay frozen.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Ends a synchronized update that timed out, applying the buffered output.
    pub fn end_sync(&mut self) {
        self.parser.stop_sync(&mut self.term);
    }

    /// Changes the size, rewrapping lines on the main screen to the new width.
    pub fn resize(&mut self, size: TerminalSize) {
        self.size = size;
        self.term.resize(GridSize(size));
    }

    /// The current size.
    pub fn size(&self) -> TerminalSize {
        self.size
    }

    /// Replaces the colours cells are drawn with, e.g. after a theme change.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// The colours cells are drawn with.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// The modes the program has switched on.
    pub fn modes(&self) -> TerminalModes {
        TerminalModes(*self.term.mode())
    }

    /// The visible screen, with colours resolved against the palette.
    ///
    /// Takes `&mut self` only because searching for the visible matches updates the search
    /// engine's cache.
    pub fn snapshot(&mut self) -> Snapshot {
        let (search_matches, focused_match) = self.visible_matches();
        let content = self.term.renderable_content();
        let colors = Resolver {
            palette: &self.palette,
            overrides: content.colors,
        };
        let display_offset = content.display_offset;

        let cells: Vec<SnapshotCell> = content
            .display_iter
            .map(|indexed| colors.cell(indexed.cell))
            .collect();

        let cursor = point_to_viewport(display_offset, content.cursor.point)
            .filter(|point| point.line < self.term.screen_lines())
            .and_then(|point| {
                let shape = match content.cursor.shape {
                    VteCursorShape::Block => CursorShape::Block,
                    VteCursorShape::Underline => CursorShape::Underline,
                    VteCursorShape::Beam => CursorShape::Beam,
                    VteCursorShape::HollowBlock => CursorShape::HollowBlock,
                    VteCursorShape::Hidden => return None,
                };
                Some(Cursor {
                    row: point.line,
                    column: point.column.0,
                    shape,
                    is_blinking: self.term.cursor_style().blinking,
                })
            });

        Snapshot {
            columns: self.term.columns(),
            cells,
            cursor,
            display_offset,
            history_size: self.term.history_size(),
            modes: TerminalModes(content.mode),
            background: colors.lookup(NamedColor::Background as usize),
            cursor_color: colors.lookup(NamedColor::Cursor as usize),
            selection: content
                .selection
                .and_then(|range| self.visible_range(range.start, range.end, range.is_block)),
            search_matches,
            focused_match,
            search_status: self.search.as_ref().map(|search| search.status),
        }
    }

    /// Scrolls the view through the scrollback. Output keeps arriving underneath; the view stays
    /// on the same text until scrolled back to the bottom.
    pub fn scroll(&mut self, amount: ScrollAmount) {
        let scroll = match amount {
            ScrollAmount::Lines(lines) => Scroll::Delta(lines),
            ScrollAmount::PageUp => Scroll::PageUp,
            ScrollAmount::PageDown => Scroll::PageDown,
            ScrollAmount::Top => Scroll::Top,
            ScrollAmount::Bottom => Scroll::Bottom,
        };
        self.term.scroll_display(scroll);
    }

    /// Whether the view is scrolled up into the scrollback.
    pub fn is_scrolled_back(&self) -> bool {
        self.term.grid().display_offset() > 0
    }

    /// Deletes the scrollback, keeping the screen.
    pub fn clear_scrollback(&mut self) {
        self.term.grid_mut().clear_history();
        // Both may point into lines that no longer exist.
        self.term.selection = None;
        if let Some(search) = &mut self.search {
            search.focused = None;
        }
        self.refresh_search_status();
    }

    /// Resets the terminal as if newly started (`RIS`), also deleting the scrollback. For when
    /// a program has left it in a bad state (wrong character set, stuck modes).
    pub fn reset(&mut self) {
        self.feed(b"\x1bc");
        self.search = None;
        self.clear_scrollback();
    }

    /// Searches the screen and scrollback for `query`, focusing the newest match and scrolling
    /// to it. An empty query ends the search.
    pub fn set_search(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            return;
        }
        let regex = match RegexSearch::new(&literal_pattern(query)) {
            Ok(regex) => regex,
            Err(error) => {
                // The pattern is escaped, so this means the query is too long for the engine.
                tracing::warn!(%error, "could not search for this text");
                self.search = None;
                return;
            }
        };
        self.search = Some(ActiveSearch {
            regex,
            focused: None,
            status: SearchStatus {
                total: 0,
                current: None,
                is_capped: false,
            },
        });
        let bottom = Point::new(self.term.bottommost_line(), self.term.last_column());
        self.focus_match_from(bottom, SearchDirection::Older);
    }

    /// Moves to the next match in `direction`, wrapping around at the ends.
    pub fn search_next(&mut self, direction: SearchDirection) {
        let Some(search) = &self.search else {
            return;
        };
        let origin = match (&search.focused, direction) {
            (Some(focused), SearchDirection::Older) => {
                focused.start().sub(&self.term, Boundary::None, 1)
            }
            (Some(focused), SearchDirection::Newer) => {
                focused.end().add(&self.term, Boundary::None, 1)
            }
            (None, SearchDirection::Older) => {
                Point::new(self.term.bottommost_line(), self.term.last_column())
            }
            (None, SearchDirection::Newer) => Point::new(self.term.topmost_line(), Column(0)),
        };
        self.focus_match_from(origin, direction);
    }

    /// Ends the search.
    pub fn clear_search(&mut self) {
        self.search = None;
    }

    /// The search's progress, if one is running.
    pub fn search_status(&self) -> Option<SearchStatus> {
        self.search.as_ref().map(|search| search.status)
    }

    fn focus_match_from(&mut self, origin: Point, direction: SearchDirection) {
        let Some(search) = &mut self.search else {
            return;
        };
        let (direction, side) = match direction {
            SearchDirection::Older => (Direction::Left, Side::Left),
            SearchDirection::Newer => (Direction::Right, Side::Right),
        };
        search.focused = self
            .term
            .search_next(&mut search.regex, origin, direction, side, None);
        if let Some(focused) = &search.focused {
            self.term.scroll_to_point(*focused.start());
        }
        self.refresh_search_status();
    }

    /// Counts the matches and finds the focused one's position among them.
    fn refresh_search_status(&mut self) {
        let Some(search) = &mut self.search else {
            return;
        };
        let start = Point::new(self.term.topmost_line(), Column(0));
        let end = Point::new(self.term.bottommost_line(), self.term.last_column());
        let mut total = 0;
        let mut current = None;
        let mut is_capped = false;
        for found in RegexIter::new(start, end, Direction::Right, &self.term, &mut search.regex) {
            if total == MAX_COUNTED_MATCHES {
                is_capped = true;
                break;
            }
            total += 1;
            if search.focused.as_ref() == Some(&found) {
                current = Some(total);
            }
        }
        search.status = SearchStatus {
            total,
            current,
            is_capped,
        };
    }

    /// The search matches on the visible screen, and the focused one if it is visible.
    fn visible_matches(&mut self) -> (Vec<VisibleRange>, Option<VisibleRange>) {
        let Some(search) = &mut self.search else {
            return (Vec::new(), None);
        };
        let offset = i32::try_from(self.term.grid().display_offset()).unwrap_or(i32::MAX);
        let top = Line(-offset);
        let bottom = top + (self.term.screen_lines().saturating_sub(1));
        let start = Point::new(top, Column(0));
        let end = Point::new(bottom, self.term.last_column());
        let found: Vec<Match> =
            RegexIter::new(start, end, Direction::Right, &self.term, &mut search.regex).collect();
        let focused = search.focused.clone();
        let visible = |found: &Match| self.visible_range(*found.start(), *found.end(), false);
        let matches = found.iter().filter_map(&visible).collect();
        let focused = focused.as_ref().and_then(visible);
        (matches, focused)
    }

    /// Starts a new selection at a visible cell, replacing any existing one.
    pub fn start_selection(&mut self, kind: SelectionKind, at: GridPoint, side: CellSide) {
        let kind = match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Word => SelectionType::Semantic,
            SelectionKind::Line => SelectionType::Lines,
            SelectionKind::Block => SelectionType::Block,
        };
        self.term.selection = Some(Selection::new(kind, self.grid_point(at), side_of(side)));
    }

    /// Moves the free end of the selection to a visible cell. Does nothing without a selection.
    pub fn extend_selection(&mut self, to: GridPoint, side: CellSide) {
        let point = self.grid_point(to);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side_of(side));
        }
    }

    /// Removes the selection.
    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// The selected text, or `None` if nothing is selected. Lines that the terminal wrapped are
    /// joined; lines the program ended are separated by `\n`.
    pub fn selection_text(&self) -> Option<String> {
        self.term
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// A visible cell as a point in the emulator's grid, clamped onto the screen.
    fn grid_point(&self, at: GridPoint) -> Point {
        let row = at.row.min(self.term.screen_lines().saturating_sub(1));
        let column = at.column.min(self.term.columns().saturating_sub(1));
        viewport_to_point(
            self.term.grid().display_offset(),
            Point::new(row, Column(column)),
        )
    }

    /// The part of the grid range `start..=end` on the visible screen.
    fn visible_range(&self, start: Point, end: Point, is_block: bool) -> Option<VisibleRange> {
        let display_offset = self.term.grid().display_offset();
        let rows = self.term.screen_lines();
        let last_column = self.term.columns().saturating_sub(1);
        let offset = i64::try_from(display_offset).ok()?;
        let to_row = |line: i32| i64::from(line) + offset;
        let (start_row, end_row) = (to_row(start.line.0), to_row(end.line.0));
        let last_row = i64::try_from(rows).ok()? - 1;
        if end_row < 0 || start_row > last_row {
            return None;
        }
        // Rows cut off above or below the screen continue from the first column or to the
        // last, except in a block selection, which keeps its columns.
        let first = if start_row < 0 {
            let column = if is_block { start.column.0 } else { 0 };
            GridPoint::new(0, column)
        } else {
            GridPoint::new(usize::try_from(start_row).ok()?, start.column.0)
        };
        let last = if end_row > last_row {
            let column = if is_block { end.column.0 } else { last_column };
            GridPoint::new(usize::try_from(last_row).ok()?, column)
        } else {
            GridPoint::new(usize::try_from(end_row).ok()?, end.column.0)
        };
        Some(VisibleRange {
            start: first,
            end: last,
            is_block,
        })
    }

    /// Turns an emulator event into one for our owner, answering colour and size queries on the
    /// way. Events the owner doesn't need return `None`.
    fn translate(&self, event: Event) -> Option<Effect> {
        match event {
            Event::PtyWrite(text) => Some(Effect::Reply(text.into_bytes())),
            // Programs clear the title with an empty `OSC 2`; treat that as a reset.
            Event::Title(title) if title.is_empty() => {
                Some(Effect::Event(TerminalEvent::TitleChanged(None)))
            }
            Event::Title(title) => Some(Effect::Event(TerminalEvent::TitleChanged(Some(title)))),
            Event::ResetTitle => Some(Effect::Event(TerminalEvent::TitleChanged(None))),
            Event::Bell => Some(Effect::Event(TerminalEvent::Bell)),
            Event::ColorRequest(index, format) => {
                let colors = Resolver {
                    palette: &self.palette,
                    overrides: self.term.colors(),
                };
                let rgb = colors.lookup(index);
                let reply = format(ansi::Rgb {
                    r: rgb.r,
                    g: rgb.g,
                    b: rgb.b,
                });
                Some(Effect::Reply(reply.into_bytes()))
            }
            Event::TextAreaSizeRequest(format) => {
                let size = self.size;
                let reply = format(WindowSize {
                    num_lines: size.rows(),
                    num_cols: size.columns(),
                    cell_width: size.cell_width(),
                    cell_height: size.cell_height(),
                });
                Some(Effect::Reply(reply.into_bytes()))
            }
            // OSC 52 is disabled in `new`, so the clipboard events never arrive. Redraws are
            // driven by snapshots and the pty reports exit itself; the rest are cursor-shape
            // hints the snapshot already carries.
            Event::ClipboardStore(..)
            | Event::ClipboardLoad(..)
            | Event::MouseCursorDirty
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Exit
            | Event::ChildExit(_) => None,
        }
    }
}

fn side_of(side: CellSide) -> Side {
    match side {
        CellSide::Left => Side::Left,
        CellSide::Right => Side::Right,
    }
}

/// What one emulator event asks of the terminal's owner.
enum Effect {
    Reply(Vec<u8>),
    Event(TerminalEvent),
}

/// Collects emulator events so [`Terminal::drain_events`] can hand them out after a feed.
struct EventQueue(mpsc::Sender<Event>);

impl EventListener for EventQueue {
    fn send_event(&self, event: Event) {
        // The receiver lives in the same `Terminal` as the `Term` that owns this sender, so it
        // can't have been dropped; there is nothing to report if it somehow had.
        let _ = self.0.send(event);
    }
}

/// [`TerminalSize`] in the shape the emulator expects.
struct GridSize(TerminalSize);

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.0.rows())
    }

    fn columns(&self) -> usize {
        usize::from(self.0.columns())
    }
}

/// Resolves cell colours: colours the program redefined (`OSC 4/10/11/12`) win over the palette.
struct Resolver<'a> {
    palette: &'a Palette,
    overrides: &'a Colors,
}

impl Resolver<'_> {
    /// The colour at an emulator colour index (`0..color::COUNT`), see [`Colors`].
    fn lookup(&self, index: usize) -> Rgb {
        if index < color::COUNT
            && let Some(rgb) = self.overrides[index]
        {
            return Rgb::new(rgb.r, rgb.g, rgb.b);
        }
        let p = self.palette;
        match index {
            0..=255 => u8::try_from(index).map_or(p.foreground, |i| p.indexed(i)),
            i if i == NamedColor::Background as usize => p.background,
            i if i == NamedColor::Cursor as usize => p.cursor,
            i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
                let normal = i - NamedColor::DimBlack as usize;
                u8::try_from(normal)
                    .map_or(p.foreground, |n| p.indexed(n))
                    .dim()
            }
            i if i == NamedColor::DimForeground as usize => p.foreground.dim(),
            // Foreground, bright foreground and anything unexpected.
            _ => p.foreground,
        }
    }

    fn cell(&self, cell: &Cell) -> SnapshotCell {
        let flags = cell.flags;
        let mut fg = self.foreground(cell.fg, flags);
        let mut bg = self.background(cell.bg);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        SnapshotCell {
            ch: cell.c,
            zerowidth: cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
            fg,
            bg,
            style: CellStyle(flags),
        }
    }

    /// Text colour. Bold text in one of the eight normal colours is drawn in its bright
    /// version, as most terminals do by default; dim text is darkened.
    fn foreground(&self, color: Color, flags: Flags) -> Rgb {
        let is_bold = flags.contains(Flags::BOLD);
        let is_dim = flags.contains(Flags::DIM);
        match color {
            Color::Spec(rgb) => {
                let rgb = Rgb::new(rgb.r, rgb.g, rgb.b);
                if is_dim { rgb.dim() } else { rgb }
            }
            Color::Named(named) => {
                let named = match (is_bold, is_dim) {
                    (true, false) => named.to_bright(),
                    (_, true) => named.to_dim(),
                    (false, false) => named,
                };
                self.lookup(named as usize)
            }
            Color::Indexed(index) => match (is_bold, is_dim, index) {
                (true, false, 0..=7) => self.lookup(usize::from(index) + 8),
                (false, true, 0..=7) => {
                    self.lookup(NamedColor::DimBlack as usize + usize::from(index))
                }
                (_, true, _) => self.lookup(usize::from(index)).dim(),
                _ => self.lookup(usize::from(index)),
            },
        }
    }

    fn background(&self, color: Color) -> Rgb {
        match color {
            Color::Spec(rgb) => Rgb::new(rgb.r, rgb.g, rgb.b),
            Color::Named(named) => self.lookup(named as usize),
            Color::Indexed(index) => self.lookup(usize::from(index)),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::search::SearchDirection;
    use crate::snapshot::{MouseEncoding, MouseReporting, Underline};

    fn terminal(columns: u16, rows: u16) -> Terminal {
        Terminal::new(&TerminalConfig::default(), TerminalSize::new(columns, rows))
    }

    /// The text of a visible row, without wide-character spacers or trailing blanks.
    fn row_text(snapshot: &Snapshot, row: usize) -> String {
        let line = snapshot.lines().nth(row).unwrap();
        let text: String = line
            .iter()
            .filter(|cell| !cell.style.is_spacer())
            .flat_map(|cell| std::iter::once(cell.ch).chain(cell.zerowidth.iter().copied()))
            .collect();
        text.trim_end().to_owned()
    }

    fn screen_text(snapshot: &Snapshot) -> Vec<String> {
        (0..snapshot.rows())
            .map(|row| row_text(snapshot, row))
            .collect()
    }

    #[test]
    fn snapshot_has_one_cell_per_position() {
        let snapshot = terminal(7, 3).snapshot();
        assert_eq!((snapshot.columns(), snapshot.rows()), (7, 3));
        assert_eq!(snapshot.lines().len(), 3);
        assert!(snapshot.lines().all(|line| line.len() == 7));
        assert!(snapshot.cell(2, 6).is_some());
        assert!(snapshot.cell(3, 0).is_none());
        assert!(snapshot.cell(0, 7).is_none());
    }

    #[test]
    fn text_lands_on_the_grid_and_moves_the_cursor() {
        let mut term = terminal(10, 3);
        term.feed(b"hello\r\nworld");
        let snapshot = term.snapshot();

        assert_eq!(screen_text(&snapshot), ["hello", "world", ""]);
        let cursor = snapshot.cursor.unwrap();
        assert_eq!((cursor.row, cursor.column), (1, 5));
        assert_eq!(cursor.shape, CursorShape::Block);
    }

    #[test]
    fn output_split_across_feeds_is_parsed_as_one_stream() {
        let mut term = terminal(10, 2);
        // An SGR sequence and a UTF-8 character both cut in half.
        term.feed(b"\x1b[3");
        term.feed(b"1mr\xc3");
        term.feed(b"\xa9");
        let snapshot = term.snapshot();

        assert_eq!(row_text(&snapshot, 0), "r\u{e9}");
        assert_eq!(snapshot.cell(0, 0).unwrap().fg, term.palette().ansi[1]);
    }

    #[test]
    fn colours_resolve_against_the_palette() {
        let palette = Palette::default();
        let cases: [(&str, &[u8], Rgb, Rgb); 10] = [
            ("default", b"x", palette.foreground, palette.background),
            ("red", b"\x1b[31mx", palette.ansi[1], palette.background),
            (
                "bold red is bright",
                b"\x1b[1;31mx",
                palette.ansi[9],
                palette.background,
            ),
            (
                "bright via 90s",
                b"\x1b[92mx",
                palette.ansi[10],
                palette.background,
            ),
            (
                "dim red",
                b"\x1b[2;31mx",
                palette.ansi[1].dim(),
                palette.background,
            ),
            (
                "256 colour",
                b"\x1b[38;5;196mx",
                Rgb::new(255, 0, 0),
                palette.background,
            ),
            (
                "bold indexed 1 is bright",
                b"\x1b[1;38;5;1mx",
                palette.ansi[9],
                palette.background,
            ),
            (
                "truecolor",
                b"\x1b[38;2;1;2;3;48;2;4;5;6mx",
                Rgb::new(1, 2, 3),
                Rgb::new(4, 5, 6),
            ),
            (
                "inverse",
                b"\x1b[7;31;42mx",
                palette.ansi[2],
                palette.ansi[1],
            ),
            (
                "hidden",
                b"\x1b[8;31;42mx",
                palette.ansi[2],
                palette.ansi[2],
            ),
        ];
        for (name, input, fg, bg) in cases {
            let mut term = terminal(10, 2);
            term.feed(input);
            let snapshot = term.snapshot();
            let cell = snapshot.cell(0, 0).unwrap();
            assert_eq!((cell.fg, cell.bg), (fg, bg), "{name}");
        }
    }

    #[test]
    fn palette_changes_apply_to_the_next_snapshot() {
        let mut term = terminal(10, 2);
        term.feed(b"\x1b[34mx");
        let mut palette = Palette::default();
        palette.ansi[4] = Rgb::new(1, 2, 3);
        palette.background = Rgb::new(9, 9, 9);
        term.set_palette(palette);

        let snapshot = term.snapshot();
        assert_eq!(snapshot.cell(0, 0).unwrap().fg, Rgb::new(1, 2, 3));
        assert_eq!(snapshot.background, Rgb::new(9, 9, 9));
    }

    #[test]
    fn programs_can_redefine_colours() {
        let mut term = terminal(10, 2);
        // OSC 4 redefines index 1, OSC 11 the background.
        term.feed(b"\x1b]4;1;rgb:10/20/30\x07\x1b]11;rgb:01/02/03\x07\x1b[31mx");
        let snapshot = term.snapshot();
        assert_eq!(snapshot.cell(0, 0).unwrap().fg, Rgb::new(0x10, 0x20, 0x30));
        assert_eq!(snapshot.background, Rgb::new(1, 2, 3));
    }

    #[test]
    fn text_styles_are_reported() {
        let mut term = terminal(10, 2);
        term.feed(b"\x1b[1mb\x1b[0;3mi\x1b[0;4mu\x1b[0;4:3mc\x1b[0;9ms\x1b[0mn");
        let snapshot = term.snapshot();
        let style = |column| snapshot.cell(0, column).unwrap().style;

        assert!(style(0).is_bold());
        assert!(style(1).is_italic());
        assert_eq!(style(2).underline(), Some(Underline::Single));
        assert_eq!(style(3).underline(), Some(Underline::Curly));
        assert!(style(4).is_strikeout());
        let plain = style(5);
        assert!(!plain.is_bold() && !plain.is_italic() && !plain.is_strikeout());
        assert_eq!(plain.underline(), None);
    }

    #[test]
    fn wide_and_combining_characters() {
        let mut term = terminal(10, 2);
        term.feed("日x e\u{301}".as_bytes());
        let snapshot = term.snapshot();

        let wide = snapshot.cell(0, 0).unwrap();
        assert_eq!(wide.ch, '日');
        assert!(wide.style.is_wide());
        assert!(snapshot.cell(0, 1).unwrap().style.is_spacer());
        assert_eq!(snapshot.cell(0, 2).unwrap().ch, 'x');
        assert_eq!(snapshot.cell(0, 4).unwrap().zerowidth, ['\u{301}']);
        assert_eq!(row_text(&snapshot, 0), "日x e\u{301}");
    }

    #[test]
    fn long_lines_wrap_and_reflow_on_resize() {
        let mut term = terminal(5, 4);
        term.feed(b"abcdefgh");
        assert_eq!(screen_text(&term.snapshot()), ["abcde", "fgh", "", ""]);

        term.resize(TerminalSize::new(10, 4));
        assert_eq!(term.size().columns(), 10);
        assert_eq!(screen_text(&term.snapshot()), ["abcdefgh", "", "", ""]);

        // Narrowing keeps the cursor's row, so the first half of the line moves to scrollback.
        term.resize(TerminalSize::new(4, 4));
        let snapshot = term.snapshot();
        assert_eq!(screen_text(&snapshot), ["efgh", "", "", ""]);
        assert_eq!(snapshot.history_size, 1);
    }

    #[test]
    fn lines_scrolled_off_the_top_go_to_scrollback() {
        let mut term = terminal(10, 3);
        for i in 0..10 {
            term.feed(format!("line {i}\r\n").as_bytes());
        }
        let snapshot = term.snapshot();
        assert_eq!(snapshot.history_size, 8);
        assert_eq!(snapshot.display_offset, 0);
        assert_eq!(screen_text(&snapshot), ["line 8", "line 9", ""]);
    }

    #[test]
    fn scrollback_is_limited_by_config() {
        let config = TerminalConfig {
            scrollback_lines: 5,
            ..TerminalConfig::default()
        };
        let mut term = Terminal::new(&config, TerminalSize::new(10, 3));
        for i in 0..50 {
            term.feed(format!("{i}\r\n").as_bytes());
        }
        assert_eq!(term.snapshot().history_size, 5);
    }

    #[test]
    fn alternate_screen_hides_the_main_screen_and_restores_it() {
        let mut term = terminal(10, 2);
        term.feed(b"main");
        term.feed(b"\x1b[?1049h");
        let snapshot = term.snapshot();
        assert!(snapshot.modes.is_alt_screen());
        assert_eq!(screen_text(&snapshot), ["", ""]);

        term.feed(b"\x1b[?1049l");
        let snapshot = term.snapshot();
        assert!(!snapshot.modes.is_alt_screen());
        assert_eq!(row_text(&snapshot, 0), "main");
    }

    #[test]
    fn modes_follow_the_program() {
        let mut term = terminal(10, 2);
        let initial = term.modes();
        assert!(!initial.is_app_cursor());
        assert!(!initial.is_bracketed_paste());
        assert_eq!(initial.mouse_reporting(), MouseReporting::Off);
        assert_eq!(initial.mouse_encoding(), MouseEncoding::Default);
        assert!(initial.is_alternate_scroll());

        term.feed(b"\x1b[?1h\x1b=\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h\x1b[20h");
        let modes = term.modes();
        assert!(modes.is_app_cursor());
        assert!(modes.is_app_keypad());
        assert!(modes.is_bracketed_paste());
        assert!(modes.is_focus_reporting());
        assert!(modes.is_line_feed_new_line());
        assert_eq!(modes.mouse_reporting(), MouseReporting::Drag);
        assert_eq!(modes.mouse_encoding(), MouseEncoding::Sgr);

        term.feed(b"\x1b[?1003h");
        assert_eq!(term.modes().mouse_reporting(), MouseReporting::Motion);
        term.feed(b"\x1b[?1003l\x1b[?1002l\x1b[?1000h\x1b[?1006l\x1b[?1005h");
        assert_eq!(term.modes().mouse_reporting(), MouseReporting::Click);
        assert_eq!(term.modes().mouse_encoding(), MouseEncoding::Utf8);
        assert_eq!(term.snapshot().modes, term.modes());
    }

    #[test]
    fn cursor_shape_and_visibility() {
        type Case<'a> = (&'a str, &'a [u8], Option<(CursorShape, bool)>);
        let cases: [Case; 5] = [
            ("default", b"", Some((CursorShape::Block, false))),
            (
                "blinking beam",
                b"\x1b[5 q",
                Some((CursorShape::Beam, true)),
            ),
            (
                "steady underline",
                b"\x1b[4 q",
                Some((CursorShape::Underline, false)),
            ),
            (
                "blinking block",
                b"\x1b[1 q",
                Some((CursorShape::Block, true)),
            ),
            ("hidden", b"\x1b[?25l", None),
        ];
        for (name, input, expected) in cases {
            let mut term = terminal(10, 2);
            term.feed(input);
            let cursor = term.snapshot().cursor;
            assert_eq!(cursor.map(|c| (c.shape, c.is_blinking)), expected, "{name}");
        }
    }

    #[test]
    fn title_and_bell_events() {
        let mut term = terminal(10, 2);
        term.feed(b"\x1b]0;build: ok\x07\x07\x1b]2;\x07");
        assert_eq!(
            term.drain_events(),
            [
                TerminalEvent::TitleChanged(Some("build: ok".into())),
                TerminalEvent::Bell,
                TerminalEvent::TitleChanged(None),
            ]
        );
        assert_eq!(term.drain_events(), []);
    }

    #[test]
    fn queries_are_answered_with_replies() {
        let palette = Palette {
            background: Rgb::new(0x12, 0x34, 0x56),
            ..Palette::default()
        };
        let size = TerminalSize::new(20, 5).with_cell_size(8, 16);
        let cases: [(&str, &[u8], &[u8]); 4] = [
            ("cursor position", b"ab\x1b[6n", b"\x1b[1;3R"),
            (
                "background colour",
                b"\x1b]11;?\x07",
                b"\x1b]11;rgb:1212/3434/5656\x07",
            ),
            ("text area in cells", b"\x1b[18t", b"\x1b[8;5;20t"),
            ("text area in pixels", b"\x1b[14t", b"\x1b[4;80;160t"),
        ];
        for (name, query, reply) in cases {
            let mut term = Terminal::new(&TerminalConfig::default(), size);
            term.set_palette(palette.clone());
            term.feed(query);
            assert_eq!(term.take_reply(), reply, "{name}");
            assert_eq!(term.drain_events(), [], "{name}");
            assert_eq!(term.take_reply(), b"", "{name}");
        }
    }

    fn select(
        term: &mut Terminal,
        kind: SelectionKind,
        from: (usize, usize, CellSide),
        to: (usize, usize, CellSide),
    ) {
        term.start_selection(kind, GridPoint::new(from.0, from.1), from.2);
        term.extend_selection(GridPoint::new(to.0, to.1), to.2);
    }

    #[test]
    fn selection_kinds_select_the_expected_text() {
        use CellSide::{Left, Right};
        let screen = b"ls -la /usr/local\r\nsecond line here\r\nthird";
        let cases = [
            (
                "simple",
                SelectionKind::Simple,
                (0, 3, Left),
                (0, 5, Right),
                Some("-la"),
            ),
            (
                "simple across lines",
                SelectionKind::Simple,
                (0, 7, Left),
                (1, 5, Right),
                Some("/usr/local\nsecond"),
            ),
            (
                "backwards",
                SelectionKind::Simple,
                (0, 5, Right),
                (0, 3, Left),
                Some("-la"),
            ),
            (
                "word is a path",
                SelectionKind::Word,
                (0, 10, Left),
                (0, 10, Left),
                Some("/usr/local"),
            ),
            (
                "line",
                SelectionKind::Line,
                (1, 3, Left),
                (1, 3, Left),
                Some("second line here\n"),
            ),
            (
                "block",
                SelectionKind::Block,
                (0, 0, Left),
                (2, 1, Right),
                Some("ls\nse\nth"),
            ),
            (
                "a click selects nothing",
                SelectionKind::Simple,
                (0, 2, Left),
                (0, 2, Left),
                None,
            ),
        ];
        for (name, kind, from, to, expected) in cases {
            let mut term = terminal(20, 4);
            term.feed(screen);
            select(&mut term, kind, from, to);
            assert_eq!(term.selection_text().as_deref(), expected, "{name}");
        }
    }

    #[test]
    fn wrapped_lines_copy_as_one() {
        let mut term = terminal(5, 4);
        term.feed(b"abcdefgh\r\nxy");
        select(
            &mut term,
            SelectionKind::Line,
            (0, 0, CellSide::Left),
            (1, 0, CellSide::Left),
        );
        assert_eq!(term.selection_text().as_deref(), Some("abcdefgh\n"));
    }

    #[test]
    fn snapshot_shows_the_visible_part_of_the_selection() {
        let mut term = terminal(10, 3);
        term.feed(b"one\r\ntwo\r\nthree");
        assert_eq!(term.snapshot().selection, None);

        select(
            &mut term,
            SelectionKind::Simple,
            (0, 1, CellSide::Left),
            (1, 1, CellSide::Right),
        );
        assert_eq!(
            term.snapshot().selection,
            Some(VisibleRange {
                start: GridPoint::new(0, 1),
                end: GridPoint::new(1, 1),
                is_block: false,
            })
        );

        // Output scrolls the selection up with its text; the part above the screen is cut off.
        term.feed(b"\r\nfour");
        assert_eq!(
            term.snapshot().selection,
            Some(VisibleRange {
                start: GridPoint::new(0, 0),
                end: GridPoint::new(0, 1),
                is_block: false,
            })
        );
        assert_eq!(term.selection_text().as_deref(), Some("ne\ntw"));

        // Scrolled entirely off the screen: nothing to draw, but still copyable.
        term.feed(b"\r\nfive\r\nsix");
        assert_eq!(term.snapshot().selection, None);
        assert_eq!(term.selection_text().as_deref(), Some("ne\ntw"));

        term.clear_selection();
        assert_eq!(term.selection_text(), None);
    }

    #[test]
    fn selection_points_are_clamped_to_the_screen() {
        let mut term = terminal(5, 2);
        term.feed(b"abc");
        select(
            &mut term,
            SelectionKind::Simple,
            (9, 9, CellSide::Left),
            (0, 0, CellSide::Left),
        );
        assert_eq!(term.selection_text().as_deref(), Some("abc"));
        // Extending without a selection is ignored.
        term.clear_selection();
        term.extend_selection(GridPoint::new(0, 1), CellSide::Right);
        assert_eq!(term.selection_text(), None);
    }

    /// A 10×3 terminal that printed `line 0` … `line 9`, leaving 8 lines of scrollback.
    fn with_history() -> Terminal {
        let mut term = terminal(10, 3);
        for i in 0..10 {
            term.feed(format!("line {i}\r\n").as_bytes());
        }
        term
    }

    #[test]
    fn scrolling_moves_the_view_through_history() {
        let cases = [
            ("three lines up", vec![ScrollAmount::Lines(3)], 3, "line 5"),
            ("page up", vec![ScrollAmount::PageUp], 3, "line 5"),
            ("top", vec![ScrollAmount::Top], 8, "line 0"),
            ("past the top", vec![ScrollAmount::Lines(100)], 8, "line 0"),
            (
                "down again",
                vec![ScrollAmount::Top, ScrollAmount::Lines(-2)],
                6,
                "line 2",
            ),
            (
                "page down",
                vec![ScrollAmount::Top, ScrollAmount::PageDown],
                5,
                "line 3",
            ),
            (
                "bottom",
                vec![ScrollAmount::Top, ScrollAmount::Bottom],
                0,
                "line 8",
            ),
        ];
        for (name, scrolls, offset, first_row) in cases {
            let mut term = with_history();
            for scroll in scrolls {
                term.scroll(scroll);
            }
            let snapshot = term.snapshot();
            assert_eq!(snapshot.display_offset, offset, "{name}");
            assert_eq!(row_text(&snapshot, 0), first_row, "{name}");
            assert_eq!(term.is_scrolled_back(), offset > 0, "{name}");
        }
    }

    #[test]
    fn the_view_stays_on_its_text_while_output_arrives() {
        let mut term = with_history();
        term.scroll(ScrollAmount::Lines(2));
        term.feed(b"new\r\n");
        let snapshot = term.snapshot();
        assert_eq!(row_text(&snapshot, 0), "line 6");
        // The cursor is below the view.
        assert_eq!(snapshot.cursor, None);
    }

    #[test]
    fn clear_scrollback_keeps_the_screen() {
        let mut term = with_history();
        term.scroll(ScrollAmount::Top);
        term.clear_scrollback();
        let snapshot = term.snapshot();
        assert_eq!((snapshot.history_size, snapshot.display_offset), (0, 0));
        assert_eq!(screen_text(&snapshot), ["line 8", "line 9", ""]);
    }

    #[test]
    fn reset_clears_everything_and_restores_modes() {
        let mut term = with_history();
        term.feed(b"\x1b[?1h\x1b[?2004h\x1b[31mred");
        term.set_search("line");
        term.reset();
        let snapshot = term.snapshot();
        assert_eq!(snapshot.history_size, 0);
        assert_eq!(screen_text(&snapshot), ["", "", ""]);
        assert!(!snapshot.modes.is_app_cursor());
        assert!(!snapshot.modes.is_bracketed_paste());
        assert_eq!(snapshot.search_status, None);
        term.feed(b"x");
        assert_eq!(
            term.snapshot().cell(0, 0).unwrap().fg,
            term.palette().foreground
        );
    }

    #[test]
    fn search_finds_matches_in_the_scrollback_newest_first() {
        let mut term = with_history();
        term.set_search("LINE 2");
        assert_eq!(
            term.search_status().unwrap().total,
            0,
            "smart case: upper case is exact"
        );

        term.set_search("line");
        let status = term.search_status().unwrap();
        assert_eq!(
            (status.total, status.current, status.is_capped),
            (10, Some(10), false)
        );
        // The newest match is on screen and focused.
        let snapshot = term.snapshot();
        assert_eq!(snapshot.search_matches.len(), 2);
        assert_eq!(
            snapshot.focused_match,
            Some(VisibleRange {
                start: GridPoint::new(1, 0),
                end: GridPoint::new(1, 3),
                is_block: false,
            })
        );

        // Going older scrolls the view to keep the match visible.
        for _ in 0..5 {
            term.search_next(SearchDirection::Older);
        }
        let snapshot = term.snapshot();
        assert_eq!(snapshot.search_status.unwrap().current, Some(5));
        let focused = snapshot.focused_match.expect("focused match is visible");
        assert_eq!(row_text(&snapshot, focused.start.row), "line 4");
        assert!(snapshot.display_offset > 0);

        term.search_next(SearchDirection::Newer);
        assert_eq!(term.search_status().unwrap().current, Some(6));
    }

    #[test]
    fn search_wraps_and_is_literal() {
        let mut term = terminal(20, 3);
        term.feed(b"a.c abc a.c");
        term.set_search("a.c");
        assert_eq!(term.search_status().unwrap().total, 2, "the dot is literal");
        assert_eq!(term.search_status().unwrap().current, Some(2));
        term.search_next(SearchDirection::Newer);
        assert_eq!(
            term.search_status().unwrap().current,
            Some(1),
            "wraps to the oldest"
        );
        term.search_next(SearchDirection::Older);
        assert_eq!(
            term.search_status().unwrap().current,
            Some(2),
            "wraps to the newest"
        );

        term.set_search("zzz");
        let status = term.search_status().unwrap();
        assert_eq!((status.total, status.current), (0, None));
        assert_eq!(term.snapshot().search_matches, []);

        term.set_search("");
        assert_eq!(term.search_status(), None);
        term.set_search("abc");
        term.clear_search();
        assert_eq!(term.snapshot().search_status, None);
    }

    #[test]
    fn replies_and_events_are_kept_apart() {
        let mut term = terminal(10, 2);
        term.feed(b"\x07\x1b[6n\x1b]0;t\x07\x1b[6n");
        assert_eq!(term.take_reply(), b"\x1b[1;1R\x1b[1;1R");
        assert_eq!(
            term.drain_events(),
            [
                TerminalEvent::Bell,
                TerminalEvent::TitleChanged(Some("t".into()))
            ]
        );
    }

    #[test]
    fn osc_52_clipboard_requests_are_ignored() {
        let mut term = terminal(10, 2);
        // "aGk=" is base64 for "hi".
        term.feed(b"\x1b]52;c;aGk=\x07\x1b]52;c;?\x07");
        assert_eq!(term.drain_events(), []);
        assert_eq!(term.take_reply(), b"");
    }

    #[test]
    fn synchronized_updates_hold_output_until_ended() {
        let mut term = terminal(10, 2);
        term.feed(b"\x1b[?2026hhidden");
        assert!(term.sync_deadline().is_some());
        assert_eq!(row_text(&term.snapshot(), 0), "");

        term.end_sync();
        assert!(term.sync_deadline().is_none());
        assert_eq!(row_text(&term.snapshot(), 0), "hidden");

        term.feed(b"\x1b[?2026h!\x1b[?2026l");
        assert!(term.sync_deadline().is_none());
        assert_eq!(row_text(&term.snapshot(), 0), "hidden!");
    }
}
