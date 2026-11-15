//! Terminal colours: the 16 ANSI colours plus the default foreground, background and cursor.
//!
//! The UI builds a [`Palette`] from its theme tokens and hands it to the terminal, which resolves
//! every cell to concrete [`Rgb`] values. Knowing the palette here, rather than only in the UI,
//! lets the terminal answer programs that ask for colours (`OSC 10/11`, used by vim and others
//! to pick a light or dark scheme).

/// An opaque 8-bit-per-channel colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

/// How much darker `SGR 2` (dim) text is than normal text: the factor xterm and Alacritty use.
const DIM_FACTOR: f32 = 0.66;

/// Channel levels of the xterm 6 × 6 × 6 colour cube (indices 16–231).
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

impl Rgb {
    /// A colour from its channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// A colour from `0xRRGGBB`; higher bits are ignored.
    pub const fn from_hex(hex: u32) -> Self {
        let [_, r, g, b] = hex.to_be_bytes();
        Self { r, g, b }
    }

    /// This colour as drawn for dim text.
    #[must_use]
    pub fn dim(self) -> Self {
        Self::new(dim(self.r), dim(self.g), dim(self.b))
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a u8 times a factor below 1 stays within 0..=255"
)]
fn dim(channel: u8) -> u8 {
    (f32::from(channel) * DIM_FACTOR).round() as u8
}

/// The colours a terminal draws with.
///
/// `ansi` follows xterm's order: black, red, green, yellow, blue, magenta, cyan, white, then the
/// bright versions of each. The remaining 240 of the 256 indexed colours are fixed by the xterm
/// standard and computed by [`Palette::indexed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The 16 ANSI colours.
    pub ansi: [Rgb; 16],
    /// Default text colour.
    pub foreground: Rgb,
    /// Default background colour.
    pub background: Rgb,
    /// Cursor colour.
    pub cursor: Rgb,
}

impl Palette {
    /// The colour of xterm 256-colour index `index`.
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self
                .ansi
                .get(usize::from(index))
                .copied()
                .unwrap_or(self.foreground),
            16..=231 => {
                let i = index - 16;
                let level = |n: u8| CUBE_LEVELS.get(usize::from(n % 6)).copied().unwrap_or(0);
                Rgb::new(level(i / 36), level(i / 6), level(i))
            }
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                Rgb::new(gray, gray, gray)
            }
        }
    }
}

impl Default for Palette {
    /// xterm's default colours, so a terminal is usable before the UI supplies its theme.
    fn default() -> Self {
        Self {
            ansi: [
                Rgb::from_hex(0x00_0000),
                Rgb::from_hex(0xcd_0000),
                Rgb::from_hex(0x00_cd00),
                Rgb::from_hex(0xcd_cd00),
                Rgb::from_hex(0x00_00ee),
                Rgb::from_hex(0xcd_00cd),
                Rgb::from_hex(0x00_cdcd),
                Rgb::from_hex(0xe5_e5e5),
                Rgb::from_hex(0x7f_7f7f),
                Rgb::from_hex(0xff_0000),
                Rgb::from_hex(0x00_ff00),
                Rgb::from_hex(0xff_ff00),
                Rgb::from_hex(0x5c_5cff),
                Rgb::from_hex(0xff_00ff),
                Rgb::from_hex(0x00_ffff),
                Rgb::from_hex(0xff_ffff),
            ],
            foreground: Rgb::from_hex(0xe5_e5e5),
            background: Rgb::from_hex(0x00_0000),
            cursor: Rgb::from_hex(0xe5_e5e5),
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn indexed_covers_ansi_cube_and_grayscale() {
        let palette = Palette::default();
        let cases = [
            ("ansi black", 0, palette.ansi[0]),
            ("ansi bright white", 15, palette.ansi[15]),
            ("cube origin", 16, Rgb::new(0, 0, 0)),
            ("cube pure blue", 21, Rgb::new(0, 0, 255)),
            ("cube pure red", 196, Rgb::new(255, 0, 0)),
            ("cube mixed", 16 + 36 + 2 * 6 + 3, Rgb::new(95, 135, 175)),
            ("cube end", 231, Rgb::new(255, 255, 255)),
            ("gray start", 232, Rgb::new(8, 8, 8)),
            ("gray end", 255, Rgb::new(238, 238, 238)),
        ];
        for (name, index, expected) in cases {
            assert_eq!(palette.indexed(index), expected, "{name}");
        }
    }

    #[test]
    fn from_hex_and_dim() {
        assert_eq!(Rgb::from_hex(0x12_3456), Rgb::new(0x12, 0x34, 0x56));
        assert_eq!(Rgb::from_hex(0xff12_3456), Rgb::new(0x12, 0x34, 0x56));
        assert_eq!(Rgb::new(100, 0, 255).dim(), Rgb::new(66, 0, 168));
    }
}
