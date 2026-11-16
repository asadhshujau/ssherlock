//! Working out the settings a host ends up with: its own, else its nearest group's, else the
//! default (HOST-02).

use super::{Colour, Group, Host, HostSettings};
use crate::address::Address;
use crate::algorithms::{AlgorithmPreferences, Compression};
use crate::id::Id;
use crate::port::Port;

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
}

impl EffectiveSettings {
    /// Resolves `layers`, nearest first: the first layer that sets a field wins.
    pub fn from_layers<'a>(
        layers: impl IntoIterator<Item = (Source, &'a HostSettings)> + Clone,
    ) -> Self {
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
            compression: pick(layers, |s| s.compression, Compression::Off),
        }
    }

    /// What something inside `ancestors` (nearest first) inherits before its own settings: the
    /// placeholders an editor shows for fields left unset.
    pub fn inherited(ancestors: &[&Group]) -> Self {
        Self::from_layers(ancestors.iter().map(|g| (Source::Group(g.id), &g.settings)))
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
/// [`GroupTree::ancestors`](super::GroupTree::ancestors) returns them).
pub fn resolve(host: &Host, ancestors: &[&Group]) -> EffectiveHost {
    let layers = std::iter::once((Source::Own, &host.settings))
        .chain(ancestors.iter().map(|g| (Source::Group(g.id), &g.settings)));
    EffectiveHost {
        id: host.id,
        title: host.title().into_owned(),
        address: host.address.clone(),
        settings: EffectiveSettings::from_layers(layers),
    }
}

/// The first layer's value for `field`, or `default`.
fn pick<'a, T>(
    layers: impl IntoIterator<Item = (Source, &'a HostSettings)>,
    field: impl Fn(&HostSettings) -> Option<T>,
    default: T,
) -> Resolved<T> {
    layers
        .into_iter()
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

        let effective = resolve(&host, &tree.ancestors(host.group));
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

            let resolved = resolve(&host, &[&inner_group, &outer_group]).settings.port;
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
        let s = resolve(&host, &[]).settings;
        assert_eq!(s.port.value, Port::SSH);
        assert_eq!(s.username.value, None);
        assert_eq!(s.colour.value, None);
        assert_eq!(s.compression.value, Compression::Off);
        assert!(s.algorithms().is_empty());
        assert_eq!(s.port.source, Source::Default);
    }

    #[test]
    fn inherited_skips_the_own_layer() {
        let now = Timestamp::UNIX_EPOCH;
        let mut group = Group::new("g", None, now);
        group.settings.username = Some("deploy".into());
        let inherited = EffectiveSettings::inherited(&[&group]);
        assert_eq!(inherited.username.value.as_deref(), Some("deploy"));
        assert_eq!(inherited.username.source, Source::Group(group.id));
        assert_eq!(inherited.port.source, Source::Default);
    }
}
