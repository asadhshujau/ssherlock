//! Encoding user input as the bytes a terminal program expects: keys, paste, focus and mouse.
//!
//! These are pure functions of the input and the program's [`TerminalModes`], following xterm,
//! so they're testable without a UI. The view maps its toolkit's key and mouse events onto
//! [`Key`], [`Modifiers`] and [`MouseInput`] and sends whatever comes back. Platform choices,
//! such as whether macOS Option acts as Meta, are made by the view before calling in: here
//! `alt` always means "prefix with ESC".

use crate::snapshot::{MouseEncoding, MouseReporting, TerminalModes};

/// A key the terminal knows how to encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A key that types a character, as produced by the keyboard layout. Pass the unshifted
    /// character with `shift` for Ctrl combinations (`Ctrl+Shift+A` is `Char('a')`).
    Char(char),
    /// Return or keypad Enter.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Insert.
    Insert,
    /// Forward delete.
    Delete,
    /// F1 to F12; other numbers encode as nothing.
    Function(u8),
}

/// Modifier keys held with a key or mouse event.
///
/// The platform key (Cmd, Super) isn't here: it's for app shortcuts and never reaches the
/// program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Shift is held.
    pub shift: bool,
    /// Alt, or Option acting as Meta, is held.
    pub alt: bool,
    /// Control is held.
    pub control: bool,
}

impl Modifiers {
    /// No modifiers.
    pub const NONE: Self = Self {
        shift: false,
        alt: false,
        control: false,
    };

    /// xterm's modifier parameter: 1 plus Shift=1, Alt=2, Ctrl=4. 1 means none.
    fn parameter(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.control)
    }

    fn is_empty(self) -> bool {
        self == Self::NONE
    }
}

const ESC: u8 = 0x1b;

/// The bytes for a key press, or `None` if the key sends nothing.
pub fn encode_key(key: Key, modifiers: Modifiers, modes: TerminalModes) -> Option<Vec<u8>> {
    let alt_prefixed = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if modifiers.alt {
            out.push(ESC);
        }
        out.extend_from_slice(bytes);
        out
    };

    match key {
        Key::Char(ch) => {
            if modifiers.control
                && let Some(byte) = control_byte(ch)
            {
                return Some(alt_prefixed(&[byte]));
            }
            let mut buffer = [0; 4];
            Some(alt_prefixed(ch.encode_utf8(&mut buffer).as_bytes()))
        }
        Key::Enter if modes.is_line_feed_new_line() => Some(alt_prefixed(b"\r\n")),
        Key::Enter => Some(alt_prefixed(b"\r")),
        Key::Tab if modifiers.shift => Some(b"\x1b[Z".to_vec()),
        Key::Tab => Some(alt_prefixed(b"\t")),
        // Ctrl+Backspace sends BS, which shells bind to "delete word" just like Alt+Backspace.
        Key::Backspace if modifiers.control => Some(alt_prefixed(b"\x08")),
        Key::Backspace => Some(alt_prefixed(b"\x7f")),
        Key::Escape => Some(alt_prefixed(&[ESC])),
        Key::Up => Some(cursor_key(b'A', modifiers, modes)),
        Key::Down => Some(cursor_key(b'B', modifiers, modes)),
        Key::Right => Some(cursor_key(b'C', modifiers, modes)),
        Key::Left => Some(cursor_key(b'D', modifiers, modes)),
        Key::Home => Some(cursor_key(b'H', modifiers, modes)),
        Key::End => Some(cursor_key(b'F', modifiers, modes)),
        Key::Insert => Some(tilde_key(2, modifiers)),
        Key::Delete => Some(tilde_key(3, modifiers)),
        Key::PageUp => Some(tilde_key(5, modifiers)),
        Key::PageDown => Some(tilde_key(6, modifiers)),
        Key::Function(n @ 1..=4) => {
            // F1–F4 are SS3 P–S unmodified, CSI 1;m P–S with modifiers.
            let letter = b'P' + (n - 1);
            Some(if modifiers.is_empty() {
                vec![ESC, b'O', letter]
            } else {
                csi_modified(1, modifiers.parameter(), letter)
            })
        }
        Key::Function(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return None,
            };
            Some(tilde_key(code, modifiers))
        }
    }
}

/// The C0 control byte for Ctrl+`ch`, following xterm, or `None` if Ctrl doesn't change `ch`.
fn control_byte(ch: char) -> Option<u8> {
    let byte = u8::try_from(ch).ok()?;
    match byte {
        b'a'..=b'z' => Some(byte - b'a' + 1),
        b'A'..=b'Z' => Some(byte - b'A' + 1),
        b'@' | b' ' | b'2' => Some(0x00),
        b'[' | b'3' => Some(0x1b),
        b'\\' | b'4' => Some(0x1c),
        b']' | b'5' => Some(0x1d),
        b'^' | b'6' => Some(0x1e),
        b'_' | b'7' | b'/' => Some(0x1f),
        b'8' | b'?' => Some(0x7f),
        _ => None,
    }
}

/// Arrows, Home and End: `CSI x`, or `SS3 x` in application cursor mode, or `CSI 1;m x` when
/// modified.
fn cursor_key(letter: u8, modifiers: Modifiers, modes: TerminalModes) -> Vec<u8> {
    if !modifiers.is_empty() {
        csi_modified(1, modifiers.parameter(), letter)
    } else if modes.is_app_cursor() {
        vec![ESC, b'O', letter]
    } else {
        vec![ESC, b'[', letter]
    }
}

/// Keys of the form `CSI n ~`, or `CSI n;m ~` when modified.
fn tilde_key(code: u8, modifiers: Modifiers) -> Vec<u8> {
    if modifiers.is_empty() {
        format!("\x1b[{code}~").into_bytes()
    } else {
        csi_modified(code, modifiers.parameter(), b'~')
    }
}

fn csi_modified(code: u8, parameter: u8, last: u8) -> Vec<u8> {
    let mut out = format!("\x1b[{code};{parameter}").into_bytes();
    out.push(last);
    out
}

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// The bytes for pasting `text`.
///
/// Line endings become CR, as if Enter were pressed. When the program has bracketed paste on,
/// the text is wrapped so the program can tell it from typing, and any end marker inside the
/// text is removed so pasted content can't break out of the bracket and run as typed commands.
pub fn encode_paste(text: &str, modes: TerminalModes) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if modes.is_bracketed_paste() {
        let inner = normalized.replace(PASTE_END, "");
        format!("{PASTE_START}{inner}{PASTE_END}").into_bytes()
    } else {
        normalized.into_bytes()
    }
}

/// Whether `text` would be sent as more than one line, so pasting it deserves a confirmation.
/// A single trailing line break doesn't count.
pub fn is_multiline(text: &str) -> bool {
    let trimmed = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .or_else(|| text.strip_suffix('\r'))
        .unwrap_or(text);
    trimmed.contains(['\n', '\r'])
}

/// The bytes for the terminal gaining or losing focus, if the program asked for them.
pub fn encode_focus(is_focused: bool, modes: TerminalModes) -> Option<&'static [u8]> {
    if !modes.is_focus_reporting() {
        return None;
    }
    Some(if is_focused { b"\x1b[I" } else { b"\x1b[O" })
}

/// A mouse button, or the scroll wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Middle button or wheel click.
    Middle,
    /// Secondary button.
    Right,
    /// Wheel scrolled up one step.
    WheelUp,
    /// Wheel scrolled down one step.
    WheelDown,
}

/// What the mouse did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseAction {
    /// A button went down, or the wheel moved one step.
    Press(MouseButton),
    /// A button came up.
    Release(MouseButton),
    /// The pointer moved, with this button held, or none.
    Move(Option<MouseButton>),
}

/// A mouse event over a cell of the visible screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MouseInput {
    /// What happened.
    pub action: MouseAction,
    /// Row from the top of the visible screen, from 0.
    pub row: usize,
    /// Column from the left, from 0.
    pub column: usize,
    /// Modifiers held.
    pub modifiers: Modifiers,
}

/// Largest coordinate the original X10 encoding can carry (one byte, offset by 32).
const X10_MAX: usize = 255 - 32 - 1;

/// Largest coordinate the UTF-8 encoding can carry (two-byte UTF-8, offset by 32).
const UTF8_MAX: usize = 2047 - 32 - 1;

/// The bytes reporting a mouse event, if the program asked for this kind of event and the
/// position can be encoded.
pub fn encode_mouse(input: MouseInput, modes: TerminalModes) -> Option<Vec<u8>> {
    let reporting = modes.mouse_reporting();
    let wants = match input.action {
        MouseAction::Press(_) | MouseAction::Release(_) => reporting != MouseReporting::Off,
        MouseAction::Move(Some(_)) => {
            matches!(reporting, MouseReporting::Drag | MouseReporting::Motion)
        }
        MouseAction::Move(None) => reporting == MouseReporting::Motion,
    };
    // The wheel has no release.
    let is_wheel_release = matches!(
        input.action,
        MouseAction::Release(MouseButton::WheelUp | MouseButton::WheelDown)
    );
    if !wants || is_wheel_release {
        return None;
    }

    let button_code = |button: Option<MouseButton>| match button {
        Some(MouseButton::Left) => 0,
        Some(MouseButton::Middle) => 1,
        Some(MouseButton::Right) => 2,
        None => 3,
        Some(MouseButton::WheelUp) => 64,
        Some(MouseButton::WheelDown) => 65,
    };
    let (code, motion) = match input.action {
        MouseAction::Press(button) | MouseAction::Release(button) => (button_code(Some(button)), 0),
        MouseAction::Move(button) => (button_code(button), 32),
    };
    let modifiers = 4 * u32::from(input.modifiers.shift)
        + 8 * u32::from(input.modifiers.alt)
        + 16 * u32::from(input.modifiers.control);
    let code = code + motion + modifiers;
    let is_release = matches!(input.action, MouseAction::Release(_));
    let (x, y) = (input.column + 1, input.row + 1);

    match modes.mouse_encoding() {
        MouseEncoding::Sgr => {
            let last = if is_release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{last}").into_bytes())
        }
        MouseEncoding::Default | MouseEncoding::Utf8 => {
            // These encodings can't say which button was released.
            let code = if is_release { 3 + modifiers } else { code };
            let mut out = b"\x1b[M".to_vec();
            out.push(u8::try_from(32 + code).ok()?);
            if modes.mouse_encoding() == MouseEncoding::Utf8 {
                if input.column > UTF8_MAX || input.row > UTF8_MAX {
                    return None;
                }
                for value in [x, y] {
                    let ch = char::from_u32(u32::try_from(32 + value).ok()?)?;
                    let mut buffer = [0; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
                }
            } else {
                if input.column > X10_MAX || input.row > X10_MAX {
                    return None;
                }
                out.push(u8::try_from(32 + x).ok()?);
                out.push(u8::try_from(32 + y).ok()?);
            }
            Some(out)
        }
    }
}

/// On the alternate screen, programs like `less` and `man` expect the wheel to send arrow keys
/// when they haven't asked for mouse reports. Returns those keys for `lines` steps (positive is
/// up), or `None` when the wheel should scroll the scrollback or be reported instead.
pub fn encode_alternate_scroll(lines: i32, modes: TerminalModes) -> Option<Vec<u8>> {
    if !modes.is_alt_screen()
        || !modes.is_alternate_scroll()
        || modes.mouse_reporting() != MouseReporting::Off
        || lines == 0
    {
        return None;
    }
    let key = cursor_key(if lines > 0 { b'A' } else { b'B' }, Modifiers::NONE, modes);
    let steps = usize::try_from(lines.unsigned_abs()).ok()?;
    Some(key.repeat(steps))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::size::TerminalSize;
    use crate::terminal::{Terminal, TerminalConfig};

    /// The modes a terminal is in after the program sends `setup`.
    fn modes(setup: &[u8]) -> TerminalModes {
        let mut terminal = Terminal::new(&TerminalConfig::default(), TerminalSize::default());
        terminal.feed(setup);
        terminal.modes()
    }

    const SHIFT: Modifiers = Modifiers {
        shift: true,
        alt: false,
        control: false,
    };
    const ALT: Modifiers = Modifiers {
        shift: false,
        alt: true,
        control: false,
    };
    const CTRL: Modifiers = Modifiers {
        shift: false,
        alt: false,
        control: true,
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        shift: true,
        alt: false,
        control: true,
    };
    const ALL: Modifiers = Modifiers {
        shift: true,
        alt: true,
        control: true,
    };

    #[test]
    fn keys_encode_like_xterm() {
        let normal = TerminalModes::default();
        let cases: &[(&str, Key, Modifiers, &[u8])] = &[
            ("letter", Key::Char('a'), Modifiers::NONE, b"a"),
            ("unicode", Key::Char('é'), Modifiers::NONE, "é".as_bytes()),
            ("ctrl+c", Key::Char('c'), CTRL, b"\x03"),
            ("ctrl+shift+c", Key::Char('c'), CTRL_SHIFT, b"\x03"),
            ("ctrl+upper", Key::Char('Z'), CTRL, b"\x1a"),
            ("ctrl+space", Key::Char(' '), CTRL, b"\x00"),
            ("ctrl+[", Key::Char('['), CTRL, b"\x1b"),
            ("ctrl+\\", Key::Char('\\'), CTRL, b"\x1c"),
            ("ctrl+/", Key::Char('/'), CTRL, b"\x1f"),
            ("ctrl+?", Key::Char('?'), CTRL, b"\x7f"),
            ("ctrl+digit 1 unchanged", Key::Char('1'), CTRL, b"1"),
            ("alt+x", Key::Char('x'), ALT, b"\x1bx"),
            (
                "ctrl+alt+x",
                Key::Char('x'),
                Modifiers {
                    alt: true,
                    control: true,
                    shift: false,
                },
                b"\x1b\x18",
            ),
            ("enter", Key::Enter, Modifiers::NONE, b"\r"),
            ("alt+enter", Key::Enter, ALT, b"\x1b\r"),
            ("tab", Key::Tab, Modifiers::NONE, b"\t"),
            ("shift+tab", Key::Tab, SHIFT, b"\x1b[Z"),
            ("backspace", Key::Backspace, Modifiers::NONE, b"\x7f"),
            ("ctrl+backspace", Key::Backspace, CTRL, b"\x08"),
            ("alt+backspace", Key::Backspace, ALT, b"\x1b\x7f"),
            ("escape", Key::Escape, Modifiers::NONE, b"\x1b"),
            ("up", Key::Up, Modifiers::NONE, b"\x1b[A"),
            ("down", Key::Down, Modifiers::NONE, b"\x1b[B"),
            ("right", Key::Right, Modifiers::NONE, b"\x1b[C"),
            ("left", Key::Left, Modifiers::NONE, b"\x1b[D"),
            ("ctrl+left", Key::Left, CTRL, b"\x1b[1;5D"),
            ("shift+up", Key::Up, SHIFT, b"\x1b[1;2A"),
            ("all+right", Key::Right, ALL, b"\x1b[1;8C"),
            ("home", Key::Home, Modifiers::NONE, b"\x1b[H"),
            ("end", Key::End, Modifiers::NONE, b"\x1b[F"),
            ("shift+end", Key::End, SHIFT, b"\x1b[1;2F"),
            ("insert", Key::Insert, Modifiers::NONE, b"\x1b[2~"),
            ("delete", Key::Delete, Modifiers::NONE, b"\x1b[3~"),
            ("ctrl+delete", Key::Delete, CTRL, b"\x1b[3;5~"),
            ("page up", Key::PageUp, Modifiers::NONE, b"\x1b[5~"),
            ("shift+page down", Key::PageDown, SHIFT, b"\x1b[6;2~"),
            ("f1", Key::Function(1), Modifiers::NONE, b"\x1bOP"),
            ("f4", Key::Function(4), Modifiers::NONE, b"\x1bOS"),
            ("shift+f1", Key::Function(1), SHIFT, b"\x1b[1;2P"),
            ("f5", Key::Function(5), Modifiers::NONE, b"\x1b[15~"),
            ("f10", Key::Function(10), Modifiers::NONE, b"\x1b[21~"),
            ("f11", Key::Function(11), Modifiers::NONE, b"\x1b[23~"),
            ("ctrl+f12", Key::Function(12), CTRL, b"\x1b[24;5~"),
        ];
        for (name, key, modifiers, expected) in cases {
            assert_eq!(
                encode_key(*key, *modifiers, normal).as_deref(),
                Some(*expected),
                "{name}"
            );
        }
        assert_eq!(encode_key(Key::Function(13), Modifiers::NONE, normal), None);
        assert_eq!(encode_key(Key::Function(0), Modifiers::NONE, normal), None);
    }

    #[test]
    fn modes_change_cursor_keys_and_enter() {
        let app = modes(b"\x1b[?1h");
        assert_eq!(
            encode_key(Key::Up, Modifiers::NONE, app).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(Key::Home, Modifiers::NONE, app).unwrap(),
            b"\x1bOH"
        );
        // Modified keys look the same in both modes.
        assert_eq!(encode_key(Key::Up, CTRL, app).unwrap(), b"\x1b[1;5A");

        let lnm = modes(b"\x1b[20h");
        assert_eq!(
            encode_key(Key::Enter, Modifiers::NONE, lnm).unwrap(),
            b"\r\n"
        );
    }

    #[test]
    fn paste_normalizes_newlines_and_brackets() {
        let plain = TerminalModes::default();
        let bracketed = modes(b"\x1b[?2004h");
        let cases: &[(&str, &str, TerminalModes, &str)] = &[
            ("plain", "ls -la", plain, "ls -la"),
            ("lf to cr", "a\nb\n", plain, "a\rb\r"),
            ("crlf to cr", "a\r\nb", plain, "a\rb"),
            ("bracketed", "a\nb", bracketed, "\x1b[200~a\rb\x1b[201~"),
            (
                "end marker can't escape",
                "x\x1b[201~rm -rf ~\n",
                bracketed,
                "\x1b[200~xrm -rf ~\r\x1b[201~",
            ),
        ];
        for (name, text, modes, expected) in cases {
            assert_eq!(
                String::from_utf8(encode_paste(text, *modes)).unwrap(),
                *expected,
                "{name}"
            );
        }
    }

    #[test]
    fn multiline_detection_ignores_one_trailing_newline() {
        let cases = [
            ("", false),
            ("ls", false),
            ("ls\n", false),
            ("ls\r\n", false),
            ("a\nb", true),
            ("a\rb", true),
            ("ls\n\n", true),
        ];
        for (text, expected) in cases {
            assert_eq!(is_multiline(text), expected, "{text:?}");
        }
    }

    #[test]
    fn focus_is_reported_only_when_asked() {
        assert_eq!(encode_focus(true, TerminalModes::default()), None);
        let focus = modes(b"\x1b[?1004h");
        assert_eq!(encode_focus(true, focus), Some(&b"\x1b[I"[..]));
        assert_eq!(encode_focus(false, focus), Some(&b"\x1b[O"[..]));
    }

    fn mouse(action: MouseAction, row: usize, column: usize, modifiers: Modifiers) -> MouseInput {
        MouseInput {
            action,
            row,
            column,
            modifiers,
        }
    }

    #[test]
    fn mouse_reports_follow_mode_and_encoding() {
        use MouseAction::{Move, Press, Release};
        use MouseButton::{Left, Middle, Right, WheelDown, WheelUp};

        let none = TerminalModes::default();
        let click_x10 = modes(b"\x1b[?1000h");
        let drag_sgr = modes(b"\x1b[?1002h\x1b[?1006h");
        let motion_sgr = modes(b"\x1b[?1003h\x1b[?1006h");
        let click_utf8 = modes(b"\x1b[?1000h\x1b[?1005h");

        let cases: &[(&str, TerminalModes, MouseInput, Option<&[u8]>)] = &[
            ("off", none, mouse(Press(Left), 0, 0, Modifiers::NONE), None),
            (
                "x10 press",
                click_x10,
                mouse(Press(Left), 2, 4, Modifiers::NONE),
                Some(b"\x1b[M %#"),
            ),
            (
                "x10 release",
                click_x10,
                mouse(Release(Right), 0, 0, Modifiers::NONE),
                Some(b"\x1b[M#!!"),
            ),
            (
                "x10 ctrl",
                click_x10,
                mouse(Press(Middle), 0, 0, CTRL),
                Some(b"\x1b[M1!!"),
            ),
            (
                "x10 drag ignored",
                click_x10,
                mouse(Move(Some(Left)), 0, 0, Modifiers::NONE),
                None,
            ),
            (
                "x10 too far",
                click_x10,
                mouse(Press(Left), 0, 223, Modifiers::NONE),
                None,
            ),
            (
                "x10 last column",
                click_x10,
                mouse(Press(Left), 0, 222, Modifiers::NONE),
                Some(b"\x1b[M \xff!"),
            ),
            (
                "sgr press",
                drag_sgr,
                mouse(Press(Left), 2, 4, Modifiers::NONE),
                Some(b"\x1b[<0;5;3M"),
            ),
            (
                "sgr release keeps button",
                drag_sgr,
                mouse(Release(Right), 0, 0, Modifiers::NONE),
                Some(b"\x1b[<2;1;1m"),
            ),
            (
                "sgr drag",
                drag_sgr,
                mouse(Move(Some(Left)), 9, 9, Modifiers::NONE),
                Some(b"\x1b[<32;10;10M"),
            ),
            (
                "sgr hover needs motion mode",
                drag_sgr,
                mouse(Move(None), 0, 0, Modifiers::NONE),
                None,
            ),
            (
                "sgr hover",
                motion_sgr,
                mouse(Move(None), 0, 0, Modifiers::NONE),
                Some(b"\x1b[<35;1;1M"),
            ),
            (
                "sgr wheel",
                drag_sgr,
                mouse(Press(WheelUp), 0, 0, Modifiers::NONE),
                Some(b"\x1b[<64;1;1M"),
            ),
            (
                "sgr shift wheel",
                drag_sgr,
                mouse(Press(WheelDown), 0, 0, SHIFT),
                Some(b"\x1b[<69;1;1M"),
            ),
            (
                "wheel release",
                drag_sgr,
                mouse(Release(WheelUp), 0, 0, Modifiers::NONE),
                None,
            ),
            (
                "sgr far",
                drag_sgr,
                mouse(Press(Left), 999, 999, ALL),
                Some(b"\x1b[<28;1000;1000M"),
            ),
            (
                "utf8 far",
                click_utf8,
                mouse(Press(Left), 0, 300, Modifiers::NONE),
                Some("\x1b[M \u{14d}!".as_bytes()),
            ),
        ];
        for (name, modes, input, expected) in cases {
            assert_eq!(encode_mouse(*input, *modes).as_deref(), *expected, "{name}");
        }
    }

    #[test]
    fn alternate_scroll_sends_arrows_on_the_alt_screen() {
        let alt = modes(b"\x1b[?1049h");
        assert_eq!(encode_alternate_scroll(2, alt).unwrap(), b"\x1b[A\x1b[A");
        assert_eq!(encode_alternate_scroll(-1, alt).unwrap(), b"\x1b[B");
        assert_eq!(encode_alternate_scroll(0, alt), None);

        let alt_app = modes(b"\x1b[?1049h\x1b[?1h");
        assert_eq!(encode_alternate_scroll(1, alt_app).unwrap(), b"\x1bOA");

        let cases = [
            ("main screen", modes(b"")),
            ("alternate scroll off", modes(b"\x1b[?1049h\x1b[?1007l")),
            ("mouse reporting on", modes(b"\x1b[?1049h\x1b[?1000h")),
        ];
        for (name, modes) in cases {
            assert_eq!(encode_alternate_scroll(3, modes), None, "{name}");
        }
    }
}
