//! Which hosts, groups and identities use a key or identity (KEY-14).

use super::{Identity, Key};
use crate::hosts::{Group, GroupTree, Host, resolve};
use crate::id::Id;

/// What uses a key or identity, each list in the order given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Hosts that log in with it, directly or inherited from a group or identity.
    pub hosts: Vec<Id<Host>>,
    /// Groups that pick it in their own settings (directly or through their identity).
    pub groups: Vec<Id<Group>>,
    /// Identities that hold it (for keys).
    pub identities: Vec<Id<Identity>>,
}

impl Usage {
    /// Whether nothing uses it.
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty() && self.groups.is_empty() && self.identities.is_empty()
    }
}

/// What uses `key`. A host counts if the key it would log in with, after inheritance, is
/// `key`.
pub fn usage_of_key(
    key: Id<Key>,
    hosts: &[Host],
    groups: &[Group],
    identities: &[Identity],
) -> Usage {
    let tree = GroupTree::new(groups);
    let identity_has_key = |id: Option<Id<Identity>>| {
        identities
            .iter()
            .any(|i| Some(i.id) == id && i.key == Some(key))
    };
    Usage {
        hosts: hosts
            .iter()
            .filter(|h| {
                resolve(h, &tree.ancestors(h.group), identities)
                    .settings
                    .key
                    .value
                    == Some(key)
            })
            .map(|h| h.id)
            .collect(),
        groups: groups
            .iter()
            .filter(|g| g.settings.key == Some(key) || identity_has_key(g.settings.identity))
            .map(|g| g.id)
            .collect(),
        identities: identities
            .iter()
            .filter(|i| i.key == Some(key))
            .map(|i| i.id)
            .collect(),
    }
}

/// What uses `identity`. A host counts if the identity it ends up with, after inheritance, is
/// `identity`.
pub fn usage_of_identity(
    identity: Id<Identity>,
    hosts: &[Host],
    groups: &[Group],
    identities: &[Identity],
) -> Usage {
    let tree = GroupTree::new(groups);
    Usage {
        hosts: hosts
            .iter()
            .filter(|h| {
                resolve(h, &tree.ancestors(h.group), identities)
                    .settings
                    .identity
                    .value
                    == Some(identity)
            })
            .map(|h| h.id)
            .collect(),
        groups: groups
            .iter()
            .filter(|g| g.settings.identity == Some(identity))
            .map(|g| g.id)
            .collect(),
        identities: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use jiff::Timestamp;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn counts_direct_inherited_and_identity_use() {
        let now = Timestamp::UNIX_EPOCH;
        let key: Id<Key> = Id::generate();
        let other: Id<Key> = Id::generate();
        let mut deploy = Identity::new("deploy", now);
        deploy.key = Some(key);
        let identities = [deploy.clone()];

        let mut group = Group::new("prod", None, now);
        group.settings.identity = Some(deploy.id);
        let mut plain_group = Group::new("dev", None, now);
        plain_group.settings.key = Some(other);

        let host = |group: Option<Id<Group>>, key: Option<Id<Key>>| {
            let mut h = Host::new("h".parse().unwrap(), now);
            h.group = group;
            h.settings.key = key;
            h
        };
        let direct = host(None, Some(key));
        let inherited = host(Some(group.id), None);
        let overridden = host(Some(group.id), Some(other));
        let unrelated = host(Some(plain_group.id), None);
        let hosts = [
            direct.clone(),
            inherited.clone(),
            overridden.clone(),
            unrelated,
        ];
        let groups = [group.clone(), plain_group];

        let usage = usage_of_key(key, &hosts, &groups, &identities);
        assert_eq!(usage.hosts, [direct.id, inherited.id]);
        assert_eq!(usage.groups, [group.id]);
        assert_eq!(usage.identities, [deploy.id]);

        let usage = usage_of_identity(deploy.id, &hosts, &groups, &identities);
        // The host with its own key still uses the identity (for its user name).
        assert_eq!(usage.hosts, [inherited.id, overridden.id]);
        assert_eq!(usage.groups, [group.id]);

        assert!(usage_of_key(Id::generate(), &hosts, &groups, &identities).is_empty());
    }
}
