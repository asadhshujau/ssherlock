//! Every difference between macOS, Linux and Windows that the UI cares about, in one place
//! (CLAUDE.md §7): which modifier app shortcuts use, what Alt/Option does in a terminal, and
//! which monospace fonts to look for.

/// Monospace font families to try for terminals, most preferred first.
#[cfg(target_os = "macos")]
pub const MONO_FAMILIES: &[&str] = &["SF Mono", "Menlo", "Monaco", "Courier New"];

/// Monospace font families to try for terminals, most preferred first.
#[cfg(target_os = "windows")]
pub const MONO_FAMILIES: &[&str] = &["Cascadia Mono", "Consolas", "Courier New"];

/// Monospace font families to try for terminals, most preferred first.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const MONO_FAMILIES: &[&str] = &[
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "Cascadia Mono",
    "JetBrains Mono",
];

/// What Alt (Option on macOS) does when held with a character key in a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AltKey {
    /// Sends ESC before the key, so readline's Alt+B / Alt+F work.
    Meta,
    /// Types whatever character the keyboard layout composes (Option+E then E gives é).
    Compose,
}

/// What Alt does in a terminal on this platform.
///
/// macOS keeps Option for composing characters, like Terminal.app and iTerm2 do out of the box.
/// Elsewhere Alt doesn't compose, so it is always Meta. A setting will make this choosable (APP-02).
#[cfg(target_os = "macos")]
pub const ALT_KEY: AltKey = AltKey::Compose;

/// What Alt does in a terminal on this platform.
#[cfg(not(target_os = "macos"))]
pub const ALT_KEY: AltKey = AltKey::Meta;

/// Whether the platform has a primary selection: selecting text copies it there and the middle
/// button pastes it. X11 and Wayland have one; macOS and Windows don't.
pub const HAS_PRIMARY_SELECTION: bool = cfg!(not(any(target_os = "macos", target_os = "windows")));

/// Whether `modifiers` is the one held to open links with a click: Cmd on macOS, Ctrl
/// elsewhere (as in Terminal.app and GNOME Terminal). A plain click selects text.
pub fn is_link_modifier(modifiers: &gpui::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.control
    }
}

/// The name of the link modifier, for hints.
pub const LINK_MODIFIER_NAME: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// The modifiers that `mod-` stands for in `keymap/default.json`.
///
/// On macOS app shortcuts use Cmd. Elsewhere Ctrl alone belongs to the shell (Ctrl+C, Ctrl+W,
/// …), so app shortcuts use Ctrl+Shift, as GNOME Terminal, Konsole and Windows Terminal do.
#[cfg(target_os = "macos")]
const MOD: &str = "cmd-";

/// The modifiers that `mod-` stands for in `keymap/default.json`.
#[cfg(not(target_os = "macos"))]
const MOD: &str = "ctrl-shift-";

/// Rewrites a keymap keystroke sequence for this platform, expanding the `mod-` prefix of each
/// keystroke into [`MOD`].
pub fn expand_keystrokes(spec: &str) -> String {
    expand_with(spec, MOD)
}

fn expand_with(spec: &str, prefix: &str) -> String {
    spec.split_whitespace()
        .map(|keystroke| match keystroke.strip_prefix("mod-") {
            Some(rest) => format!("{prefix}{rest}"),
            None => keystroke.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn mod_expands_per_platform() {
        let cases = [
            ("mod-v", "cmd-", "cmd-v"),
            ("mod-v", "ctrl-shift-", "ctrl-shift-v"),
            ("shift-insert", "cmd-", "shift-insert"),
            ("mod-k mod-s", "cmd-", "cmd-k cmd-s"),
            ("  mod-t  ", "cmd-", "cmd-t"),
            ("alt-mod-x", "cmd-", "alt-mod-x"),
        ];
        for (spec, prefix, expected) in cases {
            assert_eq!(expand_with(spec, prefix), expected, "{spec}");
        }
        assert!(expand_keystrokes("mod-c").ends_with("-c"));
    }

    #[test]
    fn link_modifier_is_cmd_on_macos_and_ctrl_elsewhere() {
        let ctrl = gpui::Modifiers {
            control: true,
            ..gpui::Modifiers::default()
        };
        let cmd = gpui::Modifiers {
            platform: true,
            ..gpui::Modifiers::default()
        };
        let expected = if cfg!(target_os = "macos") {
            (false, true)
        } else {
            (true, false)
        };
        assert_eq!((is_link_modifier(&ctrl), is_link_modifier(&cmd)), expected);
        assert!(!is_link_modifier(&gpui::Modifiers::default()));
    }

    #[test]
    fn there_are_mono_fonts_to_try() {
        assert_ne!(MONO_FAMILIES.len(), 0);
    }
}
