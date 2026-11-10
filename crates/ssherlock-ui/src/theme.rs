//! Design tokens: every colour, spacing, radius, font and size a view may use.
//!
//! Views read tokens through [`ActiveTheme::theme`] (`cx.theme()`) and never write literal
//! colours or pixel values (CLAUDE.md §7). If a view needs a value that isn't here, add a token.
//! This module is the only place in the UI where literal colours and sizes appear.

use std::sync::LazyLock;

use gpui::{App, Global, Hsla, Pixels, SharedString, WindowAppearance, px, rgb, rgba};

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
        }
    }
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
