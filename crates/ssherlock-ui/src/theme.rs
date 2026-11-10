//! Design tokens: every colour, spacing, radius, font and size a view may use.
//!
//! Views read tokens through [`ActiveTheme::theme`] (`cx.theme()`) and never write literal
//! colours or pixel values (CLAUDE.md §7). If a view needs a value that isn't here, add a token.
//! This module is the only place in the UI where literal colours and sizes appear.

use std::sync::LazyLock;

use gpui::{App, Global, Hsla, Pixels, Rgba, SharedString, WindowAppearance, px, rgb, rgba};
use ssherlock_terminal::{Palette, Rgb};

use crate::platform;

/// Whether a theme is meant for a light or a dark system appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    /// Dark text on a light background.
    Light,
    /// Light text on a dark background.
    Dark,
}

impl Appearance {
    /// Maps the platform's appearance (including macOS "vibrant" variants) to ours.
    pub fn from_window(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
        }
    }
}

/// The complete set of design tokens for one appearance.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Which system appearance this theme suits.
    pub appearance: Appearance,
    /// Colour tokens.
    pub colors: Colors,
    /// Colours inside terminals.
    pub terminal: TerminalColors,
    /// Gaps and padding.
    pub spacing: Spacing,
    /// Corner radii.
    pub radius: Radius,
    /// Font families and sizes.
    pub text: Typography,
    /// Fixed sizes of window chrome and controls.
    pub metrics: Metrics,
}

/// Colour tokens, named by role rather than by hue.
#[derive(Clone, Debug, PartialEq)]
pub struct Colors {
    /// Window background behind all content.
    pub background: Hsla,
    /// Raised areas such as panels and cards.
    pub surface: Hsla,
    /// Dividers and outlines.
    pub border: Hsla,
    /// Primary text.
    pub text: Hsla,
    /// Secondary text: hints, labels, inactive titles.
    pub text_muted: Hsla,
    /// Brand and focus colour.
    pub accent: Hsla,
    /// Title bar of the focused window.
    pub title_bar: Hsla,
    /// Title bar of an unfocused window.
    pub title_bar_inactive: Hsla,
    /// Background of a hovered window control or icon button.
    pub control_hover: Hsla,
    /// Background of the hovered close button.
    pub control_close_hover: Hsla,
    /// Text on top of `control_close_hover`.
    pub control_close_text: Hsla,
    /// Drop shadow around a client-decorated window.
    pub window_shadow: Hsla,
    /// Background of a tab bar and its inactive tabs.
    pub tab_bar: Hsla,
    /// Background of the active tab; matches the content below it.
    pub tab_active: Hsla,
    /// Background of a hovered inactive tab.
    pub tab_hover: Hsla,
    /// The thumb of an overlay scrollbar.
    pub scrollbar: Hsla,
}

/// Colours inside a terminal: the 16 ANSI colours programs pick from, plus defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalColors {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then their bright versions.
    pub ansi: [Hsla; 16],
    /// Default text.
    pub foreground: Hsla,
    /// Default background.
    pub background: Hsla,
    /// The cursor.
    pub cursor: Hsla,
    /// Behind selected text.
    pub selection: Hsla,
    /// Behind search matches.
    pub search_match: Hsla,
    /// Behind the search match the user is on.
    pub search_match_focused: Hsla,
}

/// Spacing scale for gaps and padding.
#[derive(Clone, Debug, PartialEq)]
pub struct Spacing {
    /// Extra small.
    pub xs: Pixels,
    /// Small.
    pub sm: Pixels,
    /// Medium.
    pub md: Pixels,
    /// Large.
    pub lg: Pixels,
    /// Extra large.
    pub xl: Pixels,
}

/// Corner radii.
#[derive(Clone, Debug, PartialEq)]
pub struct Radius {
    /// Small controls.
    pub sm: Pixels,
    /// Panels and popovers.
    pub md: Pixels,
    /// Corners of a client-decorated window.
    pub window: Pixels,
}

/// Font families and sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct Typography {
    /// Font for interface text. `.SystemUIFont` resolves to the platform's UI font.
    pub ui_family: SharedString,
    /// Default interface text size.
    pub ui_size: Pixels,
    /// Small text: window titles, captions.
    pub small_size: Pixels,
    /// Large headline text.
    pub display_size: Pixels,
    /// Monospace families for terminals, most preferred first; the first one installed is used.
    pub mono_families: &'static [&'static str],
    /// Terminal text size.
    pub terminal_size: Pixels,
    /// Terminal line height as a multiple of `terminal_size`.
    pub terminal_line_height: f32,
}

/// Fixed sizes of window chrome and controls.
#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    /// Height of the client-drawn title bar.
    pub title_bar_height: Pixels,
    /// Width and height of a window control button (close, minimise, maximise).
    pub window_control_size: Pixels,
    /// Width and height of the glyph drawn inside a window control.
    pub window_control_glyph: Pixels,
    /// Invisible margin around a client-decorated window, used for the shadow and as the
    /// resize grip.
    pub window_inset: Pixels,
    /// Width of hairline borders.
    pub border_width: Pixels,
    /// Thickness of the beam and underline terminal cursors.
    pub cursor_thickness: Pixels,
    /// Height of a tab bar.
    pub tab_bar_height: Pixels,
    /// Narrowest a tab gets.
    pub tab_min_width: Pixels,
    /// Widest a tab gets; longer titles are cut with an ellipsis.
    pub tab_max_width: Pixels,
    /// Thickness of the active-tab and drop-target markers.
    pub tab_indicator: Pixels,
    /// Width of an overlay scrollbar.
    pub scrollbar_width: Pixels,
    /// Width of the text field in a search bar.
    pub search_field_width: Pixels,
}

impl Theme {
    /// The built-in theme for `appearance`.
    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => Self::light(),
            Appearance::Dark => Self::dark(),
        }
    }

    /// The built-in dark theme.
    pub fn dark() -> Self {
        Self {
            appearance: Appearance::Dark,
            colors: Colors {
                background: rgb(0x001b_1d22).into(),
                surface: rgb(0x0023_262d).into(),
                border: rgb(0x0033_363f).into(),
                text: rgb(0x00e6_e8ec).into(),
                text_muted: rgb(0x009a_a0ab).into(),
                accent: rgb(0x00e0_a84a).into(),
                title_bar: rgb(0x0016_181c).into(),
                title_bar_inactive: rgb(0x001b_1d22).into(),
                control_hover: rgba(0xffff_ff1a).into(),
                control_close_hover: rgb(0x00c4_2b1c).into(),
                control_close_text: rgb(0x00ff_ffff).into(),
                window_shadow: rgba(0x0000_0066).into(),
                tab_bar: rgb(0x0016_181c).into(),
                tab_active: rgb(0x001b_1d22).into(),
                tab_hover: rgb(0x0020_2329).into(),
                scrollbar: rgba(0xffff_ff40).into(),
            },
            terminal: TerminalColors {
                ansi: [
                    rgb(0x0030_333b).into(),
                    rgb(0x00e8_6b6b).into(),
                    rgb(0x009c_c97e).into(),
                    rgb(0x00e0_c078).into(),
                    rgb(0x0066_a8ea).into(),
                    rgb(0x00c5_86d9).into(),
                    rgb(0x005c_bec7).into(),
                    rgb(0x00b4_b9c2).into(),
                    rgb(0x0062_6874).into(),
                    rgb(0x00f2_8585).into(),
                    rgb(0x00b1_dc94).into(),
                    rgb(0x00ef_d392).into(),
                    rgb(0x0082_bcf5).into(),
                    rgb(0x00d7_a0e8).into(),
                    rgb(0x0078_d2da).into(),
                    rgb(0x00ee_f0f3).into(),
                ],
                foreground: rgb(0x00e6_e8ec).into(),
                background: rgb(0x001b_1d22).into(),
                cursor: rgb(0x00e0_a84a).into(),
                selection: rgba(0xe0a8_4a4d).into(),
                search_match: rgba(0x5cbe_c740).into(),
                search_match_focused: rgba(0xe0c0_7880).into(),
            },
            spacing: Spacing::default(),
            radius: Radius::default(),
            text: Typography::default(),
            metrics: Metrics::default(),
        }
    }

    /// The built-in light theme.
    pub fn light() -> Self {
        Self {
            appearance: Appearance::Light,
            colors: Colors {
                background: rgb(0x00f7_f7f8).into(),
                surface: rgb(0x00ff_ffff).into(),
                border: rgb(0x00d5_d7dd).into(),
                text: rgb(0x001d_2026).into(),
                text_muted: rgb(0x005c_6370).into(),
                accent: rgb(0x00a8_6a00).into(),
                title_bar: rgb(0x00eb_ecef).into(),
                title_bar_inactive: rgb(0x00f2_f3f5).into(),
                control_hover: rgba(0x0000_000f).into(),
                control_close_hover: rgb(0x00c4_2b1c).into(),
                control_close_text: rgb(0x00ff_ffff).into(),
                window_shadow: rgba(0x0000_0040).into(),
                tab_bar: rgb(0x00eb_ecef).into(),
                tab_active: rgb(0x00f7_f7f8).into(),
                tab_hover: rgb(0x00e2_e4e8).into(),
                scrollbar: rgba(0x0000_0040).into(),
            },
            terminal: TerminalColors {
                ansi: [
                    rgb(0x001d_2026).into(),
                    rgb(0x00c2_3a32).into(),
                    rgb(0x0037_7d2c).into(),
                    rgb(0x0086_5d00).into(),
                    rgb(0x002c_5fbf).into(),
                    rgb(0x0093_3caf).into(),
                    rgb(0x0017_7889).into(),
                    rgb(0x00c8_cbd2).into(),
                    rgb(0x005c_6370).into(),
                    rgb(0x00d8_4a40).into(),
                    rgb(0x0048_9339).into(),
                    rgb(0x00a0_7200).into(),
                    rgb(0x003f_74d6).into(),
                    rgb(0x00a9_52c6).into(),
                    rgb(0x0023_8ea1).into(),
                    rgb(0x00ff_ffff).into(),
                ],
                foreground: rgb(0x001d_2026).into(),
                background: rgb(0x00f7_f7f8).into(),
                cursor: rgb(0x00a8_6a00).into(),
                selection: rgba(0xa86a_0038).into(),
                search_match: rgba(0x1778_8933).into(),
                search_match_focused: rgba(0xc290_0066).into(),
            },
            spacing: Spacing::default(),
            radius: Radius::default(),
            text: Typography::default(),
            metrics: Metrics::default(),
        }
    }
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            xs: px(2.),
            sm: px(4.),
            md: px(8.),
            lg: px(12.),
            xl: px(16.),
        }
    }
}

impl Default for Radius {
    fn default() -> Self {
        Self {
            sm: px(4.),
            md: px(6.),
            window: px(10.),
        }
    }
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            ui_family: SharedString::new_static(".SystemUIFont"),
            ui_size: px(14.),
            small_size: px(12.),
            display_size: px(28.),
            mono_families: platform::MONO_FAMILIES,
            terminal_size: px(14.),
            terminal_line_height: 1.25,
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            title_bar_height: px(34.),
            window_control_size: px(24.),
            window_control_glyph: px(10.),
            window_inset: px(10.),
            border_width: px(1.),
            cursor_thickness: px(2.),
            tab_bar_height: px(34.),
            tab_min_width: px(96.),
            tab_max_width: px(220.),
            tab_indicator: px(2.),
            scrollbar_width: px(6.),
            search_field_width: px(220.),
        }
    }
}

impl Theme {
    /// The terminal colours in the form the terminal emulator takes.
    pub fn terminal_palette(&self) -> Palette {
        let t = &self.terminal;
        Palette {
            ansi: t.ansi.map(to_rgb),
            foreground: to_rgb(t.foreground),
            background: to_rgb(t.background),
            cursor: to_rgb(t.cursor),
        }
    }
}

/// An opaque 8-bit colour from a token; alpha is dropped because terminal colours are opaque.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "each channel is clamped to 0..=1 and scaled to 0..=255 before the cast"
)]
fn to_rgb(color: Hsla) -> Rgb {
    let Rgba { r, g, b, .. } = color.to_rgb();
    let channel = |c: f32| (c.clamp(0., 1.) * 255.).round() as u8;
    Rgb::new(channel(r), channel(g), channel(b))
}

/// A colour from the terminal emulator as a gpui colour.
pub fn from_rgb(color: Rgb) -> Hsla {
    Rgba {
        r: f32::from(color.r) / 255.,
        g: f32::from(color.g) / 255.,
        b: f32::from(color.b) / 255.,
        a: 1.,
    }
    .into()
}

impl Global for Theme {}

/// Gives every gpui context a `theme()` accessor.
pub trait ActiveTheme {
    /// The theme currently in use.
    fn theme(&self) -> &Theme;
}

/// Used only if a view renders before [`crate::init`] installed a theme, e.g. in a test.
static FALLBACK: LazyLock<Theme> = LazyLock::new(Theme::dark);

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.try_global::<Theme>().unwrap_or(&FALLBACK)
    }
}

#[cfg(test)]
mod tests {
    use gpui::Rgba;
    use pretty_assertions::assert_eq;

    use super::*;

    /// WCAG 2.x relative luminance of an sRGB colour.
    fn luminance(color: Hsla) -> f32 {
        let Rgba { r, g, b, .. } = color.into();
        let channel = |c: f32| {
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG contrast ratio, from 1 (none) to 21 (black on white).
    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa_contrast_in_both_themes() {
        for theme in [Theme::dark(), Theme::light()] {
            let c = &theme.colors;
            let pairs = [
                ("text on background", c.text, c.background, 4.5),
                ("text on surface", c.text, c.surface, 4.5),
                ("text on title bar", c.text, c.title_bar, 4.5),
                ("active tab text", c.text, c.tab_active, 4.5),
                ("inactive tab text", c.text_muted, c.tab_bar, 4.5),
                ("hovered tab text", c.text_muted, c.tab_hover, 4.5),
                ("muted text on background", c.text_muted, c.background, 4.5),
                ("muted text on title bar", c.text_muted, c.title_bar, 4.5),
                (
                    "muted on inactive bar",
                    c.text_muted,
                    c.title_bar_inactive,
                    4.5,
                ),
                (
                    "close glyph on hover",
                    c.control_close_text,
                    c.control_close_hover,
                    4.5,
                ),
            ];
            for (name, fg, bg, minimum) in pairs {
                let ratio = contrast(fg, bg);
                assert!(
                    ratio >= minimum,
                    "{:?} theme: {name} contrast {ratio:.2} < {minimum}",
                    theme.appearance
                );
            }
        }
    }

    #[test]
    fn terminal_colours_are_readable_in_both_themes() {
        for theme in [Theme::dark(), Theme::light()] {
            let t = &theme.terminal;
            let ratio = contrast(t.foreground, t.background);
            assert!(
                ratio >= 7.0,
                "{:?}: foreground {ratio:.2}",
                theme.appearance
            );
            // Red, green, yellow, blue, magenta, cyan and their bright versions are used for
            // text on the default background.
            for (index, color) in t.ansi.iter().enumerate() {
                if matches!(index, 0 | 7 | 8 | 15) {
                    continue;
                }
                let ratio = contrast(*color, t.background);
                assert!(
                    ratio >= 3.0,
                    "{:?}: ansi {index} contrast {ratio:.2} < 3",
                    theme.appearance
                );
            }
            assert!(
                contrast(t.cursor, t.background) >= 3.0,
                "{:?}: cursor",
                theme.appearance
            );
        }
    }

    #[test]
    fn terminal_palette_round_trips_through_rgb() {
        let theme = Theme::dark();
        let palette = theme.terminal_palette();
        assert_eq!(palette.background, Rgb::from_hex(0x1b_1d22));
        assert_eq!(palette.ansi[1], Rgb::from_hex(0xe8_6b6b));
        assert_eq!(palette.cursor, Rgb::from_hex(0xe0_a84a));
        for rgb in [
            Rgb::new(0, 0, 0),
            Rgb::new(255, 255, 255),
            Rgb::new(18, 52, 86),
        ] {
            assert_eq!(to_rgb(from_rgb(rgb)), rgb);
        }
    }

    #[test]
    fn appearance_follows_the_window() {
        let cases = [
            (WindowAppearance::Light, Appearance::Light),
            (WindowAppearance::VibrantLight, Appearance::Light),
            (WindowAppearance::Dark, Appearance::Dark),
            (WindowAppearance::VibrantDark, Appearance::Dark),
        ];
        for (window, expected) in cases {
            assert_eq!(Appearance::from_window(window), expected);
            assert_eq!(Theme::for_appearance(expected).appearance, expected);
        }
    }
}
