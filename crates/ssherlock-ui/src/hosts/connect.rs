//! Turning a saved host into what a new remote tab needs: the resolved target, connection
//! options and colour. Shared by the hosts page and the quick connect bar, so a host connects
//! the same way from either.

use ssherlock_core::hosts::{Colour, Group, GroupTree, Host, resolve};
use ssherlock_core::keychain::Identity;
use ssherlock_core::{ConnectTarget, Id, Port};

use crate::ssh::{ConnectOptions, SavedCredentials};

/// One saved host to connect to, resolved against its groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectRequest {
    /// The saved host.
    pub host: Id<Host>,
    /// Where and as whom.
    pub target: ConnectTarget,
    /// Algorithms and compression.
    pub options: ConnectOptions,
    /// The colour its tab is marked with (TERM-05).
    pub colour: Option<Colour>,
}

impl ConnectRequest {
    /// The request for `host`, inheriting from `groups` and the `identities` they pick.
    pub fn new(host: &Host, groups: &[Group], identities: &[Identity]) -> Self {
        let tree = GroupTree::new(groups);
        let effective = resolve(host, &tree.ancestors(host.group), identities);
        let settings = effective.settings;
        Self {
            host: host.id,
            options: ConnectOptions {
                algorithms: settings.algorithms(),
                compression: settings.compression.value,
                credentials: SavedCredentials {
                    key: settings.key.value,
                    password: settings.password.value,
                },
            },
            target: ConnectTarget {
                user: settings.username.value,
                address: effective.address,
                port: Some(settings.port.value),
            },
            colour: settings.colour.value,
        }
    }
}

/// `user@address[:port]` for a resolved target, leaving out the default port. Without a saved
/// user, the local user name the connection will use.
pub fn target_text(target: &ConnectTarget, local_user: Option<&str>) -> String {
    let user = target.user.as_deref().or(local_user);
    let display = ConnectTarget {
        user: user.map(str::to_owned),
        address: target.address.clone(),
        port: target.port.filter(|p| *p != Port::SSH),
    };
    display.to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;
    use ssherlock_core::{Compression, Timestamp};

    use super::*;

    #[test]
    fn a_request_carries_everything_inherited() {
        let now = Timestamp::UNIX_EPOCH;
        let mut group = Group::new("prod", None, now);
        group.settings.username = Some("ops".into());
        group.settings.colour = Some(Colour::Red);
        group.settings.compression = Some(Compression::On);
        group.settings.algorithms.cipher = vec!["aes256-gcm@openssh.com".into()];
        let mut host = Host::new("db.example.com".parse().unwrap(), now);
        host.group = Some(group.id);
        host.settings.port = Port::new(2222);

        let request = ConnectRequest::new(&host, &[group], &[]);
        assert_eq!(request.target.to_string(), "ops@db.example.com:2222");
        assert_eq!(request.colour, Some(Colour::Red));
        assert_eq!(request.options.compression, Compression::On);
        assert_eq!(
            request.options.algorithms.cipher,
            ["aes256-gcm@openssh.com"]
        );
    }

    #[test]
    fn credentials_can_come_from_an_identity() {
        let now = Timestamp::UNIX_EPOCH;
        let mut identity = Identity::new("deploy", now);
        identity.key = Some(Id::generate());
        identity.password = Some(ssherlock_core::Sealed::from_bytes(vec![1]));
        let mut host = Host::new("example.com".parse().unwrap(), now);
        host.settings.identity = Some(identity.id);
        let request = ConnectRequest::new(&host, &[], std::slice::from_ref(&identity));
        assert_eq!(request.target.user.as_deref(), Some("deploy"));
        assert_eq!(request.options.credentials.key, identity.key);
        assert_eq!(request.options.credentials.password, identity.password);
    }

    #[test]
    fn target_text_leaves_out_the_default_port() {
        let target = |user: Option<&str>, port: u16| ConnectTarget {
            user: user.map(str::to_owned),
            address: "example.com".parse().unwrap(),
            port: Port::new(port),
        };
        assert_eq!(target_text(&target(Some("a"), 22), None), "a@example.com");
        assert_eq!(
            target_text(&target(None, 2222), Some("me")),
            "me@example.com:2222"
        );
        assert_eq!(target_text(&target(None, 22), None), "example.com");
    }
}
