//! A saved host (HOST-01).

use std::borrow::Cow;
use std::collections::BTreeSet;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Group, HostSettings, MAX_NAME_LEN, SettingsError, Tag, has_control};
use crate::address::Address;
use crate::id::Id;

/// A saved SSH host.
///
/// Credentials arrive with the keychain (Phase 4) as an identity in [`HostSettings`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// Unique id.
    pub id: Id<Host>,
    /// The name shown in the list. Empty means "show the address".
    #[serde(default)]
    pub label: String,
    /// Where the server is.
    pub address: Address,
    /// The group the host is in, if any. Settings it doesn't set are inherited from there.
    #[serde(default)]
    pub group: Option<Id<Group>>,
    /// Tags for filtering. A set, so a tag can't be added twice.
    #[serde(default)]
    pub tags: BTreeSet<Tag>,
    /// Free-form notes.
    #[serde(default)]
    pub notes: String,
    /// Shown under Favourites (HOST-06).
    #[serde(default)]
    pub is_favourite: bool,
    /// The host's own settings; anything unset is inherited.
    #[serde(default)]
    pub settings: HostSettings,
    /// When the host was created.
    pub created_at: Timestamp,
    /// When the host was last edited.
    pub updated_at: Timestamp,
    /// When the host was last connected to, for Recents (HOST-07). Recorded on connect, not by
    /// editing, so saving a host never changes it.
    #[serde(default)]
    pub last_connected_at: Option<Timestamp>,
}

/// Why a [`Host`] can't be saved.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// The label is longer than [`MAX_NAME_LEN`].
    #[error("the label is longer than 120 characters")]
    LabelTooLong,
    /// The label has a newline or other control character.
    #[error("the label cannot contain line breaks or control characters")]
    LabelControl,
    /// One of the settings is invalid.
    #[error(transparent)]
    Settings(#[from] SettingsError),
}

impl Host {
    /// A new host at `address` with nothing else set, created at `now`.
    pub fn new(address: Address, now: Timestamp) -> Self {
        Self {
            id: Id::generate(),
            label: String::new(),
            address,
            group: None,
            tags: BTreeSet::new(),
            notes: String::new(),
            is_favourite: false,
            settings: HostSettings::default(),
            created_at: now,
            updated_at: now,
            last_connected_at: None,
        }
    }

    /// The name to show: the label, or the address when there's no label.
    pub fn title(&self) -> Cow<'_, str> {
        let label = self.label.trim();
        if label.is_empty() {
            Cow::Owned(self.address.to_string())
        } else {
            Cow::Borrowed(label)
        }
    }

    /// Checks the host can be saved. Pure and total.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), HostError> {
        if self.label.chars().count() > MAX_NAME_LEN {
            return Err(HostError::LabelTooLong);
        }
        if has_control(&self.label) {
            return Err(HostError::LabelControl);
        }
        self.settings.validate()?;
        Ok(())
    }

    /// A copy with a new id, saved at `now` (HOST-08). The label gets " (copy)" so the two can
    /// be told apart; the copy isn't a favourite and has never been connected to.
    #[must_use]
    pub fn duplicate(&self, now: Timestamp) -> Self {
        let label = format!("{} (copy)", self.title());
        let label = if label.chars().count() > MAX_NAME_LEN {
            self.label.clone()
        } else {
            label
        };
        Self {
            id: Id::generate(),
            label,
            is_favourite: false,
            created_at: now,
            updated_at: now,
            last_connected_at: None,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn host(address: &str) -> Host {
        Host::new(address.parse().unwrap(), Timestamp::UNIX_EPOCH)
    }

    #[test]
    fn title_falls_back_to_the_address() {
        let mut h = host("db.example.com");
        assert_eq!(h.title(), "db.example.com");
        h.label = "   ".into();
        assert_eq!(h.title(), "db.example.com");
        h.label = " Prod DB ".into();
        assert_eq!(h.title(), "Prod DB");
    }

    #[test]
    fn validation() {
        let with_label = |label: &str| Host {
            label: label.into(),
            ..host("example.com")
        };
        let cases = [
            (with_label(""), Ok(())),
            (with_label("Prod — DB 1"), Ok(())),
            (with_label(&"é".repeat(120)), Ok(())),
            (with_label(&"é".repeat(121)), Err(HostError::LabelTooLong)),
            (with_label("two\nlines"), Err(HostError::LabelControl)),
            (
                Host {
                    settings: HostSettings {
                        username: Some(String::new()),
                        ..HostSettings::default()
                    },
                    ..host("example.com")
                },
                Err(HostError::Settings(SettingsError::EmptyUsername)),
            ),
        ];
        for (h, expected) in cases {
            assert_eq!(h.validate(), expected, "{:?}", h.label);
        }
    }

    #[test]
    fn duplicate_gets_a_new_identity_and_keeps_the_rest() {
        let mut original = host("example.com");
        original.label = "Web".into();
        original.notes = "notes".into();
        original.is_favourite = true;
        original.tags = Tag::parse_list("prod").unwrap();
        original.last_connected_at = Some(Timestamp::UNIX_EPOCH);
        let now = Timestamp::from_second(1_000).unwrap();

        let copy = original.duplicate(now);
        assert_ne!(copy.id, original.id);
        assert_eq!(copy.label, "Web (copy)");
        assert_eq!((copy.created_at, copy.updated_at), (now, now));
        assert_eq!(copy.last_connected_at, None);
        assert!(!copy.is_favourite);
        assert_eq!(
            (&copy.address, &copy.notes, &copy.tags),
            (&original.address, &original.notes, &original.tags)
        );
        assert_eq!(host("10.0.0.1").duplicate(now).label, "10.0.0.1 (copy)");
        // A label at the limit is kept as it is rather than made invalid.
        original.label = "x".repeat(MAX_NAME_LEN);
        assert!(original.duplicate(now).validate().is_ok());
    }

    #[test]
    fn missing_optional_fields_load_with_defaults() {
        let json = r#"{
            "id": "01936f4e-7a2b-7c3d-8e4f-5a6b7c8d9e0f",
            "address": "example.com",
            "created_at": "2026-01-02T03:04:05Z",
            "updated_at": "2026-01-02T03:04:05Z"
        }"#;
        let h: Host = serde_json::from_str(json).unwrap();
        assert_eq!(h.title(), "example.com");
        assert!(h.tags.is_empty() && h.group.is_none() && h.last_connected_at.is_none());
        let back: Host = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back, h);
    }
}
