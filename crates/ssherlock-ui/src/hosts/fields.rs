//! Reading the host editor's text fields into model values, and writing values back as text.
//!
//! Pure functions, so every rule about what a field accepts is tested without gpui. Empty text
//! always means "not set" (inherit, or keep in a bulk edit).

use ssherlock_core::Port;

/// A port field: empty is unset.
///
/// # Errors
///
/// A message for the form if it isn't a port number.
pub fn port(text: &str) -> Result<Option<Port>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| "Use a number from 1 to 65535".to_owned())
}

/// A user name field: empty is unset. Checking what's in it is the model's job.
pub fn username(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// An algorithm list: names separated by commas and/or spaces, most preferred first.
pub fn algorithms(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// An algorithm list as the field shows it.
pub fn format_algorithms(names: &[String]) -> String {
    names.join(", ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn ports() {
        assert_eq!(port(""), Ok(None));
        assert_eq!(port("  "), Ok(None));
        assert_eq!(port(" 2222 "), Ok(Port::new(2222)));
        for bad in ["0", "65536", "ssh", "22a"] {
            assert!(port(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn usernames() {
        assert_eq!(username(""), None);
        assert_eq!(username("  alice "), Some("alice".into()));
        assert_eq!(
            username("al ice"),
            Some("al ice".into()),
            "the model rejects this one"
        );
    }

    #[test]
    fn algorithm_lists_round_trip() {
        let names =
            algorithms("aes256-gcm@openssh.com,  chacha20-poly1305@openssh.com aes128-ctr,");
        assert_eq!(
            names,
            [
                "aes256-gcm@openssh.com",
                "chacha20-poly1305@openssh.com",
                "aes128-ctr"
            ]
        );
        assert_eq!(algorithms(&format_algorithms(&names)), names);
        assert_eq!(algorithms(" , "), Vec::<String>::new());
    }
}
