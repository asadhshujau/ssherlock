//! A TCP port number, never 0.
//!
//! **Model** layer. Shared by quick connect and saved hosts so both parse and reject ports the
//! same way.

use std::fmt;
use std::num::NonZeroU16;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A TCP port from 1 to 65535. Port 0 means "any port" to the OS, which is never what a user
/// means when they type it, so it can't be represented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Port(NonZeroU16);

/// Why text or a number isn't a valid [`Port`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the port {0:?} is not a number from 1 to 65535")]
pub struct PortError(pub String);

impl Port {
    /// The SSH port, 22.
    pub const SSH: Self = Self(NonZeroU16::MIN.saturating_add(21));

    /// `number` as a port, or `None` for 0.
    pub const fn new(number: u16) -> Option<Self> {
        match NonZeroU16::new(number) {
            Some(port) => Some(Self(port)),
            None => None,
        }
    }

    /// The port number.
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

impl Default for Port {
    /// [`Port::SSH`].
    fn default() -> Self {
        Self::SSH
    }
}

impl FromStr for Port {
    type Err = PortError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse::<u16>()
            .ok()
            .and_then(Self::new)
            .ok_or_else(|| PortError(text.to_owned()))
    }
}

impl TryFrom<u16> for Port {
    type Error = PortError;

    fn try_from(number: u16) -> Result<Self, Self::Error> {
        Self::new(number).ok_or_else(|| PortError(number.to_string()))
    }
}

impl From<Port> for u16 {
    fn from(port: Port) -> Self {
        port.get()
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parsing() {
        let cases = [
            ("22", Ok(22)),
            ("1", Ok(1)),
            ("65535", Ok(65535)),
            ("0", Err(())),
            ("65536", Err(())),
            ("", Err(())),
            ("ssh", Err(())),
            ("-1", Err(())),
            (" 22", Err(())),
        ];
        for (text, expected) in cases {
            let parsed = text.parse::<Port>();
            assert_eq!(
                parsed.as_ref().map(|p| p.get()).map_err(|_| ()),
                expected,
                "{text:?}"
            );
            if let Err(error) = parsed {
                assert_eq!(error, PortError(text.to_owned()));
            }
        }
    }

    #[test]
    fn ssh_is_22_and_the_default() {
        assert_eq!(Port::SSH.get(), 22);
        assert_eq!(Port::default(), Port::SSH);
        assert_eq!(Port::SSH.to_string(), "22");
    }

    #[test]
    fn serde_uses_the_number_and_refuses_zero() {
        let port = Port::new(2222).unwrap();
        assert_eq!(serde_json::to_string(&port).unwrap(), "2222");
        assert_eq!(serde_json::from_str::<Port>("2222").unwrap(), port);
        assert!(serde_json::from_str::<Port>("0").is_err());
    }
}
