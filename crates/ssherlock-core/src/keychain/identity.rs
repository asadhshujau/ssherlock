//! An identity: a user name with a password and/or key, shared by hosts (KEY-03).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Key, LabelError, check_label};
use crate::id::Id;
use crate::secret::Sealed;

/// A user name and the credentials to log in with it. Hosts and groups pick an identity
/// instead of repeating these, so changing it once changes it everywhere.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Unique id.
    pub id: Id<Identity>,
    /// The name shown in pickers. Empty means "show the user name".
    #[serde(default)]
    pub label: String,
    /// The user to log in as.
    pub username: String,
    /// The password, sealed by the vault. `None` means ask when needed.
    #[serde(default)]
    pub password: Option<Sealed>,
    /// The key to log in with.
    #[serde(default)]
    pub key: Option<Id<Key>>,
    /// When the identity was created.
    pub created_at: Timestamp,
    /// When the identity was last edited.
    pub updated_at: Timestamp,
}

/// Why an [`Identity`] can't be saved.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// The label is longer than 120 characters.
    #[error("the label is longer than 120 characters")]
    LabelTooLong,
    /// The label has a newline or other control character.
    #[error("the label cannot contain line breaks or control characters")]
    LabelControl,
    /// The user name is empty.
    #[error("an identity needs a user name")]
    EmptyUsername,
    /// The user name has a space or control character.
    #[error("the user name {0:?} contains {1:?}, which is not allowed")]
    InvalidUsername(String, char),
}

impl Identity {
    /// A new identity for `username`, created at `now`.
    pub fn new(username: impl Into<String>, now: Timestamp) -> Self {
        Self {
            id: Id::generate(),
            label: String::new(),
            username: username.into(),
            password: None,
            key: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// The name to show: the label, or the user name when there's no label.
    pub fn title(&self) -> &str {
        let label = self.label.trim();
        if label.is_empty() {
            &self.username
        } else {
            label
        }
    }

    /// Checks the identity can be saved. Pure and total.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), IdentityError> {
        check_label(&self.label).map_err(|e| match e {
            LabelError::TooLong => IdentityError::LabelTooLong,
            LabelError::Control => IdentityError::LabelControl,
        })?;
        if self.username.is_empty() {
            return Err(IdentityError::EmptyUsername);
        }
        if let Some(bad) = self
            .username
            .chars()
            .find(|c| c.is_whitespace() || c.is_control())
        {
            return Err(IdentityError::InvalidUsername(self.username.clone(), bad));
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
        let with = |label: &str, user: &str| Identity {
            label: label.into(),
            ..Identity::new(user, Timestamp::UNIX_EPOCH)
        };
        let cases = [
            (with("", "deploy"), Ok(())),
            (with("Prod deploy", "deploy@corp"), Ok(())),
            (with("", ""), Err(IdentityError::EmptyUsername)),
            (
                with("", "de ploy"),
                Err(IdentityError::InvalidUsername("de ploy".into(), ' ')),
            ),
            (
                with(&"x".repeat(121), "u"),
                Err(IdentityError::LabelTooLong),
            ),
            (with("a\nb", "u"), Err(IdentityError::LabelControl)),
        ];
        for (identity, expected) in cases {
            assert_eq!(identity.validate(), expected, "{identity:?}");
        }
    }

    #[test]
    fn title_falls_back_to_the_user_name() {
        let mut identity = Identity::new("deploy", Timestamp::UNIX_EPOCH);
        assert_eq!(identity.title(), "deploy");
        identity.label = "Prod".into();
        assert_eq!(identity.title(), "Prod");
    }

    #[test]
    fn missing_optional_fields_load_with_defaults() {
        let json = r#"{
            "id": "01936f4e-7a2b-7c3d-8e4f-5a6b7c8d9e0f",
            "username": "deploy",
            "created_at": "2026-01-02T03:04:05Z",
            "updated_at": "2026-01-02T03:04:05Z"
        }"#;
        let identity: Identity = serde_json::from_str(json).unwrap();
        assert_eq!(identity.title(), "deploy");
        assert!(identity.password.is_none() && identity.key.is_none());
    }
}
