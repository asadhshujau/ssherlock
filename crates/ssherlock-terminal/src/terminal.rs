//! The terminal emulator: bytes from a program in, a [`Snapshot`] and [`TerminalEvent`]s out.
//!
//! [`Terminal`] wraps `alacritty_terminal`'s parser and grid and does no I/O, so it behaves the
//! same whether the bytes come from a local pty or an SSH channel, and tests can drive it with
//! plain byte strings. Nothing from `alacritty_terminal` appears in this crate's public API.

use std::sync::mpsc;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::{self, Colors};
use alacritty_terminal::term::{Config, Osc52, Term, point_to_viewport};
use alacritty_terminal::vte::ansi::{self, Color, CursorShape as VteCursorShape, NamedColor};

use crate::palette::{Palette, Rgb};
use crate::pty::PtyExit;
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

/// A terminal emulator for one program.
pub struct Terminal {
    term: Term<EventQueue>,
    parser: ansi::Processor,
    events: mpsc::Receiver<Event>,
    pending_events: Vec<TerminalEvent>,
    pending_reply: Vec<u8>,
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
    pub fn snapshot(&self) -> Snapshot {
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
        }
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
