//! Groups of hosts, nested to any depth (HOST-02), and [`GroupTree`] for questions about the
//! nesting.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{HostSettings, MAX_NAME_LEN, SettingsError, has_control};
use crate::id::Id;

/// A group of hosts and other groups. Its settings apply to everything inside it unless
/// overridden closer to the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// Unique id.
    pub id: Id<Group>,
    /// The name shown in the sidebar.
    pub name: String,
    /// The group this one is inside, or `None` at the top level.
    #[serde(default)]
    pub parent: Option<Id<Group>>,
    /// Settings passed down to everything inside.
    #[serde(default)]
    pub settings: HostSettings,
    /// When the group was created.
    pub created_at: Timestamp,
    /// When the group was last edited.
    pub updated_at: Timestamp,
}

/// Why a [`Group`] can't be saved or moved.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GroupError {
    /// The name is empty or only spaces.
    #[error("the group name is empty")]
    EmptyName,
    /// The name is longer than [`MAX_NAME_LEN`].
    #[error("the group name is longer than 120 characters")]
    NameTooLong,
    /// The name has a newline or other control character.
    #[error("the group name cannot contain line breaks or control characters")]
    NameControl,
    /// The new parent is the group itself or inside it.
    #[error("a group cannot be moved inside itself")]
    Cycle,
    /// The new parent doesn't exist.
    #[error("the parent group no longer exists")]
    UnknownParent,
    /// One of the settings is invalid.
    #[error(transparent)]
    Settings(#[from] SettingsError),
}

impl Group {
    /// A new group called `name` inside `parent`, created at `now`.
    pub fn new(name: impl Into<String>, parent: Option<Id<Self>>, now: Timestamp) -> Self {
        Self {
            id: Id::generate(),
            name: name.into(),
            parent,
            settings: HostSettings::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Checks the group's own fields. Where it sits is checked by [`GroupTree::check_parent`].
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), GroupError> {
        if self.name.trim().is_empty() {
            return Err(GroupError::EmptyName);
        }
        if self.name.chars().count() > MAX_NAME_LEN {
            return Err(GroupError::NameTooLong);
        }
        if has_control(&self.name) {
            return Err(GroupError::NameControl);
        }
        self.settings.validate()?;
        Ok(())
    }
}

/// One line of the sidebar's group tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupRow<'a> {
    /// The group on this line.
    pub group: &'a Group,
    /// 0 for top-level groups, 1 inside those, and so on.
    pub depth: usize,
    /// Whether it has groups inside it (so the row gets an expand arrow).
    pub has_children: bool,
}

/// An index over a list of groups for nesting questions.
///
/// It tolerates bad data instead of looping or losing groups: a group whose parent is missing,
/// or whose chain of parents loops back on itself, is treated as top-level.
#[derive(Debug)]
pub struct GroupTree<'a> {
    by_id: HashMap<Id<Group>, &'a Group>,
    /// Children of each parent (`None` = top level), sorted by name.
    children: HashMap<Option<Id<Group>>, Vec<&'a Group>>,
}

impl<'a> GroupTree<'a> {
    /// Indexes `groups`.
    pub fn new(groups: &'a [Group]) -> Self {
        let by_id: HashMap<_, _> = groups.iter().map(|g| (g.id, g)).collect();
        let mut children: HashMap<Option<Id<Group>>, Vec<&'a Group>> = HashMap::new();
        for group in groups {
            let parent = group
                .parent
                .filter(|parent| by_id.contains_key(parent) && !loops(group, &by_id));
            children.entry(parent).or_default().push(group);
        }
        for list in children.values_mut() {
            list.sort_by_cached_key(|g| sort_key(g));
        }
        Self { by_id, children }
    }

    /// The group with `id`, if it exists.
    pub fn get(&self, id: Id<Group>) -> Option<&'a Group> {
        self.by_id.get(&id).copied()
    }

    /// The groups directly inside `parent` (`None` for the top level), sorted by name.
    pub fn children(&self, parent: Option<Id<Group>>) -> &[&'a Group] {
        self.children.get(&parent).map_or(&[], Vec::as_slice)
    }

    /// `group` and the groups it's inside, nearest first: the order settings are inherited in.
    /// Empty for `None` or an unknown id.
    pub fn ancestors(&self, group: Option<Id<Group>>) -> Vec<&'a Group> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut next = group;
        while let Some(group) = next.and_then(|id| self.get(id)) {
            if !seen.insert(group.id) {
                break;
            }
            chain.push(group);
            next = group.parent;
        }
        chain
    }

    /// The group's names from the top down, joined like `Production / Databases`.
    pub fn path(&self, group: Id<Group>) -> String {
        let mut names: Vec<&str> = self
            .ancestors(Some(group))
            .iter()
            .map(|g| g.name.as_str())
            .collect();
        names.reverse();
        names.join(" / ")
    }

    /// Whether `group` is `outer` or somewhere inside it.
    pub fn is_within(&self, group: Option<Id<Group>>, outer: Id<Group>) -> bool {
        self.ancestors(group).iter().any(|g| g.id == outer)
    }

    /// `group` and every group inside it, parents before children.
    pub fn descendants(&self, group: Id<Group>) -> Vec<Id<Group>> {
        let mut out = Vec::new();
        let mut stack = vec![group];
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            out.push(id);
            stack.extend(self.children(Some(id)).iter().rev().map(|g| g.id));
        }
        out
    }

    /// Checks `group` may be moved inside `parent`.
    ///
    /// # Errors
    ///
    /// [`GroupError::UnknownParent`] if `parent` doesn't exist, [`GroupError::Cycle`] if it is
    /// `group` or inside it.
    pub fn check_parent(
        &self,
        group: Id<Group>,
        parent: Option<Id<Group>>,
    ) -> Result<(), GroupError> {
        let Some(parent) = parent else {
            return Ok(());
        };
        if self.get(parent).is_none() {
            return Err(GroupError::UnknownParent);
        }
        if self.is_within(Some(parent), group) {
            return Err(GroupError::Cycle);
        }
        Ok(())
    }

    /// The sidebar's lines: groups depth-first in name order, skipping the inside of groups for
    /// which `is_expanded` says no.
    pub fn rows(&self, is_expanded: impl Fn(Id<Group>) -> bool) -> Vec<GroupRow<'a>> {
        let mut rows = Vec::new();
        let mut stack: Vec<(&'a Group, usize)> =
            self.children(None).iter().rev().map(|g| (*g, 0)).collect();
        while let Some((group, depth)) = stack.pop() {
            let inside = self.children(Some(group.id));
            rows.push(GroupRow {
                group,
                depth,
                has_children: !inside.is_empty(),
            });
            if is_expanded(group.id) {
                stack.extend(inside.iter().rev().map(|g| (*g, depth + 1)));
            }
        }
        rows
    }
}

/// Whether following `group`'s parents leads back to it.
fn loops(group: &Group, by_id: &HashMap<Id<Group>, &Group>) -> bool {
    let mut seen = HashSet::new();
    let mut next = group.parent;
    while let Some(id) = next {
        if id == group.id {
            return true;
        }
        if !seen.insert(id) {
            // A loop further up that doesn't include this group; the groups in it are
            // detached instead, which makes this one's chain finite.
            return false;
        }
        next = by_id.get(&id).and_then(|g| g.parent);
    }
    false
}

fn sort_key(group: &Group) -> (String, Id<Group>) {
    (group.name.to_lowercase(), group.id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn group(name: &str, parent: Option<&Group>) -> Group {
        Group::new(name, parent.map(|p| p.id), Timestamp::UNIX_EPOCH)
    }

    /// prod ─┬─ db ── replicas
    ///       └─ web
    /// staging
    fn sample() -> Vec<Group> {
        let prod = group("prod", None);
        let db = group("db", Some(&prod));
        let replicas = group("replicas", Some(&db));
        let web = group("web", Some(&prod));
        let staging = group("Staging", None);
        vec![web, replicas, staging, db, prod]
    }

    fn names<'a>(groups: impl IntoIterator<Item = &'a Group>) -> Vec<&'a str> {
        groups.into_iter().map(|g| g.name.as_str()).collect()
    }

    fn find<'a>(groups: &'a [Group], name: &str) -> &'a Group {
        groups.iter().find(|g| g.name == name).unwrap()
    }

    #[test]
    fn validation() {
        let named = |name: &str| group(name, None);
        let cases = [
            (named("Production"), Ok(())),
            (named(""), Err(GroupError::EmptyName)),
            (named("  "), Err(GroupError::EmptyName)),
            (named(&"x".repeat(121)), Err(GroupError::NameTooLong)),
            (named("a\tb"), Err(GroupError::NameControl)),
        ];
        for (g, expected) in cases {
            assert_eq!(g.validate(), expected, "{:?}", g.name);
        }
    }

    #[test]
    fn children_are_sorted_by_name_ignoring_case() {
        let groups = sample();
        let tree = GroupTree::new(&groups);
        assert_eq!(
            names(tree.children(None).iter().copied()),
            ["prod", "Staging"]
        );
        let prod = find(&groups, "prod");
        assert_eq!(
            names(tree.children(Some(prod.id)).iter().copied()),
            ["db", "web"]
        );
    }

    #[test]
    fn ancestors_and_paths() {
        let groups = sample();
        let tree = GroupTree::new(&groups);
        let replicas = find(&groups, "replicas");
        assert_eq!(
            names(tree.ancestors(Some(replicas.id))),
            ["replicas", "db", "prod"]
        );
        assert_eq!(tree.path(replicas.id), "prod / db / replicas");
        assert_eq!(tree.ancestors(None).len(), 0);
        assert_eq!(tree.ancestors(Some(Id::generate())).len(), 0);
        assert!(tree.is_within(Some(replicas.id), find(&groups, "prod").id));
        assert!(!tree.is_within(Some(replicas.id), find(&groups, "web").id));
    }

    #[test]
    fn descendants_include_the_group_itself_parents_first() {
        let groups = sample();
        let tree = GroupTree::new(&groups);
        let ids = tree.descendants(find(&groups, "prod").id);
        let found: Vec<&str> = ids
            .iter()
            .map(|id| tree.get(*id).unwrap().name.as_str())
            .collect();
        assert_eq!(found, ["prod", "db", "replicas", "web"]);
    }

    #[test]
    fn moving_a_group_inside_itself_is_refused() {
        let groups = sample();
        let tree = GroupTree::new(&groups);
        let prod = find(&groups, "prod").id;
        let replicas = find(&groups, "replicas").id;
        let staging = find(&groups, "Staging").id;
        let cases = [
            (prod, None, Ok(())),
            (replicas, Some(staging), Ok(())),
            (prod, Some(prod), Err(GroupError::Cycle)),
            (prod, Some(replicas), Err(GroupError::Cycle)),
            (prod, Some(Id::generate()), Err(GroupError::UnknownParent)),
        ];
        for (group, parent, expected) in cases {
            assert_eq!(tree.check_parent(group, parent), expected);
        }
    }

    #[test]
    fn rows_follow_expansion() {
        let groups = sample();
        let tree = GroupTree::new(&groups);
        let all: Vec<(&str, usize, bool)> = tree
            .rows(|_| true)
            .iter()
            .map(|r| (r.group.name.as_str(), r.depth, r.has_children))
            .collect();
        assert_eq!(
            all,
            [
                ("prod", 0, true),
                ("db", 1, true),
                ("replicas", 2, false),
                ("web", 1, false),
                ("Staging", 0, false),
            ]
        );
        let db = find(&groups, "db").id;
        let collapsed_db = tree.rows(|id| id != db);
        assert_eq!(
            names(collapsed_db.iter().map(|r| r.group)),
            ["prod", "db", "web", "Staging"]
        );
        assert_eq!(tree.rows(|_| false).len(), 2);
    }

    #[test]
    fn bad_data_does_not_loop_or_lose_groups() {
        let mut a = group("a", None);
        let mut b = group("b", None);
        a.parent = Some(b.id);
        b.parent = Some(a.id);
        let mut selfish = group("self", None);
        selfish.parent = Some(selfish.id);
        let orphan = Group {
            parent: Some(Id::generate()),
            ..group("orphan", None)
        };
        let groups = vec![a, b, selfish, orphan];
        let tree = GroupTree::new(&groups);

        let shown = tree.rows(|_| true);
        let mut shown: Vec<&str> = names(shown.iter().map(|r| r.group));
        shown.sort_unstable();
        assert_eq!(shown, ["a", "b", "orphan", "self"]);
        assert_eq!(tree.ancestors(Some(groups[0].id)).len(), 2);
        assert_eq!(tree.descendants(groups[2].id).len(), 1);
    }
}
