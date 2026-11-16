//! The colour a host or group is marked with, shown on its list row and its tabs.

use serde::{Deserialize, Serialize};

/// A colour from a fixed set. Stored by name; the theme decides the actual shade, so marks stay
/// readable in light and dark mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Colour {
    /// Red, the usual mark for production.
    Red,
    /// Orange.
    Orange,
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Teal.
    Teal,
    /// Blue.
    Blue,
    /// Purple.
    Purple,
    /// Pink.
    Pink,
    /// Grey.
    Grey,
}

impl Colour {
    /// Every colour, in the order a picker shows them.
    pub const ALL: [Self; 9] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
        Self::Pink,
        Self::Grey,
    ];

    /// The colour's name, for menus and screen readers.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Teal => "Teal",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Pink => "Pink",
            Self::Grey => "Grey",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::HashSet;

    use super::*;

    #[test]
    fn all_lists_each_colour_once_with_a_distinct_name() {
        let names: HashSet<_> = Colour::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), Colour::ALL.len());
        assert_eq!(serde_json::to_string(&Colour::Teal).unwrap(), r#""teal""#);
    }
}
