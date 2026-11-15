//! Parsing what the user types in the quick connect bar: `host`, `user@host`, `host:port`,
//! `user@[2001:db8::1]:2222` or an `ssh://user@host:port` URL.
//!
//! **Model** layer: pure, total, table-tested. Matching saved hosts (the other half of CONN-23)
//! arrives with saved hosts in Phase 3.

use std::fmt;

use crate::address::{Address, AddressError};

/// Where the quick connect bar says to connect. Missing parts fall back to defaults chosen by the
/// caller (port 22, the local user name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectTarget {
    /// The user to log in as, if given.
    pub user: Option<String>,
    /// The server.
    pub address: Address,
    /// The port, if given.
    pub port: Option<u16>,
}

/// Why the quick connect text can't be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QuickConnectError {
    /// Nothing was typed.
    #[error("type a host to connect to, like user@example.com:22")]
    Empty,
    /// A URL with a scheme other than `ssh`.
    #[error("only ssh:// addresses are supported, not {0}://")]
    UnsupportedScheme(String),
    /// `@` with nothing before it.
    #[error("the user name before @ is empty")]
    EmptyUser,
    /// The port isn't a number from 1 to 65535.
    #[error("the port {0:?} is not a number from 1 to 65535")]
    InvalidPort(String),
    /// A `[` without its `]`, or text after the `]` that isn't `:port`.
    #[error("the bracketed IPv6 address is not closed properly")]
    UnclosedBracket,
    /// An `ssh://` URL with a path; SSH URLs name a host, not a file.
    #[error("an ssh:// address cannot have a path")]
    UrlPath,
    /// The host part is invalid.
    #[error(transparent)]
    Address(#[from] AddressError),
}

impl ConnectTarget {
    /// Parses quick connect text. Surrounding whitespace is ignored.
    ///
    /// # Errors
    ///
    /// A [`QuickConnectError`] saying what's wrong, worded for the user.
    pub fn parse(input: &str) -> Result<Self, QuickConnectError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(QuickConnectError::Empty);
        }
        let (authority, is_url) = match input.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("ssh") => (rest, true),
            Some((scheme, _)) => {
                return Err(QuickConnectError::UnsupportedScheme(scheme.to_owned()));
            }
            None => (input, false),
        };
        let authority = if is_url {
            match authority.split_once('/') {
                Some((authority, "")) => authority,
                Some(_) => return Err(QuickConnectError::UrlPath),
                None => authority,
            }
        } else {
            authority
        };

        // The last `@` separates the user, so `alice@corp@host` logs in as `alice@corp`.
        let (user, host_port) = match authority.rsplit_once('@') {
            Some((user, host_port)) => {
                // `ssh://user;fingerprint=…@host` (the SSH URI draft): the parameters are
                // dropped; host keys are checked through known_hosts as always.
                let user = if is_url {
                    user.split(';').next().unwrap_or(user)
                } else {
                    user
                };
                let user = if is_url {
                    percent_decode(user)
                } else {
                    user.to_owned()
                };
                if user.is_empty() {
                    return Err(QuickConnectError::EmptyUser);
                }
                (Some(user), host_port)
            }
            None => (None, authority),
        };

        let (host, port) = split_host_port(host_port)?;
        let port = port.map(parse_port).transpose()?;
        Ok(Self {
            user,
            address: host.parse()?,
            port,
        })
    }
}

impl fmt::Display for ConnectTarget {
    /// `user@host:port`, leaving out what wasn't given, with IPv6 bracketed when a port follows.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(user) = &self.user {
            write!(f, "{user}@")?;
        }
        match self.port {
            Some(port) => write!(f, "{}:{port}", self.address.to_bracketed()),
            None => write!(f, "{}", self.address),
        }
    }
}

/// Splits `host`, `host:port`, `[v6]`, `[v6]:port`. A bare IPv6 address (two or more colons, no
/// brackets) is all host: there's no way to tell where a port would start.
fn split_host_port(text: &str) -> Result<(&str, Option<&str>), QuickConnectError> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or(QuickConnectError::UnclosedBracket)?;
        return match after {
            "" => Ok((host, None)),
            _ => after
                .strip_prefix(':')
                .map(|port| (host, Some(port)))
                .ok_or(QuickConnectError::UnclosedBracket),
        };
    }
    match text.matches(':').count() {
        1 => Ok(text
            .split_once(':')
            .map_or((text, None), |(host, port)| (host, Some(port)))),
        _ => Ok((text, None)),
    }
}

fn parse_port(text: &str) -> Result<u16, QuickConnectError> {
    match text.parse::<u16>() {
        Ok(port) if port != 0 => Ok(port),
        _ => Err(QuickConnectError::InvalidPort(text.to_owned())),
    }
}

/// Decodes `%XX` escapes (URL user names may escape `@` or `;`). Invalid escapes are kept as-is.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let escaped = (b == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(decoded) = escaped {
            out.push(decoded);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn target(user: Option<&str>, host: &str, port: Option<u16>) -> ConnectTarget {
        ConnectTarget {
            user: user.map(str::to_owned),
            address: host.parse().unwrap(),
            port,
        }
    }

    #[test]
    fn valid_inputs() {
        let cases = [
            ("example.com", target(None, "example.com", None)),
            ("  example.com  ", target(None, "example.com", None)),
            (
                "alice@example.com",
                target(Some("alice"), "example.com", None),
            ),
            ("example.com:2222", target(None, "example.com", Some(2222))),
            (
                "alice@example.com:2222",
                target(Some("alice"), "example.com", Some(2222)),
            ),
            (
                "alice@corp@example.com",
                target(Some("alice@corp"), "example.com", None),
            ),
            ("192.0.2.1:22", target(None, "192.0.2.1", Some(22))),
            ("::1", target(None, "::1", None)),
            ("2001:db8::1", target(None, "2001:db8::1", None)),
            ("[2001:db8::1]", target(None, "2001:db8::1", None)),
            (
                "bob@[2001:db8::1]:2200",
                target(Some("bob"), "2001:db8::1", Some(2200)),
            ),
            ("ssh://example.com", target(None, "example.com", None)),
            (
                "SSH://alice@example.com:2222/",
                target(Some("alice"), "example.com", Some(2222)),
            ),
            (
                "ssh://alice;fingerprint=SHA256-abc@example.com",
                target(Some("alice"), "example.com", None),
            ),
            (
                "ssh://al%40ice@example.com",
                target(Some("al@ice"), "example.com", None),
            ),
            ("ssh://bob@[::1]:22", target(Some("bob"), "::1", Some(22))),
            ("Example.COM", target(None, "example.com", None)),
        ];
        for (input, expected) in cases {
            assert_eq!(ConnectTarget::parse(input), Ok(expected), "{input:?}");
        }
    }

    #[test]
    fn invalid_inputs() {
        let cases = [
            ("", QuickConnectError::Empty),
            ("   ", QuickConnectError::Empty),
            (
                "http://example.com",
                QuickConnectError::UnsupportedScheme("http".into()),
            ),
            ("@example.com", QuickConnectError::EmptyUser),
            (
                "example.com:",
                QuickConnectError::InvalidPort(String::new()),
            ),
            ("example.com:0", QuickConnectError::InvalidPort("0".into())),
            (
                "example.com:70000",
                QuickConnectError::InvalidPort("70000".into()),
            ),
            (
                "example.com:ssh",
                QuickConnectError::InvalidPort("ssh".into()),
            ),
            ("[::1", QuickConnectError::UnclosedBracket),
            ("[::1]2222", QuickConnectError::UnclosedBracket),
            ("ssh://example.com/etc", QuickConnectError::UrlPath),
            ("alice@", QuickConnectError::Address(AddressError::Empty)),
            (
                "exa mple.com",
                QuickConnectError::Address(AddressError::InvalidCharacter(
                    "exa mple.com".into(),
                    ' ',
                )),
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(ConnectTarget::parse(input), Err(expected), "{input:?}");
        }
    }

    #[test]
    fn display_round_trips() {
        for input in [
            "example.com",
            "alice@example.com:2222",
            "bob@[2001:db8::1]:2200",
            "::1",
            "10.0.0.1:22",
        ] {
            let parsed = ConnectTarget::parse(input).unwrap();
            assert_eq!(parsed.to_string(), input);
            assert_eq!(ConnectTarget::parse(&parsed.to_string()).unwrap(), parsed);
        }
    }

    #[test]
    fn percent_decoding() {
        for (input, expected) in [
            ("a%40b", "a@b"),
            ("100%", "100%"),
            ("%zz", "%zz"),
            ("%41%42", "AB"),
            ("", ""),
        ] {
            assert_eq!(percent_decode(input), expected, "{input:?}");
        }
    }

    proptest::proptest! {
        #[test]
        fn never_panics(input in "\\PC{0,64}") {
            let _ = ConnectTarget::parse(&input);
        }

        #[test]
        fn display_reparses(
            user in proptest::option::of("[a-z][a-z0-9_.-]{0,8}"),
            host in "[a-z][a-z0-9-]{0,10}(\\.[a-z]{2,5})?",
            port in proptest::option::of(1u16..),
        ) {
            let target = ConnectTarget { user, address: host.parse().unwrap(), port };
            proptest::prop_assert_eq!(ConnectTarget::parse(&target.to_string()), Ok(target));
        }
    }
}
