//! Filtering, sorting and ranking hosts (HOST-03…07, CONN-23).
//!
//! One matcher serves both the host list's search box and the quick connect bar, so a host that
//! turns up for some text in one turns up in the other. Every whitespace-separated term must
//! match somewhere: the label (also fuzzily, so `pdb` finds `prod-db`), the address, the user
//! name, a tag or the notes. Matches in the label count most.

use std::cmp::{Ordering, Reverse};

use serde::{Deserialize, Serialize};

use super::{Group, GroupTree, Host, Tag};
use crate::fuzzy;
use crate::id::Id;

/// How the host list is sorted (HOST-05).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// By title, A to Z, ignoring case.
    #[default]
    Name,
    /// Most recently connected first; never-connected hosts last, by name.
    LastUsed,
    /// Newest first.
    Created,
}

impl SortOrder {
    /// Every order, as a menu lists them.
    pub const ALL: [Self; 3] = [Self::Name, Self::LastUsed, Self::Created];

    /// The menu label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::LastUsed => "Last used",
            Self::Created => "Date created",
        }
    }
}

/// Which hosts the list shows before the search text is applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Every host.
    #[default]
    All,
    /// Favourites only (HOST-06).
    Favourites,
    /// Hosts connected to at least once, most recent first, whatever the sort order (HOST-07).
    Recent,
    /// Hosts in this group or any group inside it.
    Group(Id<Group>),
}

/// What the host list is asked to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostQuery {
    /// Search text; empty shows everything in scope.
    pub text: String,
    /// Only hosts with this tag (HOST-03).
    pub tag: Option<Tag>,
    /// Which part of the list.
    pub scope: Scope,
}

/// The hosts `query` selects, in `order`.
pub fn filter<'a>(
    hosts: &'a [Host],
    query: &HostQuery,
    groups: &GroupTree<'_>,
    order: SortOrder,
) -> Vec<&'a Host> {
    let terms = terms(&query.text);
    let mut found: Vec<&Host> = hosts
        .iter()
        .filter(|host| in_scope(host, query.scope, groups))
        .filter(|host| query.tag.as_ref().is_none_or(|tag| host.tags.contains(tag)))
        .filter(|host| score(host, &terms).is_some())
        .collect();
    let order = if query.scope == Scope::Recent {
        SortOrder::LastUsed
    } else {
        order
    };
    sort(&mut found, order);
    found
}

/// The hosts matching `text`, best match first, for the quick connect bar. Empty text matches
/// nothing; ties go to the most recently used host.
pub fn rank<'a>(hosts: &'a [Host], text: &str) -> Vec<&'a Host> {
    let terms = terms(text);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u32, &Host)> = hosts
        .iter()
        .filter_map(|host| score(host, &terms).map(|s| (s, host)))
        .collect();
    scored.sort_by(|(a_score, a), (b_score, b)| {
        b_score
            .cmp(a_score)
            .then_with(|| compare(a, b, SortOrder::LastUsed))
    });
    scored.into_iter().map(|(_, host)| host).collect()
}

/// Up to `limit` hosts connected to most recently.
pub fn recent(hosts: &[Host], limit: usize) -> Vec<&Host> {
    let mut used: Vec<&Host> = hosts
        .iter()
        .filter(|h| h.last_connected_at.is_some())
        .collect();
    sort(&mut used, SortOrder::LastUsed);
    used.truncate(limit);
    used
}

/// Sorts `hosts` in place.
pub fn sort(hosts: &mut [&Host], order: SortOrder) {
    hosts.sort_by(|a, b| compare(a, b, order));
}

fn compare(a: &Host, b: &Host, order: SortOrder) -> Ordering {
    let by_name = || {
        a.title()
            .to_lowercase()
            .cmp(&b.title().to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    };
    match order {
        SortOrder::Name => by_name(),
        // `Reverse(None)` sorts after every `Reverse(Some(_))`, so unused hosts go last.
        SortOrder::LastUsed => Reverse(a.last_connected_at)
            .cmp(&Reverse(b.last_connected_at))
            .then_with(by_name),
        SortOrder::Created => b.created_at.cmp(&a.created_at).then_with(by_name),
    }
}

fn in_scope(host: &Host, scope: Scope, groups: &GroupTree<'_>) -> bool {
    match scope {
        Scope::All => true,
        Scope::Favourites => host.is_favourite,
        Scope::Recent => host.last_connected_at.is_some(),
        Scope::Group(group) => groups.is_within(host.group, group),
    }
}

fn terms(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_lowercase).collect()
}

/// How well `host` matches every term, or `None` if some term matches nothing.
fn score(host: &Host, terms: &[String]) -> Option<u32> {
    if terms.is_empty() {
        return Some(0);
    }
    let label = host.title().to_lowercase();
    let address = host.address.to_string();
    let user = host
        .settings
        .username
        .as_deref()
        .unwrap_or("")
        .to_lowercase();
    let notes = host.notes.to_lowercase();
    terms.iter().try_fold(0, |total, term| {
        let best = [
            text_score(&label, term, 100, 80, 60).or_else(|| fuzzy::score(&label, term)),
            text_score(&address, term, 70, 50, 50),
            text_score(&user, term, 40, 30, 30),
            host.tags
                .iter()
                .filter_map(|tag| {
                    if tag.as_str() == term {
                        Some(60)
                    } else {
                        text_score(tag.as_str(), term, 45, 30, 30)
                    }
                })
                .max(),
            notes.contains(term.as_str()).then_some(10),
        ]
        .into_iter()
        .flatten()
        .max()?;
        Some(total + best)
    })
}

/// `prefix` if `haystack` starts with `term`, `word` if a word in it does, `inner` if it
/// appears anywhere.
fn text_score(haystack: &str, term: &str, prefix: u32, word: u32, inner: u32) -> Option<u32> {
    if haystack.starts_with(term) {
        return Some(prefix);
    }
    let mut found = None;
    for (at, _) in haystack.match_indices(term) {
        let at_word = haystack
            .get(..at)
            .and_then(|before| before.chars().next_back())
            .is_some_and(|c| !c.is_alphanumeric());
        if at_word {
            return Some(word);
        }
        found = Some(inner);
    }
    found
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::time::Instant;

    use jiff::Timestamp;
    use pretty_assertions::assert_eq;

    use super::*;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    fn host(label: &str, address: &str) -> Host {
        let mut h = Host::new(address.parse().unwrap(), at(0));
        h.label = label.into();
        h
    }

    fn titles(hosts: &[&Host]) -> Vec<String> {
        hosts.iter().map(|h| h.title().into_owned()).collect()
    }

    fn sample() -> Vec<Host> {
        let mut web = host("Web 1", "web1.example.com");
        web.tags = Tag::parse_list("prod web").unwrap();
        web.created_at = at(30);
        web.last_connected_at = Some(at(100));
        let mut db = host("prod-db", "10.0.0.5");
        db.tags = Tag::parse_list("prod database").unwrap();
        db.settings.username = Some("postgres".into());
        db.created_at = at(20);
        db.last_connected_at = Some(at(200));
        db.is_favourite = true;
        let mut staging = host("", "staging.internal");
        staging.notes = "Ask Sam before rebooting".into();
        staging.created_at = at(10);
        vec![web, db, staging]
    }

    #[test]
    fn search_matches_each_field() {
        let hosts = sample();
        let tree = GroupTree::new(&[]);
        let cases: [(&str, &[&str]); 12] = [
            ("", &["prod-db", "staging.internal", "Web 1"]),
            ("web", &["Web 1"]),
            ("WEB", &["Web 1"]),
            ("10.0", &["prod-db"]),
            ("example.com", &["Web 1"]),
            ("postgres", &["prod-db"]),
            ("database", &["prod-db"]),
            ("prod", &["prod-db", "Web 1"]),
            ("sam", &["staging.internal"]),
            ("pdb", &["prod-db"]),
            ("prod web", &["Web 1"]),
            ("nothing-like-this", &[]),
        ];
        for (text, expected) in cases {
            let query = HostQuery {
                text: text.into(),
                ..HostQuery::default()
            };
            assert_eq!(
                titles(&filter(&hosts, &query, &tree, SortOrder::Name)),
                expected,
                "{text:?}"
            );
        }
    }

    #[test]
    fn tag_and_scope_filters() {
        let hosts = sample();
        let tree = GroupTree::new(&[]);
        let run = |query: HostQuery| titles(&filter(&hosts, &query, &tree, SortOrder::Name));
        assert_eq!(
            run(HostQuery {
                tag: Some("web".parse().unwrap()),
                ..HostQuery::default()
            }),
            ["Web 1"]
        );
        assert_eq!(
            run(HostQuery {
                scope: Scope::Favourites,
                ..HostQuery::default()
            }),
            ["prod-db"]
        );
        // Recent ignores the requested order: most recent first.
        assert_eq!(
            run(HostQuery {
                scope: Scope::Recent,
                ..HostQuery::default()
            }),
            ["prod-db", "Web 1"]
        );
    }

    #[test]
    fn group_scope_includes_subgroups() {
        let outer = Group::new("outer", None, at(0));
        let inner = Group::new("inner", Some(outer.id), at(0));
        let other = Group::new("other", None, at(0));
        let mut hosts = sample();
        hosts[0].group = Some(inner.id);
        hosts[1].group = Some(outer.id);
        hosts[2].group = Some(other.id);
        let groups = vec![outer.clone(), inner.clone(), other];
        let tree = GroupTree::new(&groups);
        let in_group = |id| {
            let query = HostQuery {
                scope: Scope::Group(id),
                ..HostQuery::default()
            };
            titles(&filter(&hosts, &query, &tree, SortOrder::Name))
        };
        assert_eq!(in_group(outer.id), ["prod-db", "Web 1"]);
        assert_eq!(in_group(inner.id), ["Web 1"]);
    }

    #[test]
    fn sort_orders() {
        let hosts = sample();
        let tree = GroupTree::new(&[]);
        let sorted = |order| titles(&filter(&hosts, &HostQuery::default(), &tree, order));
        assert_eq!(
            sorted(SortOrder::Name),
            ["prod-db", "staging.internal", "Web 1"]
        );
        assert_eq!(
            sorted(SortOrder::LastUsed),
            ["prod-db", "Web 1", "staging.internal"]
        );
        assert_eq!(
            sorted(SortOrder::Created),
            ["Web 1", "prod-db", "staging.internal"]
        );
    }

    #[test]
    fn ranking_prefers_label_prefixes_then_recency() {
        let mut hosts = sample();
        hosts.push(host("Webmail", "mail.example.com"));
        assert_eq!(titles(&rank(&hosts, "web")), ["Web 1", "Webmail"]);
        // Both match "prod" as a tag or label prefix; prod-db's label prefix wins.
        assert_eq!(titles(&rank(&hosts, "prod")), ["prod-db", "Web 1"]);
        assert_eq!(rank(&hosts, "  ").len(), 0);
        // A tie on score goes to the more recently used host.
        let mut a = host("alpha", "a.example.com");
        let mut b = host("alpine", "b.example.com");
        a.last_connected_at = Some(at(1));
        b.last_connected_at = Some(at(2));
        assert_eq!(titles(&rank(&[a, b], "alp")), ["alpine", "alpha"]);
    }

    #[test]
    fn recent_hosts_newest_first() {
        let hosts = sample();
        assert_eq!(titles(&recent(&hosts, 10)), ["prod-db", "Web 1"]);
        assert_eq!(titles(&recent(&hosts, 1)), ["prod-db"]);
    }

    #[test]
    fn text_and_fuzzy_scores() {
        assert_eq!(text_score("prod-db", "prod", 3, 2, 1), Some(3));
        assert_eq!(text_score("prod-db", "db", 3, 2, 1), Some(2));
        assert_eq!(text_score("proddb", "db", 3, 2, 1), Some(1));
        assert_eq!(text_score("prod", "x", 3, 2, 1), None);
    }

    #[test]
    fn searching_fifty_hosts_takes_well_under_five_milliseconds() {
        let hosts: Vec<Host> = (0..50)
            .map(|i| {
                let mut h = host(
                    &format!("server-{i}"),
                    &format!("10.0.{}.{}", i / 10, i % 10),
                );
                h.tags = Tag::parse_list("prod, eu-west").unwrap();
                h.notes = "Some notes about this machine and who owns it".into();
                h
            })
            .collect();
        let tree = GroupTree::new(&[]);
        let query = HostQuery {
            text: "srv 4".into(),
            ..HostQuery::default()
        };
        let start = Instant::now();
        for _ in 0..10 {
            let _ = filter(&hosts, &query, &tree, SortOrder::Name);
        }
        // Ten searches, so one search must be well under the 5 ms the phase promises, even in
        // an unoptimised test build on a slow CI machine.
        assert!(start.elapsed().as_millis() < 50, "{:?}", start.elapsed());
    }
}
