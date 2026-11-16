//! The hosts page's navigation column: what it lists, as plain data tested without gpui.

use std::collections::BTreeSet;

use ssherlock_core::Id;
use ssherlock_core::hosts::search::Scope;
use ssherlock_core::hosts::{Colour, Group, GroupTree, Host, Tag};

/// Which part of the hosts page is shown.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// Hosts in a scope: all, favourites, recent, or a group.
    Hosts(Scope),
    /// Hosts with a tag.
    Tag(Tag),
    /// Saved keys (the keychain).
    Keys,
    /// Deleted hosts and groups.
    Trash,
}

impl Default for Section {
    /// Every host.
    fn default() -> Self {
        Self::all()
    }
}

impl Section {
    /// Every host, the page's starting point.
    pub fn all() -> Self {
        Self::Hosts(Scope::All)
    }
}

/// One line in the navigation column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavItem {
    /// A heading ("Groups", "Tags"); not selectable.
    Heading(&'static str),
    /// A section to show.
    Entry {
        /// What choosing it shows.
        section: Section,
        /// The text.
        label: String,
        /// How many hosts (or trash entries) it holds.
        count: usize,
        /// 0 at the top level; groups indent by depth.
        depth: usize,
        /// For groups with groups inside: whether they're shown.
        expanded: Option<bool>,
        /// The group's colour mark.
        colour: Option<Colour>,
    },
}

impl NavItem {
    /// The section this line selects, if it selects one.
    pub fn section(&self) -> Option<&Section> {
        match self {
            Self::Heading(_) => None,
            Self::Entry { section, .. } => Some(section),
        }
    }
}

/// The lines of the navigation column for these hosts and groups, with the groups for which
/// `is_expanded` says yes showing what's inside them.
pub fn items(
    hosts: &[Host],
    groups: &[Group],
    counts: Counts,
    is_expanded: impl Fn(Id<Group>) -> bool,
) -> Vec<NavItem> {
    let tree = GroupTree::new(groups);
    let entry = |section: Section, label: &str, count: usize| NavItem::Entry {
        section,
        label: label.to_owned(),
        count,
        depth: 0,
        expanded: None,
        colour: None,
    };
    let mut items = vec![
        entry(Section::all(), "All hosts", hosts.len()),
        entry(
            Section::Hosts(Scope::Favourites),
            "Favourites",
            hosts.iter().filter(|h| h.is_favourite).count(),
        ),
        entry(
            Section::Hosts(Scope::Recent),
            "Recent",
            hosts
                .iter()
                .filter(|h| h.last_connected_at.is_some())
                .count(),
        ),
    ];

    let rows = tree.rows(&is_expanded);
    if !rows.is_empty() {
        items.push(NavItem::Heading("Groups"));
        items.extend(rows.into_iter().map(|row| {
            NavItem::Entry {
                section: Section::Hosts(Scope::Group(row.group.id)),
                label: row.group.name.clone(),
                count: hosts
                    .iter()
                    .filter(|h| tree.is_within(h.group, row.group.id))
                    .count(),
                depth: row.depth,
                expanded: row.has_children.then(|| is_expanded(row.group.id)),
                colour: row.group.settings.colour,
            }
        }));
    }

    let tags: BTreeSet<&Tag> = hosts.iter().flat_map(|h| &h.tags).collect();
    if !tags.is_empty() {
        items.push(NavItem::Heading("Tags"));
        items.extend(tags.into_iter().map(|tag| {
            entry(
                Section::Tag(tag.clone()),
                &format!("#{tag}"),
                hosts.iter().filter(|h| h.tags.contains(tag)).count(),
            )
        }));
    }

    items.push(NavItem::Heading("Keychain"));
    items.push(entry(Section::Keys, "Keys", counts.keys));
    items.push(NavItem::Heading("Deleted"));
    items.push(entry(Section::Trash, "Trash", counts.trash));
    items
}

/// How many of the things outside the host list there are, for the navigation counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Saved keys.
    pub keys: usize,
    /// Trash entries.
    pub trash: usize,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::collections::HashSet;

    use pretty_assertions::assert_eq;
    use ssherlock_core::Timestamp;

    use super::*;

    fn labels(items: &[NavItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                NavItem::Heading(text) => format!("[{text}]"),
                NavItem::Entry {
                    label,
                    count,
                    depth,
                    expanded,
                    ..
                } => {
                    let arrow = match expanded {
                        Some(true) => "v ",
                        Some(false) => "> ",
                        None => "",
                    };
                    format!("{}{arrow}{label} {count}", "  ".repeat(*depth))
                }
            })
            .collect()
    }

    #[test]
    fn lists_scopes_groups_tags_and_trash() {
        let now = Timestamp::UNIX_EPOCH;
        let prod = Group::new("prod", None, now);
        let db = Group::new("db", Some(prod.id), now);
        let mut a = Host::new("a".parse().unwrap(), now);
        a.group = Some(db.id);
        a.tags = Tag::parse_list("pg").unwrap();
        a.is_favourite = true;
        let mut b = Host::new("b".parse().unwrap(), now);
        b.group = Some(prod.id);
        b.last_connected_at = Some(now);
        let hosts = vec![a, b];
        let groups = vec![prod.clone(), db];

        let counts = Counts { keys: 3, trash: 2 };
        let collapsed = items(&hosts, &groups, counts, |_| false);
        assert_eq!(
            labels(&collapsed),
            [
                "All hosts 2",
                "Favourites 1",
                "Recent 1",
                "[Groups]",
                "> prod 2",
                "[Tags]",
                "#pg 1",
                "[Keychain]",
                "Keys 3",
                "[Deleted]",
                "Trash 2",
            ]
        );
        let open_groups = HashSet::from([prod.id]);
        let open = items(&hosts, &groups, Counts::default(), |id| {
            open_groups.contains(&id)
        });
        assert_eq!(
            labels(&open)[4..6],
            ["v prod 2".to_owned(), "  db 1".to_owned()]
        );
        assert_eq!(open.iter().filter(|i| i.section().is_none()).count(), 4);
    }

    #[test]
    fn empty_lists_have_no_headings() {
        let only = items(&[], &[], Counts::default(), |_| false);
        assert_eq!(
            labels(&only),
            [
                "All hosts 0",
                "Favourites 0",
                "Recent 0",
                "[Keychain]",
                "Keys 0",
                "[Deleted]",
                "Trash 0"
            ]
        );
    }
}
