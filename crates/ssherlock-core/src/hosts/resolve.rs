//! Working out the settings a host ends up with: its own, else its nearest group's, else the
//! default (HOST-02).
//!
//! Credentials resolve the same way once each layer's identity is unpacked: a layer that picks
//! an [`Identity`] counts as setting that identity's user name, key and password, except where
//! the layer sets one itself. So a host's own identity beats a group's user name, and a host's
//! own user name beats its identity's.

use std::borrow::Cow;

use super::{Colour, Group, Host, HostSettings};
use crate::address::Address;
use crate::algorithms::{AlgorithmPreferences, Compression};
use crate::id::Id;
use crate::keychain::{Identity, Key};
use crate::port::Port;
use crate::secret::Sealed;

/// Where a resolved setting came from, so an editor can say "inherited from Production".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The host's (or, when resolving a group, that group's) own setting.
    Own,
    /// Inherited from this group.
    Group(Id<Group>),
    /// Nobody set it; SSHerlock's default.
    Default,
}

/// A setting's value and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved<T> {
    /// The value in effect.
    pub value: T,
    /// Who set it.
    pub source: Source,
}

/// Every setting in [`HostSettings`], resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveSettings {
    /// The port; default 22.
    pub port: Resolved<Port>,
    /// The user name; `None` means the local user's, filled in when connecting.
    pub username: Resolved<Option<String>>,
    /// The colour mark; default none.
    pub colour: Resolved<Option<Colour>>,
    /// Key exchange algorithms; empty means SSHerlock's defaults.
    pub kex: Resolved<Vec<String>>,
    /// Host key algorithms; empty means the defaults.
    pub host_key: Resolved<Vec<String>>,
    /// Ciphers; empty means the defaults.
    pub cipher: Resolved<Vec<String>>,
    /// MACs; empty means the defaults.
    pub mac: Resolved<Vec<String>>,
    /// Compression; default off.
    pub compression: Resolved<Compression>,
    /// The identity picked; default none.
    pub identity: Resolved<Option<Id<Identity>>>,
    /// The saved key to log in with, from a layer or its identity; default none.
    pub key: Resolved<Option<Id<Key>>>,
    /// The saved password, from a layer or its identity; default none (ask).
    pub password: Resolved<Option<Sealed>>,
}

impl EffectiveSettings {
    /// Resolves `layers`, nearest first: the first layer that sets a field wins. Identities the
    /// layers pick are looked up in `identities`; one that isn't there (deleted) adds nothing.
    pub fn from_layers<'a>(
        layers: impl IntoIterator<Item = (Source, &'a HostSettings)>,
        identities: &[Identity],
    ) -> Self {
        let expanded: Vec<(Source, Cow<'a, HostSettings>)> = layers
            .into_iter()
            .map(|(source, settings)| (source, with_identity(settings, identities)))
            .collect();
        let layers = expanded.iter().map(|(source, s)| (*source, s.as_ref()));
        Self {
            port: pick(layers.clone(), |s| s.port, Port::SSH),
            username: pick(layers.clone(), |s| s.username.clone().map(Some), None),
            colour: pick(layers.clone(), |s| s.colour.map(Some), None),
            kex: pick(layers.clone(), |s| non_empty(&s.algorithms.kex), Vec::new()),
            host_key: pick(
                layers.clone(),
                |s| non_empty(&s.algorithms.host_key),
                Vec::new(),
            ),
            cipher: pick(
                layers.clone(),
                |s| non_empty(&s.algorithms.cipher),
                Vec::new(),
            ),
            mac: pick(layers.clone(), |s| non_empty(&s.algorithms.mac), Vec::new()),
            compression: pick(layers.clone(), |s| s.compression, Compression::Off),
            identity: pick(layers.clone(), |s| s.identity.map(Some), None),
            key: pick(layers.clone(), |s| s.key.map(Some), None),
            password: pick(layers, |s| s.password.clone().map(Some), None),
        }
    }

    /// What something inside `ancestors` (nearest first) inherits before its own settings: the
    /// placeholders an editor shows for fields left unset.
    pub fn inherited(ancestors: &[&Group], identities: &[Identity]) -> Self {
        Self::from_layers(
            ancestors.iter().map(|g| (Source::Group(g.id), &g.settings)),
            identities,
        )
    }

    /// The algorithm lists as the SSH layer takes them.
    pub fn algorithms(&self) -> AlgorithmPreferences {
        AlgorithmPreferences {
            kex: self.kex.value.clone(),
            host_key: self.host_key.value.clone(),
            cipher: self.cipher.value.clone(),
            mac: self.mac.value.clone(),
        }
    }
}

/// A host with everything resolved: what connecting to it uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveHost {
    /// The saved host's id.
    pub id: Id<Host>,
    /// The name to show ([`Host::title`]).
    pub title: String,
    /// Where to connect.
    pub address: Address,
    /// The resolved settings.
    pub settings: EffectiveSettings,
}

/// Resolves `host` inside `ancestors`, its groups nearest first (as
/// [`GroupTree::ancestors`](super::GroupTree::ancestors) returns them), unpacking the
/// identities they pick from `identities`.
pub fn resolve(host: &Host, ancestors: &[&Group], identities: &[Identity]) -> EffectiveHost {
    let layers = std::iter::once((Source::Own, &host.settings))
        .chain(ancestors.iter().map(|g| (Source::Group(g.id), &g.settings)));
    EffectiveHost {
        id: host.id,
        title: host.title().into_owned(),
        address: host.address.clone(),
        settings: EffectiveSettings::from_layers(layers, identities),
    }
}

/// `settings` with its identity's user name, key and password filling the ones it leaves unset.
fn with_identity<'a>(settings: &'a HostSettings, identities: &[Identity]) -> Cow<'a, HostSettings> {
    let Some(identity) = settings
        .identity
        .and_then(|id| identities.iter().find(|i| i.id == id))
    else {
        return Cow::Borrowed(settings);
    };
    let mut filled = settings.clone();
    if filled.username.is_none() {
        filled.username = Some(identity.username.clone());
    }
    if filled.key.is_none() {
        filled.key = identity.key;
    }
    if filled.password.is_none() {
        filled.password.clone_from(&identity.password);
    }
    Cow::Owned(filled)
}

/// The first layer's value for `field`, or `default`.
fn pick<'a, T>(
    mut layers: impl Iterator<Item = (Source, &'a HostSettings)>,
    field: impl Fn(&HostSettings) -> Option<T>,
    default: T,
) -> Resolved<T> {
    layers
        .find_map(|(source, settings)| field(settings).map(|value| Resolved { value, source }))
        .unwrap_or(Resolved {
            value: default,
            source: Source::Default,
        })
}

fn non_empty(list: &[String]) -> Option<Vec<String>> {
    (!list.is_empty()).then(|| list.to_vec())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use jiff::Timestamp;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::hosts::GroupTree;

    fn port(n: u16) -> Option<Port> {
        Port::new(n)
    }

    #[test]
    fn nearest_setting_wins_field_by_field() {
        let now = Timestamp::UNIX_EPOCH;
        let mut prod = Group::new("prod", None, now);
        prod.settings = HostSettings {
            port: port(2200),
            username: Some("ops".into()),
            colour: Some(Colour::Red),
            compression: Some(Compression::On),
            ..HostSettings::default()
        };
        let mut db = Group::new("db", Some(prod.id), now);
        db.settings = HostSettings {
            username: Some("postgres".into()),
            algorithms: AlgorithmPreferences {
                cipher: vec!["aes256-gcm@openssh.com".into()],
                ..AlgorithmPreferences::default()
            },
            ..HostSettings::default()
        };
        let mut host = Host::new("db1.example.com".parse().unwrap(), now);
        host.group = Some(db.id);
        host.settings.port = port(2222);
        let groups = vec![prod.clone(), db.clone()];
        let tree = GroupTree::new(&groups);

        let effective = resolve(&host, &tree.ancestors(host.group), &[]);
        let s = &effective.settings;

        let own = |v| Resolved {
            value: v,
            source: Source::Own,
        };
        assert_eq!(s.port, own(Port::new(2222).unwrap()));
        assert_eq!(
            s.username,
            Resolved {
                value: Some("postgres".into()),
                source: Source::Group(db.id)
            }
        );
        assert_eq!(
            s.colour,
            Resolved {
                value: Some(Colour::Red),
                source: Source::Group(prod.id)
            }
        );
        assert_eq!(
            s.compression,
            Resolved {
                value: Compression::On,
                source: Source::Group(prod.id)
            }
        );
        assert_eq!(s.cipher.value, ["aes256-gcm@openssh.com"]);
        assert_eq!(s.cipher.source, Source::Group(db.id));
        assert_eq!(
            s.kex,
            Resolved {
                value: Vec::new(),
                source: Source::Default
            }
        );
        assert_eq!(s.algorithms().cipher, ["aes256-gcm@openssh.com"]);
        assert_eq!(effective.title, "db1.example.com");
    }

    #[test]
    fn table_of_inheritance_cases() {
        // (host port, inner group port, outer group port) -> (port, source index)
        // where source index 0 = own, 1 = inner, 2 = outer, 3 = default.
        let cases = [
            ((Some(1), Some(2), Some(3)), (1, 0)),
            ((None, Some(2), Some(3)), (2, 1)),
            ((None, None, Some(3)), (3, 2)),
            ((None, None, None), (22, 3)),
            ((Some(1), None, None), (1, 0)),
            ((None, Some(2), None), (2, 1)),
        ];
        let now = Timestamp::UNIX_EPOCH;
        for ((h, inner, outer), (want_port, want_source)) in cases {
            let mut outer_group = Group::new("outer", None, now);
            outer_group.settings.port = outer.and_then(port);
            let mut inner_group = Group::new("inner", Some(outer_group.id), now);
            inner_group.settings.port = inner.and_then(port);
            let mut host = Host::new("h".parse().unwrap(), now);
            host.settings.port = h.and_then(port);

            let resolved = resolve(&host, &[&inner_group, &outer_group], &[])
                .settings
                .port;
            let sources = [
                Source::Own,
                Source::Group(inner_group.id),
                Source::Group(outer_group.id),
                Source::Default,
            ];
            assert_eq!(resolved.value.get(), want_port, "{h:?} {inner:?} {outer:?}");
            assert_eq!(Some(&resolved.source), sources.get(want_source));
        }
    }

    #[test]
    fn a_host_without_groups_gets_defaults() {
        let host = Host::new("example.com".parse().unwrap(), Timestamp::UNIX_EPOCH);
        let s = resolve(&host, &[], &[]).settings;
        assert_eq!(s.port.value, Port::SSH);
        assert_eq!(s.username.value, None);
        assert_eq!(s.colour.value, None);
        assert_eq!(s.compression.value, Compression::Off);
        assert!(s.algorithms().is_empty());
        assert_eq!(s.port.source, Source::Default);
    }

    /// What the host's layer sets, the group's, and the user name and key that should win.
    type Case<'a> = (
        &'a [&'a str],
        &'a [&'a str],
        Option<&'a str>,
        Option<Id<Key>>,
    );

    /// (host sets, group sets) -> which user name and key win. "id" means the layer picks an
    /// identity (user `alice`, key A); "user" means it sets the user name `bob` itself; "key"
    /// means it sets key B itself.
    #[test]
    fn identities_fill_their_own_layer_only() {
        let now = Timestamp::UNIX_EPOCH;
        let key_a: Id<Key> = Id::generate();
        let key_b: Id<Key> = Id::generate();
        let mut alice = Identity::new("alice", now);
        alice.key = Some(key_a);
        alice.password = Some(Sealed::from_bytes(vec![1]));
        let identities = [alice.clone()];

        let layer = |parts: &[&str]| HostSettings {
            identity: parts.contains(&"id").then_some(alice.id),
            username: parts.contains(&"user").then(|| "bob".to_owned()),
            key: parts.contains(&"key").then_some(key_b),
            ..HostSettings::default()
        };
        let cases: [Case<'_>; 7] = [
            (&[], &[], None, None),
            (&["id"], &[], Some("alice"), Some(key_a)),
            (&[], &["id"], Some("alice"), Some(key_a)),
            (&["id"], &["user", "key"], Some("alice"), Some(key_a)),
            (&["user"], &["id"], Some("bob"), Some(key_a)),
            (&["id", "user"], &[], Some("bob"), Some(key_a)),
            (&["key"], &["id"], Some("alice"), Some(key_b)),
        ];
        for (host_parts, group_parts, user, key) in cases {
            let mut group = Group::new("g", None, now);
            group.settings = layer(group_parts);
            let mut host = Host::new("h".parse().unwrap(), now);
            host.settings = layer(host_parts);
            let s = resolve(&host, &[&group], &identities).settings;
            assert_eq!(
                s.username.value.as_deref(),
                user,
                "{host_parts:?} {group_parts:?}"
            );
            assert_eq!(s.key.value, key, "{host_parts:?} {group_parts:?}");
        }

        let mut host = Host::new("h".parse().unwrap(), now);
        host.settings.identity = Some(alice.id);
        let s = resolve(&host, &[], &identities).settings;
        assert_eq!(s.password.value, alice.password);
        assert_eq!(s.password.source, Source::Own);
        assert_eq!(s.identity.value, Some(alice.id));
        // A deleted identity adds nothing.
        let s = resolve(&host, &[], &[]).settings;
        assert_eq!((s.username.value, s.key.value), (None, None));
    }

    #[test]
    fn inherited_skips_the_own_layer() {
        let now = Timestamp::UNIX_EPOCH;
        let mut group = Group::new("g", None, now);
        group.settings.username = Some("deploy".into());
        let inherited = EffectiveSettings::inherited(&[&group], &[]);
        assert_eq!(inherited.username.value.as_deref(), Some("deploy"));
        assert_eq!(inherited.username.source, Source::Group(group.id));
        assert_eq!(inherited.port.source, Source::Default);
    }
}
