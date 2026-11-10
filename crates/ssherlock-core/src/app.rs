//! The application's name and platform identifier.
//!
//! Defined once so the data directory, the Wayland `app_id`, the macOS bundle id and the Linux
//! `.desktop` file can't drift apart. Changing [`ID`] moves every user's data directory, so treat
//! it as permanent.

/// Human-readable product name, shown in window titles.
pub const NAME: &str = "SSHerlock";

/// Reverse-DNS prefix of [`ID`].
pub const QUALIFIER: &str = "io.github";

/// Organisation part of [`ID`].
pub const ORGANISATION: &str = "asadhshujau";

/// Reverse-DNS application id: `QUALIFIER.ORGANISATION.NAME`.
pub const ID: &str = "io.github.asadhshujau.SSHerlock";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_built_from_its_parts() {
        assert_eq!(ID, format!("{QUALIFIER}.{ORGANISATION}.{NAME}"));
    }
}
