//! Connection settings a group passes down to everything inside it and a host can override
//! (HOST-02, CONN-17, CONN-18).

use serde::{Deserialize, Serialize};

use super::Colour;
use crate::algorithms::{AlgorithmPreferences, Compression};
use crate::port::Port;

/// Settings shared by hosts and groups. Each field left unset (`None`, or an empty algorithm
/// list) is inherited from the nearest group that sets it, else SSHerlock's default.
///
/// Later phases add fields here (identity, environment, terminal settings) and get inheritance
/// for free; every new field must be `#[serde(default)]` so saved data still loads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostSettings {
    /// The SSH port. Default 22.
    pub port: Option<Port>,
    /// The user to log in as. Default: the local user name.
    pub username: Option<String>,
    /// The colour marking the host's row and tabs. Default: none.
    pub colour: Option<Colour>,
    /// Algorithms to offer; each empty list is inherited.
    pub algorithms: AlgorithmPreferences,
    /// Whether to compress. Default off.
    pub compression: Option<Compression>,
}

/// Why [`HostSettings`] can't be saved.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// The user name is set but empty.
    #[error("the user name is empty; leave it unset to inherit it")]
    EmptyUsername,
    /// The user name has a space or control character, which no server accepts.
    #[error("the user name {0:?} contains {1:?}, which is not allowed")]
    InvalidUsername(String, char),
    /// An algorithm name is blank or has spaces in it.
    #[error("the algorithm name {0:?} is not valid")]
    InvalidAlgorithmName(String),
}

impl HostSettings {
    /// Checks the fields that can hold bad text. Algorithm names are only checked for shape
    /// here; whether SSHerlock supports them is `ssherlock-ssh`'s call.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if let Some(user) = &self.username {
            if user.is_empty() {
                return Err(SettingsError::EmptyUsername);
            }
            if let Some(bad) = user.chars().find(|c| c.is_whitespace() || c.is_control()) {
                return Err(SettingsError::InvalidUsername(user.clone(), bad));
            }
        }
        let names = [
            &self.algorithms.kex,
            &self.algorithms.host_key,
            &self.algorithms.cipher,
            &self.algorithms.mac,
        ];
        if let Some(bad) = names.into_iter().flatten().find(|name| {
            name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control())
        }) {
            return Err(SettingsError::InvalidAlgorithmName(bad.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn validation() {
        let with_user = |user: &str| HostSettings {
            username: Some(user.into()),
            ..HostSettings::default()
        };
        let with_cipher = |name: &str| HostSettings {
            algorithms: AlgorithmPreferences {
                cipher: vec![name.into()],
                ..AlgorithmPreferences::default()
            },
            ..HostSettings::default()
        };
        let cases = [
            (HostSettings::default(), Ok(())),
            (with_user("alice"), Ok(())),
            (with_user("alice@corp"), Ok(())),
            (with_user(""), Err(SettingsError::EmptyUsername)),
            (
                with_user("al ice"),
                Err(SettingsError::InvalidUsername("al ice".into(), ' ')),
            ),
            (
                with_user("al\nice"),
                Err(SettingsError::InvalidUsername("al\nice".into(), '\n')),
            ),
            (with_cipher("aes128-ctr"), Ok(())),
            (
                with_cipher(""),
                Err(SettingsError::InvalidAlgorithmName(String::new())),
            ),
            (
                with_cipher("aes 128"),
                Err(SettingsError::InvalidAlgorithmName("aes 128".into())),
            ),
        ];
        for (settings, expected) in cases {
            assert_eq!(settings.validate(), expected, "{settings:?}");
        }
    }

    #[test]
    fn empty_json_is_all_inherited() {
        let settings: HostSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, HostSettings::default());
    }
}
