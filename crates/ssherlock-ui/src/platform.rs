//! Every difference between macOS, Linux and Windows that the UI cares about, in one place
//! (CLAUDE.md §7): which modifier app shortcuts use, what Alt/Option does in a terminal, which
//! monospace fonts to look for, whether there is a primary selection and whether middle-click
//! pastes it.

use gpui::{App, AppContext as _, ClipboardItem, Global};
use serde::Deserialize;

/// An operating system, for keymap sections that apply to only some (`"os": ["macos"]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    /// macOS.
    Macos,
    /// Linux and the BSDs, which share its desktops and conventions.
    Linux,
    /// Windows.
    Windows,
}

/// The operating system this build is for.
pub const OS: Os = if cfg!(target_os = "macos") {
    Os::Macos
} else if cfg!(target_os = "windows") {
    Os::Windows
} else {
    Os::Linux
};

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
/// button pastes it. X11 and Wayland have one; macOS and Windows don't. Matches the targets on
/// which gpui offers `write_to_primary` / `read_from_primary`.
pub const HAS_PRIMARY_SELECTION: bool = cfg!(any(target_os = "linux", target_os = "freebsd"));

/// Whether the desktop lets the middle button paste the primary selection. Set once at startup
/// by [`detect_primary_paste`]; until then, and wherever nothing says otherwise, it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimaryPaste(pub bool);

impl Global for PrimaryPaste {}

/// Whether a middle click should paste the primary selection here: the platform has one and
/// the user hasn't turned middle-click paste off.
pub fn is_primary_paste_enabled(cx: &App) -> bool {
    HAS_PRIMARY_SELECTION && cx.try_global::<PrimaryPaste>().is_none_or(|paste| paste.0)
}

/// Reads the desktop's middle-click paste setting in the background and records it as
/// [`PrimaryPaste`]. GNOME keeps it in `org.gnome.desktop.interface gtk-enable-primary-paste`
/// (Settings → Mouse & Touchpad, or Tweaks); other desktops don't have one, so it stays on, as
/// X11 has always done.
pub fn detect_primary_paste(cx: &mut App) {
    if !HAS_PRIMARY_SELECTION {
        return;
    }
    let lookup = cx.background_spawn(async {
        std::process::Command::new("gsettings")
            .args([
                "get",
                "org.gnome.desktop.interface",
                "gtk-enable-primary-paste",
            ])
            .output()
    });
    cx.spawn(async move |cx| {
        let enabled = match lookup.await {
            Ok(output) if output.status.success() => {
                parse_gsettings_bool(&String::from_utf8_lossy(&output.stdout))
            }
            // No gsettings (not GNOME) or no such key: nothing turned it off.
            Ok(_) | Err(_) => None,
        };
        if let Some(enabled) = enabled {
            tracing::debug!(enabled, "middle-click paste setting");
            let _ = cx.update(|cx| cx.set_global(PrimaryPaste(enabled)));
        }
    })
    .detach();
}

/// `true` or `false` as `gsettings get` prints a boolean.
fn parse_gsettings_bool(text: &str) -> Option<bool> {
    match text.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Puts `item` on the primary selection.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn write_to_primary(item: ClipboardItem, cx: &mut App) {
    cx.write_to_primary(item);
}

/// Puts `item` on the primary selection. There is none on this platform, so it does nothing.
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn write_to_primary(item: ClipboardItem, cx: &mut App) {
    let _ = (item, cx);
}

/// Reads the primary selection.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn read_from_primary(cx: &App) -> Option<ClipboardItem> {
    cx.read_from_primary()
}

/// Reads the primary selection. There is none on this platform, so it is always empty.
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn read_from_primary(cx: &App) -> Option<ClipboardItem> {
    let _ = cx;
    None
}

/// Whether `modifiers` is the one held to open links with a click: Cmd on macOS, Ctrl
/// elsewhere (as in Terminal.app and GNOME Terminal). A plain click selects text.
pub fn is_link_modifier(modifiers: &gpui::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.control
    }
}

/// The name of gpui's `secondary-` modifier (Cmd on macOS, Ctrl elsewhere), for hints.
pub const SECONDARY_NAME: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

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

// Keymap entries can also use gpui's own `secondary-` prefix, which means Cmd on macOS and Ctrl
// elsewhere. It suits keys typed with Shift on some layouts, such as `+`: gpui reports those as the
// symbol without Shift, so a `mod-` (Ctrl+Shift) binding for them would never match. Zoom uses it,
// as GNOME Terminal and browsers do.

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
    fn gsettings_booleans() {
        assert_eq!(parse_gsettings_bool("true\n"), Some(true));
        assert_eq!(parse_gsettings_bool("false"), Some(false));
        assert_eq!(parse_gsettings_bool(""), None);
        assert_eq!(parse_gsettings_bool("No such key"), None);
    }

    #[gpui::test]
    fn middle_click_paste_follows_the_setting(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(
                is_primary_paste_enabled(cx),
                HAS_PRIMARY_SELECTION,
                "on until told otherwise"
            );
            cx.set_global(PrimaryPaste(false));
            assert!(!is_primary_paste_enabled(cx));
        });
    }

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
