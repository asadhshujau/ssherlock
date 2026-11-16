//! Deleted things kept for restoring (HOST-09).
//!
//! **Model** layer. Deleting moves a host, or a group with everything inside it, into one
//! [`TrashEntry`]; restoring puts it all back. Entries hold full copies of the records, so the
//! live tables never carry half-deleted rows.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::hosts::{Group, Host};
use crate::id::Id;

/// One deletion, restorable as a unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashEntry {
    /// Unique id.
    pub id: Id<TrashEntry>,
    /// When it was deleted.
    pub deleted_at: Timestamp,
    /// What was deleted.
    pub contents: TrashContents,
}

/// The records a [`TrashEntry`] holds. Later features add their own lists, each
/// `#[serde(default)]` so older entries still load.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrashContents {
    /// Groups, parents before children.
    pub groups: Vec<Group>,
    /// Hosts.
    pub hosts: Vec<Host>,
}

impl TrashEntry {
    /// An entry for `contents`, deleted at `now`.
    pub fn new(contents: TrashContents, now: Timestamp) -> Self {
        Self {
            id: Id::generate(),
            deleted_at: now,
            contents,
        }
    }

    /// What the trash list shows: the group's name when a group was deleted, else the host's
    /// title, plus how many hosts went with it.
    pub fn title(&self) -> String {
        let hosts = self.contents.hosts.len();
        match (self.contents.groups.first(), self.contents.hosts.first()) {
            (Some(group), _) => match hosts {
                0 => group.name.clone(),
                1 => format!("{} (1 host)", group.name),
                n => format!("{} ({n} hosts)", group.name),
            },
            (None, Some(host)) if hosts == 1 => host.title().into_owned(),
            (None, Some(_)) => format!("{hosts} hosts"),
            (None, None) => "Nothing".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn titles() {
        let now = Timestamp::UNIX_EPOCH;
        let host = |label: &str| {
            let mut h = Host::new("example.com".parse().unwrap(), now);
            h.label = label.into();
            h
        };
        let group = Group::new("Prod", None, now);
        let entry = |groups: Vec<Group>, hosts: Vec<Host>| {
            TrashEntry::new(TrashContents { groups, hosts }, now).title()
        };
        assert_eq!(entry(vec![], vec![host("Web")]), "Web");
        assert_eq!(entry(vec![], vec![host("")]), "example.com");
        assert_eq!(entry(vec![], vec![host("a"), host("b")]), "2 hosts");
        assert_eq!(entry(vec![group.clone()], vec![]), "Prod");
        assert_eq!(entry(vec![group.clone()], vec![host("a")]), "Prod (1 host)");
        assert_eq!(
            entry(vec![group], vec![host("a"), host("b")]),
            "Prod (2 hosts)"
        );
        assert_eq!(entry(vec![], vec![]), "Nothing");
    }

    #[test]
    fn contents_round_trip_through_json() {
        let now = Timestamp::UNIX_EPOCH;
        let entry = TrashEntry::new(
            TrashContents {
                groups: vec![Group::new("g", None, now)],
                hosts: vec![Host::new("h".parse().unwrap(), now)],
            },
            now,
        );
        let json = serde_json::to_string(&entry.contents).unwrap();
        assert_eq!(
            serde_json::from_str::<TrashContents>(&json).unwrap(),
            entry.contents
        );
        assert_eq!(
            serde_json::from_str::<TrashContents>("{}").unwrap(),
            TrashContents::default()
        );
    }
}
