//! Where a server is: an IP address or a DNS name.
//!
//! **Model** layer. Shared by quick connect now and saved hosts later, so both validate and
//! print addresses the same way.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A server's address. IPv6 literals are stored without brackets.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Address {
    /// An IPv4 or IPv6 literal.
    Ip(IpAddr),
    /// A DNS name (or anything resolvable that isn't an IP literal), lower-cased.
    Name(String),
}

/// Why text isn't a valid [`Address`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    /// Nothing was given.
    #[error("the address is empty")]
    Empty,
    /// It contains a character no host name or IP literal can have.
    #[error("the address {0:?} contains {1:?}, which is not allowed in a host name")]
    InvalidCharacter(String, char),
    /// It's longer than DNS allows (253 characters).
    #[error("the address is longer than 253 characters")]
    TooLong,
}

impl Address {
    /// The address as `host` or, for IPv6, `[host]`, ready to have `:port` appended.
    pub fn to_bracketed(&self) -> String {
        match self {
            Self::Ip(IpAddr::V6(ip)) => format!("[{ip}]"),
            other => other.to_string(),
        }
    }
}

impl FromStr for Address {
    type Err = AddressError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() {
            return Err(AddressError::Empty);
        }
        // An IPv6 zone (`fe80::1%eth0`) doesn't parse as `IpAddr`; it's kept as a name so the
        // system resolver can handle it.
        if let Ok(ip) = text.parse::<IpAddr>() {
            return Ok(Self::Ip(ip));
        }
        if text.len() > 253 {
            return Err(AddressError::TooLong);
        }
        if let Some(bad) = text
            .chars()
            .find(|&c| !(c.is_ascii_alphanumeric() || "-._:%".contains(c)))
        {
            return Err(AddressError::InvalidCharacter(text.to_owned(), bad));
        }
        Ok(Self::Name(text.to_ascii_lowercase()))
    }
}

impl TryFrom<String> for Address {
    type Error = AddressError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Address> for String {
    fn from(address: Address) -> Self {
        address.to_string()
    }
}

impl fmt::Display for Address {
    /// The bare address (IPv6 without brackets), as name resolution and `known_hosts` want it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(ip) => ip.fmt(f),
            Self::Name(name) => f.write_str(name),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::{Ipv4Addr, Ipv6Addr};

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parsing() {
        let cases: Vec<(&str, Result<Address, AddressError>)> = vec![
            ("example.com", Ok(Address::Name("example.com".into()))),
            ("Example.COM", Ok(Address::Name("example.com".into()))),
            ("my_host-1", Ok(Address::Name("my_host-1".into()))),
            (
                "192.0.2.1",
                Ok(Address::Ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)))),
            ),
            ("::1", Ok(Address::Ip(IpAddr::V6(Ipv6Addr::LOCALHOST)))),
            ("fe80::1%eth0", Ok(Address::Name("fe80::1%eth0".into()))),
            ("", Err(AddressError::Empty)),
            (
                "exa mple",
                Err(AddressError::InvalidCharacter("exa mple".into(), ' ')),
            ),
            (
                "host/path",
                Err(AddressError::InvalidCharacter("host/path".into(), '/')),
            ),
            (
                "a@b",
                Err(AddressError::InvalidCharacter("a@b".into(), '@')),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(text.parse::<Address>(), expected, "{text:?}");
        }
        assert_eq!(
            "a".repeat(254).parse::<Address>(),
            Err(AddressError::TooLong)
        );
    }

    #[test]
    fn printing() {
        for (text, plain, bracketed) in [
            ("example.com", "example.com", "example.com"),
            ("192.0.2.1", "192.0.2.1", "192.0.2.1"),
            ("::1", "::1", "[::1]"),
        ] {
            let address: Address = text.parse().unwrap();
            assert_eq!(address.to_string(), plain);
            assert_eq!(address.to_bracketed(), bracketed);
        }
    }

    #[test]
    fn serde_round_trip() {
        let address: Address = "2001:db8::7".parse().unwrap();
        let json = serde_json::to_string(&address).unwrap();
        assert_eq!(json, "\"2001:db8::7\"");
        assert_eq!(serde_json::from_str::<Address>(&json).unwrap(), address);
        assert!(serde_json::from_str::<Address>("\"bad host\"").is_err());
    }
}
