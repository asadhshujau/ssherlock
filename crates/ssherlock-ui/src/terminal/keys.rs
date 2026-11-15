//! Turns a gpui keystroke into terminal input: text to type, or a key to encode.

use gpui::Keystroke;
use ssherlock_terminal::{Key, Modifiers};

use crate::platform::AltKey;

/// What a keystroke means to a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyInput {
    /// Text the keyboard layout produced, sent as UTF-8.
    Text(String),
    /// A key that needs encoding, with its modifiers.
    Key(Key, Modifiers),
}

/// The terminal input for `keystroke`, or `None` if it isn't for the terminal (shortcuts using
/// Cmd/Super, bare modifier presses).
pub fn key_input(keystroke: &Keystroke, alt: AltKey) -> Option<KeyInput> {
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    let modifiers = Modifiers {
        shift: m.shift,
        alt: m.alt && alt == AltKey::Meta,
        control: m.control,
    };

    if let Some(key) = named_key(&keystroke.key) {
        // Option only composes characters, so with named keys it is always Alt.
        let modifiers = Modifiers {
            alt: m.alt,
            ..modifiers
        };
        return Some(KeyInput::Key(key, modifiers));
    }

    // Ctrl and Meta combinations are built from the key itself: the layout's composed text
    // (`key_char`) is either missing or, for macOS Option, a different character entirely.
    if modifiers.control || modifiers.alt {
        let mut chars = keystroke.key.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return None;
        };
        let ch = if m.shift && !m.control {
            ch.to_uppercase().next().unwrap_or(ch)
        } else {
            ch
        };
        return Some(KeyInput::Key(Key::Char(ch), modifiers));
    }

    let text = keystroke.key_char.clone().or_else(|| {
        // Keys that type a single character but come without `key_char`.
        let mut chars = keystroke.key.chars();
        match (chars.next(), chars.next()) {
            (Some(_), None) => Some(keystroke.key.clone()),
            _ => None,
        }
    })?;
    (!text.is_empty()).then_some(KeyInput::Text(text))
}

/// Keys gpui names rather than spells out.
fn named_key(name: &str) -> Option<Key> {
    let key = match name {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "escape" => Key::Escape,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "space" => Key::Char(' '),
        _ => {
            let number = name.strip_prefix('f')?.parse::<u8>().ok()?;
            return (1..=12).contains(&number).then_some(Key::Function(number));
        }
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn keystroke(source: &str, key_char: Option<&str>) -> Keystroke {
        let mut keystroke = Keystroke::parse(source).unwrap();
        keystroke.key_char = key_char.map(str::to_owned);
        keystroke
    }

    fn key(key: Key, shift: bool, alt: bool, control: bool) -> KeyInput {
        KeyInput::Key(
            key,
            Modifiers {
                shift,
                alt,
                control,
            },
        )
    }

    fn text(text: &str) -> KeyInput {
        KeyInput::Text(text.to_owned())
    }

    #[test]
    fn keystrokes_map_to_terminal_input() {
        use AltKey::{Compose, Meta};
        type Case<'a> = (&'a str, &'a str, Option<&'a str>, AltKey, Option<KeyInput>);
        let cases: &[Case] = &[
            ("letter", "a", Some("a"), Meta, Some(text("a"))),
            ("shifted", "shift-a", Some("A"), Meta, Some(text("A"))),
            ("layout symbol", "shift-1", Some("!"), Meta, Some(text("!"))),
            ("composed", "e", Some("\u{e9}"), Meta, Some(text("\u{e9}"))),
            (
                "space",
                "space",
                Some(" "),
                Meta,
                Some(key(Key::Char(' '), false, false, false)),
            ),
            (
                "ctrl+c",
                "ctrl-c",
                None,
                Meta,
                Some(key(Key::Char('c'), false, false, true)),
            ),
            (
                "ctrl+shift+c",
                "ctrl-shift-c",
                None,
                Meta,
                Some(key(Key::Char('c'), true, false, true)),
            ),
            (
                "alt as meta",
                "alt-b",
                Some("b"),
                Meta,
                Some(key(Key::Char('b'), false, true, false)),
            ),
            (
                "alt+shift as meta",
                "alt-shift-b",
                Some("B"),
                Meta,
                Some(key(Key::Char('B'), true, true, false)),
            ),
            (
                "option composes",
                "alt-e",
                Some("\u{b4}"),
                Compose,
                Some(text("\u{b4}")),
            ),
            (
                "enter",
                "enter",
                None,
                Meta,
                Some(key(Key::Enter, false, false, false)),
            ),
            (
                "shift+tab",
                "shift-tab",
                None,
                Meta,
                Some(key(Key::Tab, true, false, false)),
            ),
            (
                "alt arrow, compose mode",
                "alt-left",
                None,
                Compose,
                Some(key(Key::Left, false, true, false)),
            ),
            (
                "ctrl arrow",
                "ctrl-left",
                None,
                Meta,
                Some(key(Key::Left, false, false, true)),
            ),
            (
                "page down",
                "pagedown",
                None,
                Meta,
                Some(key(Key::PageDown, false, false, false)),
            ),
            (
                "f5",
                "f5",
                None,
                Meta,
                Some(key(Key::Function(5), false, false, false)),
            ),
            ("f13 unsupported", "f13", None, Meta, None),
            ("cmd shortcut", "cmd-c", None, Meta, None),
            ("super shortcut", "super-t", None, Meta, None),
        ];
        for (name, source, key_char, alt, expected) in cases {
            assert_eq!(
                key_input(&keystroke(source, *key_char), *alt),
                *expected,
                "{name}"
            );
        }
    }
}
