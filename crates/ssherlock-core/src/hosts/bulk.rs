//! Changing many hosts at once (HOST-08): what a bulk edit changes, and applying it.

use std::collections::BTreeSet;

use jiff::Timestamp;

use super::{Colour, Group, Host, Tag};
use crate::id::Id;
use crate::port::Port;

/// A change to one setting across many hosts: leave each as it is, or set them all to the same
/// value (which may be "inherit", `None`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Change<T> {
    /// Each host keeps its own value.
    #[default]
    Keep,
    /// Every host gets this value.
    Set(T),
}

impl<T: Clone> Change<T> {
    fn apply(&self, value: &mut T) {
        if let Self::Set(new) = self {
            value.clone_from(new);
        }
    }
}

/// What a bulk edit does. Everything defaults to leaving hosts alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BulkEdit {
    /// Move them into a group, or out of every group.
    pub group: Change<Option<Id<Group>>>,
    /// Tags to add to each.
    pub add_tags: BTreeSet<Tag>,
    /// Tags to take off each.
    pub remove_tags: BTreeSet<Tag>,
    /// The user to log in as; `Set(None)` makes them inherit it.
    pub username: Change<Option<String>>,
    /// The port; `Set(None)` makes them inherit it.
    pub port: Change<Option<Port>>,
    /// The colour mark; `Set(None)` makes them inherit it.
    pub colour: Change<Option<Colour>>,
    /// Favourite or not.
    pub is_favourite: Change<bool>,
}

impl BulkEdit {
    /// Whether it changes anything at all.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `host` with the edit applied, marked as edited at `now` if anything changed.
    #[must_use]
    pub fn apply(&self, host: &Host, now: Timestamp) -> Host {
        let mut edited = host.clone();
        self.group.apply(&mut edited.group);
        // Removing wins over adding, so "remove prod, add prod" can't be read two ways.
        edited.tags.extend(self.add_tags.iter().cloned());
        edited.tags.retain(|tag| !self.remove_tags.contains(tag));
        self.username.apply(&mut edited.settings.username);
        self.port.apply(&mut edited.settings.port);
        self.colour.apply(&mut edited.settings.colour);
        self.is_favourite.apply(&mut edited.is_favourite);
        if edited != *host {
            edited.updated_at = now;
        }
        edited
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn host() -> Host {
        let mut h = Host::new("example.com".parse().unwrap(), Timestamp::UNIX_EPOCH);
        h.tags = Tag::parse_list("prod web").unwrap();
        h.settings.username = Some("alice".into());
        h.settings.port = Port::new(2222);
        h
    }

    #[test]
    fn an_empty_edit_changes_nothing() {
        let original = host();
        let now = Timestamp::from_second(100).unwrap();
        assert!(BulkEdit::default().is_empty());
        assert_eq!(BulkEdit::default().apply(&original, now), original);
    }

    #[test]
    fn each_change_applies_and_keep_leaves_alone() {
        let now = Timestamp::from_second(100).unwrap();
        let group = Id::generate();
        let edit = BulkEdit {
            group: Change::Set(Some(group)),
            add_tags: Tag::parse_list("db").unwrap(),
            remove_tags: Tag::parse_list("web, db").unwrap(),
            username: Change::Set(None),
            colour: Change::Set(Some(Colour::Blue)),
            is_favourite: Change::Set(true),
            ..BulkEdit::default()
        };
        let edited = edit.apply(&host(), now);
        assert_eq!(edited.group, Some(group));
        assert_eq!(
            Tag::format_list(&edited.tags),
            "prod",
            "removing wins over adding"
        );
        assert_eq!(edited.settings.username, None, "set to inherit");
        assert_eq!(edited.settings.port, Port::new(2222), "kept");
        assert_eq!(edited.settings.colour, Some(Colour::Blue));
        assert!(edited.is_favourite);
        assert_eq!(edited.updated_at, now);
    }
}
