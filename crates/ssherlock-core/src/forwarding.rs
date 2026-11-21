//! Port-forwarding rules saved per host (FWD-01…05, FWD-08, HOST-11).
//!
//! **Model** layer of the forwarding feature. A [`ForwardRule`] belongs to one host and says
//! what to forward, as a [`Forward`]: a local port to somewhere the server can reach
//! (`ssh -L`), a port on the server back to somewhere this machine can reach (`ssh -R`), or a
//! local SOCKS proxy whose connections leave from the server (`ssh -D`). Running them is
//! `ssherlock-ssh`'s job; everything here is plain data.

use std::borrow::Cow;
use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::address::Address;
use crate::hosts::{Host, MAX_NAME_LEN};
use crate::id::Id;
use crate::port::Port;

/// Which addresses a forwarded port listens on (FWD-08).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bind {
    /// The loopback address only, so only programs on the same machine can connect: this
    /// machine for a local or dynamic forward, the server for a remote one.
    #[default]
    Localhost,
    /// Every address, so other machines on the network can connect too. For a remote forward
    /// the server decides: OpenSSH allows it only with `GatewayPorts`.
    AllInterfaces,
}

/// A port a forward listens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Listen {
    /// Which addresses.
    #[serde(default)]
    pub bind: Bind,
    /// The port.
    pub port: Port,
}

impl Listen {
    /// `port` on the loopback address.
    pub fn localhost(port: Port) -> Self {
        Self {
            bind: Bind::Localhost,
            port,
        }
    }
}

impl fmt::Display for Listen {
    /// `localhost:8080`, or `*:8080` for every address, as `ss` and `lsof` show it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.bind {
            Bind::Localhost => write!(f, "localhost:{}", self.port),
            Bind::AllInterfaces => write!(f, "*:{}", self.port),
        }
    }
}

/// Where forwarded connections go: a host and port, resolved by whichever side connects to it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Destination {
    /// The host, as a name or an IP address.
    pub host: Address,
    /// The port.
    pub port: Port,
}

impl fmt::Display for Destination {
    /// `db.internal:5432`, with an IPv6 address in brackets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host.to_bracketed(), self.port)
    }
}

/// What a rule forwards.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Forward {
    /// Listen on this machine; each connection is carried to `destination` from the server
    /// (FWD-01, `ssh -L`).
    Local {
        /// The port on this machine.
        listen: Listen,
        /// Where the server connects to.
        destination: Destination,
    },
    /// Ask the server to listen; each connection is carried back to `destination` from this
    /// machine (FWD-02, `ssh -R`).
    Remote {
        /// The port on the server.
        listen: Listen,
        /// Where this machine connects to.
        destination: Destination,
    },
    /// A SOCKS5 proxy on this machine; each connection goes wherever the program using the
    /// proxy asks, from the server (FWD-03, `ssh -D`).
    Dynamic {
        /// The proxy's port on this machine.
        listen: Listen,
    },
}

/// Which machine a forward listens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// This machine.
    Local,
    /// The server.
    Remote,
}

impl Forward {
    /// The port the forward listens on.
    pub fn listen(&self) -> Listen {
        match self {
            Self::Local { listen, .. } | Self::Remote { listen, .. } | Self::Dynamic { listen } => {
                *listen
            }
        }
    }

    /// Which machine it listens on.
    pub fn side(&self) -> Side {
        match self {
            Self::Local { .. } | Self::Dynamic { .. } => Side::Local,
            Self::Remote { .. } => Side::Remote,
        }
    }

    /// Where connections go; `None` for a SOCKS proxy, where each connection says.
    pub fn destination(&self) -> Option<&Destination> {
        match self {
            Self::Local { destination, .. } | Self::Remote { destination, .. } => Some(destination),
            Self::Dynamic { .. } => None,
        }
    }

    /// The kind's name for lists and pickers.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Local { .. } => "Local",
            Self::Remote { .. } => "Remote",
            Self::Dynamic { .. } => "SOCKS",
        }
    }

    /// The same forward as OpenSSH's command-line option: `-L 8080:db:5432`,
    /// `-R 0.0.0.0:9000:localhost:3000`, `-D 1080`. The loopback address is OpenSSH's default,
    /// so it's left out.
    pub fn to_openssh(&self) -> String {
        let listen = |listen: &Listen| match listen.bind {
            Bind::Localhost => listen.port.to_string(),
            Bind::AllInterfaces => format!("*:{}", listen.port),
        };
        match self {
            Self::Local {
                listen: l,
                destination,
            } => format!("-L {}:{destination}", listen(l)),
            Self::Remote {
                listen: l,
                destination,
            } => format!("-R {}:{destination}", listen(l)),
            Self::Dynamic { listen: l } => format!("-D {}", listen(l)),
        }
    }

    /// Whether `self` and `other` would listen on the same port of the same machine, so they
    /// can't both run at once. Every address includes the loopback one, so the bind doesn't
    /// matter.
    pub fn clashes_with(&self, other: &Self) -> bool {
        self.side() == other.side() && self.listen().port == other.listen().port
    }
}

impl fmt::Display for Forward {
    /// What it does in a few words: `localhost:8080 → db.internal:5432`,
    /// `server localhost:9000 → localhost:3000`, `SOCKS proxy on localhost:1080`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local {
                listen,
                destination,
            } => write!(f, "{listen} \u{2192} {destination}"),
            Self::Remote {
                listen,
                destination,
            } => write!(f, "server {listen} \u{2192} {destination}"),
            Self::Dynamic { listen } => write!(f, "SOCKS proxy on {listen}"),
        }
    }
}

/// A forward saved with a host (FWD-04, HOST-11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardRule {
    /// Unique id.
    pub id: Id<ForwardRule>,
    /// The host whose connection carries it.
    pub host: Id<Host>,
    /// The name shown in lists. Empty means "describe the forward".
    #[serde(default)]
    pub label: String,
    /// What it forwards.
    pub forward: Forward,
    /// Whether the rule is in use. A disabled rule is kept, with its settings, but never
    /// started until it's enabled again.
    #[serde(default = "enabled")]
    pub is_enabled: bool,
    /// Whether it starts by itself whenever the host is connected to (FWD-05). Otherwise it's
    /// started by hand.
    #[serde(default)]
    pub auto_start: bool,
    /// When the rule was created.
    pub created_at: Timestamp,
    /// When the rule was last edited.
    pub updated_at: Timestamp,
}

/// Rules saved before `is_enabled` existed were all in use.
fn enabled() -> bool {
    true
}

/// Why a [`ForwardRule`] can't be saved.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ForwardRuleError {
    /// The label is longer than 120 characters.
    #[error("the label is longer than 120 characters")]
    LabelTooLong,
    /// The label has a newline or other control character.
    #[error("the label cannot contain line breaks or control characters")]
    LabelControl,
    /// Two enabled rules of one host listen on the same port.
    #[error("two rules listen on port {port} of the same machine")]
    Clash {
        /// The port.
        port: Port,
    },
}

impl ForwardRule {
    /// A new enabled rule for `host` doing `forward`, started by hand, created at `now`.
    pub fn new(host: Id<Host>, forward: Forward, now: Timestamp) -> Self {
        Self {
            id: Id::generate(),
            host,
            label: String::new(),
            forward,
            is_enabled: true,
            auto_start: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// The name to show: the label, or what the forward does when there's no label.
    pub fn title(&self) -> Cow<'_, str> {
        let label = self.label.trim();
        if label.is_empty() {
            Cow::Owned(self.forward.to_string())
        } else {
            Cow::Borrowed(label)
        }
    }

    /// Checks the rule can be saved. Pure and total.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), ForwardRuleError> {
        if self.label.chars().count() > MAX_NAME_LEN {
            return Err(ForwardRuleError::LabelTooLong);
        }
        if self.label.chars().any(char::is_control) {
            return Err(ForwardRuleError::LabelControl);
        }
        Ok(())
    }

    /// Checks a host's rules can be saved together: each is valid, and no two enabled ones
    /// listen on the same port of the same machine.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate_all(rules: &[Self]) -> Result<(), ForwardRuleError> {
        rules.iter().try_for_each(Self::validate)?;
        let enabled: Vec<&Self> = rules.iter().filter(|r| r.is_enabled).collect();
        for (index, rule) in enabled.iter().enumerate() {
            let later = enabled.iter().skip(index + 1);
            if let Some(other) = later
                .into_iter()
                .find(|o| rule.forward.clashes_with(&o.forward))
            {
                return Err(ForwardRuleError::Clash {
                    port: other.forward.listen().port,
                });
            }
        }
        Ok(())
    }

    /// A copy for `host` with a new id, saved at `now`: for duplicating a host (HOST-08).
    #[must_use]
    pub fn copy_for(&self, host: Id<Host>, now: Timestamp) -> Self {
        Self {
            id: Id::generate(),
            host,
            created_at: now,
            updated_at: now,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn port(n: u16) -> Port {
        Port::new(n).unwrap()
    }

    fn to(host: &str, p: u16) -> Destination {
        Destination {
            host: host.parse().unwrap(),
            port: port(p),
        }
    }

    fn all(p: u16) -> Listen {
        Listen {
            bind: Bind::AllInterfaces,
            port: port(p),
        }
    }

    fn local(p: u16, host: &str, dp: u16) -> Forward {
        Forward::Local {
            listen: Listen::localhost(port(p)),
            destination: to(host, dp),
        }
    }

    fn remote(p: u16, host: &str, dp: u16) -> Forward {
        Forward::Remote {
            listen: Listen::localhost(port(p)),
            destination: to(host, dp),
        }
    }

    fn dynamic(p: u16) -> Forward {
        Forward::Dynamic {
            listen: Listen::localhost(port(p)),
        }
    }

    fn rule(forward: Forward) -> ForwardRule {
        ForwardRule::new(Id::generate(), forward, Timestamp::UNIX_EPOCH)
    }

    #[test]
    fn descriptions() {
        let cases = [
            (
                local(8080, "db.internal", 5432),
                "localhost:8080 \u{2192} db.internal:5432",
                "-L 8080:db.internal:5432",
                "Local",
            ),
            (
                Forward::Local {
                    listen: all(8080),
                    destination: to("::1", 80),
                },
                "*:8080 \u{2192} [::1]:80",
                "-L *:8080:[::1]:80",
                "Local",
            ),
            (
                remote(9000, "localhost", 3000),
                "server localhost:9000 \u{2192} localhost:3000",
                "-R 9000:localhost:3000",
                "Remote",
            ),
            (
                Forward::Remote {
                    listen: all(9000),
                    destination: to("10.0.0.5", 22),
                },
                "server *:9000 \u{2192} 10.0.0.5:22",
                "-R *:9000:10.0.0.5:22",
                "Remote",
            ),
            (
                dynamic(1080),
                "SOCKS proxy on localhost:1080",
                "-D 1080",
                "SOCKS",
            ),
            (
                Forward::Dynamic { listen: all(1080) },
                "SOCKS proxy on *:1080",
                "-D *:1080",
                "SOCKS",
            ),
        ];
        for (forward, display, openssh, kind) in cases {
            assert_eq!(forward.to_string(), display);
            assert_eq!(forward.to_openssh(), openssh);
            assert_eq!(forward.kind_name(), kind);
        }
    }

    #[test]
    fn sides_and_destinations() {
        assert_eq!(local(1, "a", 2).side(), Side::Local);
        assert_eq!(dynamic(1).side(), Side::Local);
        assert_eq!(remote(1, "a", 2).side(), Side::Remote);
        assert_eq!(remote(1, "a", 2).destination(), Some(&to("a", 2)));
        assert_eq!(dynamic(1).destination(), None);
        assert_eq!(dynamic(7).listen(), Listen::localhost(port(7)));
    }

    #[test]
    fn clashes() {
        let cases = [
            (local(8080, "a", 1), local(8080, "b", 2), true),
            (local(8080, "a", 1), dynamic(8080), true),
            (
                local(8080, "a", 1),
                Forward::Dynamic { listen: all(8080) },
                true,
            ),
            (local(8080, "a", 1), local(8081, "a", 1), false),
            // One listens here, the other on the server.
            (local(8080, "a", 1), remote(8080, "a", 1), false),
            (remote(9000, "a", 1), remote(9000, "b", 2), true),
        ];
        for (a, b, expected) in cases {
            assert_eq!(a.clashes_with(&b), expected, "{a} / {b}");
            assert_eq!(b.clashes_with(&a), expected, "{b} / {a}");
        }
    }

    #[test]
    fn validation() {
        let labelled = |label: &str| ForwardRule {
            label: label.into(),
            ..rule(dynamic(1080))
        };
        let cases = [
            (labelled(""), Ok(())),
            (labelled("Postgres"), Ok(())),
            (labelled(&"x".repeat(120)), Ok(())),
            (
                labelled(&"x".repeat(121)),
                Err(ForwardRuleError::LabelTooLong),
            ),
            (labelled("a\tb"), Err(ForwardRuleError::LabelControl)),
        ];
        for (rule, expected) in cases {
            assert_eq!(rule.validate(), expected, "{:?}", rule.label);
        }
    }

    #[test]
    fn a_host_cannot_have_two_enabled_rules_on_one_port() {
        let a = rule(local(8080, "a", 1));
        let b = rule(dynamic(8080));
        let c = rule(remote(8080, "c", 3));
        assert_eq!(
            ForwardRule::validate_all(&[a.clone(), b.clone()]),
            Err(ForwardRuleError::Clash { port: port(8080) })
        );
        // A disabled rule doesn't run, so it can't clash; the other side's port is separate.
        let disabled = ForwardRule {
            is_enabled: false,
            ..b
        };
        assert_eq!(ForwardRule::validate_all(&[a.clone(), disabled, c]), Ok(()));
        let bad_label = ForwardRule {
            label: "x\n".into(),
            ..a
        };
        assert_eq!(
            ForwardRule::validate_all(&[bad_label]),
            Err(ForwardRuleError::LabelControl)
        );
        assert_eq!(ForwardRule::validate_all(&[]), Ok(()));
    }

    #[test]
    fn title_falls_back_to_the_description() {
        let mut r = rule(dynamic(1080));
        assert_eq!(r.title(), "SOCKS proxy on localhost:1080");
        r.label = "  Browser proxy ".into();
        assert_eq!(r.title(), "Browser proxy");
    }

    #[test]
    fn copies_belong_to_the_new_host() {
        let mut original = rule(local(8080, "db", 5432));
        original.label = "DB".into();
        original.auto_start = true;
        let host = Id::generate();
        let now = Timestamp::from_second(9).unwrap();
        let copy = original.copy_for(host, now);
        assert_ne!(copy.id, original.id);
        assert_eq!(
            (copy.host, copy.created_at, copy.updated_at),
            (host, now, now)
        );
        assert_eq!(
            (&copy.label, &copy.forward, copy.auto_start),
            (&original.label, &original.forward, true)
        );
    }

    #[test]
    fn json_round_trip_and_defaults() {
        let r = rule(Forward::Remote {
            listen: all(9000),
            destination: to("localhost", 3000),
        });
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<ForwardRule>(&json).unwrap(), r);

        let old = r#"{
            "id": "01936f4e-7a2b-7c3d-8e4f-5a6b7c8d9e0f",
            "host": "01936f4e-7a2b-7c3d-8e4f-5a6b7c8d9e10",
            "forward": {"kind": "dynamic", "listen": {"port": 1080}},
            "created_at": "2026-01-02T03:04:05Z",
            "updated_at": "2026-01-02T03:04:05Z"
        }"#;
        let r: ForwardRule = serde_json::from_str(old).unwrap();
        assert!(r.is_enabled && !r.auto_start && r.label.is_empty());
        assert_eq!(r.forward, dynamic(1080));
        assert!(serde_json::from_str::<ForwardRule>(&old.replace("1080", "0")).is_err());
    }
}
