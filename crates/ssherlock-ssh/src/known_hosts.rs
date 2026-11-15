//! OpenSSH-format `known_hosts` files: check a server's host key, add and replace entries.
//!
//! **Service** layer. Parsing, checking and editing are pure and work on text; only
//! [`KnownHosts::load`] and [`KnownHosts::save`] touch the disk. Lines this module doesn't
//! understand (comments, unknown markers, key types it can't parse) are kept verbatim, so saving
//! a file never loses anything another tool wrote.
//!
//! Supported: plain and hashed (`|1|salt|hash`) host fields, comma-separated patterns with
//! `*`/`?` wildcards and `!` negation, `[host]:port` names, and the `@revoked` and
//! `@cert-authority` markers (certificate authorities are parsed but not used until CONN-05).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{KeyInit as _, Mac as _};
use rand::TryRng as _;
use russh::keys::PublicKey;

use crate::SshError;

/// HMAC-SHA1, the hash OpenSSH uses for hashed host names.
type HmacSha1 = hmac::Hmac<sha1::Sha1>;

/// Length of the salt OpenSSH uses for hashed host names (the SHA-1 output size).
const SALT_LEN: usize = 20;

/// The name a host is stored under in `known_hosts`: `host` on port 22, `[host]:port` otherwise.
/// Host names are lower-cased, as OpenSSH does.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KnownHostName(String);

impl KnownHostName {
    /// The name for `host` (a DNS name or an IP literal, without brackets) on `port`.
    pub fn new(host: &str, port: u16) -> Self {
        let host = host.to_ascii_lowercase();
        if port == 22 {
            Self(host)
        } else {
            Self(format!("[{host}]:{port}"))
        }
    }

    /// The name as written in `known_hosts`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What `known_hosts` says about a host key the server presented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// The key is listed for this host.
    Trusted,
    /// Nothing is known about this host with this key's algorithm. `other_algorithms` lists keys
    /// of other types known for the host, so the UI can say the host isn't entirely new.
    Unknown {
        /// Known keys of other algorithms for this host.
        other_algorithms: Vec<KnownKey>,
    },
    /// The host is known with a different key of the same algorithm: possibly an attack.
    Changed {
        /// The keys `known_hosts` expected.
        known: Vec<KnownKey>,
    },
    /// The key is marked `@revoked`. Never accept it.
    Revoked {
        /// Where it was revoked.
        at: KnownKeyLocation,
    },
}

/// A key found in a `known_hosts` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownKey {
    /// Where it was found.
    pub at: KnownKeyLocation,
    /// The key.
    pub key: PublicKey,
}

/// A line in a `known_hosts` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownKeyLocation {
    /// The file.
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
}

/// Whether new entries hide the host name. Hashed matches OpenSSH's `HashKnownHosts yes`, the
/// default on Debian and Ubuntu: a stolen file then doesn't list the hosts you connect to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostHashing {
    /// `|1|salt|hash` host field.
    #[default]
    Hashed,
    /// The host name in clear text.
    Plain,
}

/// One `known_hosts` file, held in memory.
#[derive(Clone, Debug)]
pub struct KnownHosts {
    path: PathBuf,
    lines: Vec<Line>,
}

#[derive(Clone, Debug)]
struct Line {
    /// The line exactly as read (or as written by us), without its newline.
    text: String,
    /// `None` for blank lines, comments and anything we couldn't parse.
    entry: Option<Entry>,
}

#[derive(Clone, Debug)]
struct Entry {
    marker: Option<Marker>,
    hosts: Hosts,
    /// Everything after the host field (key type, base64, comment), kept for rewriting the line.
    rest: String,
    key: PublicKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marker {
    CertAuthority,
    Revoked,
}

impl Marker {
    fn as_str(self) -> &'static str {
        match self {
            Self::CertAuthority => "@cert-authority",
            Self::Revoked => "@revoked",
        }
    }
}

#[derive(Clone, Debug)]
enum Hosts {
    Hashed { salt: Vec<u8>, hash: Vec<u8> },
    Patterns(Vec<Pattern>),
}

#[derive(Clone, Debug)]
struct Pattern {
    negated: bool,
    /// Lower-cased, without the `!`.
    glob: String,
}

impl Pattern {
    fn parse(text: &str) -> Self {
        let (negated, glob) = match text.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        Self {
            negated,
            glob: glob.to_ascii_lowercase(),
        }
    }

    fn is_literal(&self) -> bool {
        !self.glob.contains(['*', '?'])
    }

    fn to_text(&self) -> String {
        if self.negated {
            format!("!{}", self.glob)
        } else {
            self.glob.clone()
        }
    }
}

impl Hosts {
    fn parse(field: &str) -> Option<Self> {
        if let Some(hashed) = field.strip_prefix("|1|") {
            let (salt, hash) = hashed.split_once('|')?;
            return Some(Self::Hashed {
                salt: BASE64.decode(salt).ok()?,
                hash: BASE64.decode(hash).ok()?,
            });
        }
        let patterns: Vec<Pattern> = field
            .split(',')
            .filter(|p| !p.is_empty())
            .map(Pattern::parse)
            .collect();
        (!patterns.is_empty()).then_some(Self::Patterns(patterns))
    }

    /// OpenSSH's rule: a matching negated pattern rejects the line outright; otherwise any
    /// matching positive pattern accepts it.
    fn matches(&self, name: &KnownHostName) -> bool {
        match self {
            Self::Hashed { salt, hash } => {
                hmac_sha1(salt, name.as_str()).is_some_and(|h| h.as_slice() == hash.as_slice())
            }
            Self::Patterns(patterns) => {
                let mut accepted = false;
                for p in patterns {
                    if glob_match(&p.glob, name.as_str()) {
                        if p.negated {
                            return false;
                        }
                        accepted = true;
                    }
                }
                accepted
            }
        }
    }
}

impl Entry {
    fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let (first, after_first) = split_field(trimmed);
        let (marker, hosts_field, rest) = if first.starts_with('@') {
            let marker = match first {
                "@cert-authority" => Marker::CertAuthority,
                "@revoked" => Marker::Revoked,
                _ => return None,
            };
            let (hosts, rest) = split_field(after_first);
            (Some(marker), hosts, rest)
        } else {
            (None, first, after_first)
        };
        let hosts = Hosts::parse(hosts_field)?;
        let rest = rest.trim();
        let key = PublicKey::from_openssh(rest).ok()?;
        Some(Self {
            marker,
            hosts,
            rest: rest.to_owned(),
            key,
        })
    }

    fn to_text(&self) -> String {
        let hosts = match &self.hosts {
            Hosts::Hashed { salt, hash } => {
                format!("|1|{}|{}", BASE64.encode(salt), BASE64.encode(hash))
            }
            Hosts::Patterns(patterns) => patterns
                .iter()
                .map(Pattern::to_text)
                .collect::<Vec<_>>()
                .join(","),
        };
        match self.marker {
            Some(marker) => format!("{} {hosts} {}", marker.as_str(), self.rest),
            None => format!("{hosts} {}", self.rest),
        }
    }

    fn is_plain_key(&self) -> bool {
        self.marker.is_none()
    }
}

impl KnownHosts {
    /// An empty file at `path` (not created until [`KnownHosts::save`]).
    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lines: Vec::new(),
        }
    }

    /// Parses `text` as the contents of the file at `path`. Never fails: lines that can't be
    /// parsed are kept but ignored.
    pub fn parse(path: impl Into<PathBuf>, text: &str) -> Self {
        let lines = text
            .lines()
            .map(|line| Line {
                text: line.to_owned(),
                entry: Entry::parse(line),
            })
            .collect();
        Self {
            path: path.into(),
            lines,
        }
    }

    /// Reads the file at `path`. A missing file is an empty one.
    ///
    /// # Errors
    ///
    /// [`SshError::KnownHostsRead`] if the file exists but can't be read as UTF-8 text.
    pub async fn load(path: impl Into<PathBuf>) -> Result<Self, SshError> {
        let path = path.into();
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => {
                let known = Self::parse(path, &text);
                let skipped = known.unparsed_entry_count();
                if skipped > 0 {
                    tracing::warn!(
                        path = %known.path.display(),
                        skipped,
                        "ignoring known_hosts lines that could not be parsed; they are kept on save"
                    );
                }
                Ok(known)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty(path)),
            Err(source) => Err(SshError::KnownHostsRead { path, source }),
        }
    }

    /// Writes the file, replacing it atomically and creating its directory if needed.
    ///
    /// # Errors
    ///
    /// [`SshError::KnownHostsWrite`] if the directory or file can't be written.
    pub async fn save(&self) -> Result<(), SshError> {
        let write_err = |source| SshError::KnownHostsWrite {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(dir).await.map_err(write_err)?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        tokio::fs::write(&tmp, self.to_text())
            .await
            .map_err(write_err)?;
        tokio::fs::rename(&tmp, &self.path).await.map_err(write_err)
    }

    /// The file this was read from or will be saved to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's contents.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            text.push_str(&line.text);
            text.push('\n');
        }
        text
    }

    /// Checks `key` for `name`, with OpenSSH's precedence: revoked, then trusted, then changed.
    pub fn check(&self, name: &KnownHostName, key: &PublicKey) -> HostKeyStatus {
        let mut same_algorithm = Vec::new();
        let mut other_algorithms = Vec::new();
        let mut trusted = false;
        for (index, entry) in self.entries() {
            let same_key = entry.key.key_data() == key.key_data();
            match entry.marker {
                Some(Marker::Revoked) if same_key && entry.hosts.matches(name) => {
                    return HostKeyStatus::Revoked {
                        at: self.location(index),
                    };
                }
                None if entry.hosts.matches(name) => {
                    if same_key {
                        trusted = true;
                    } else {
                        let known = KnownKey {
                            at: self.location(index),
                            key: entry.key.clone(),
                        };
                        if entry.key.algorithm() == key.algorithm() {
                            same_algorithm.push(known);
                        } else {
                            other_algorithms.push(known);
                        }
                    }
                }
                _ => {}
            }
        }
        if trusted {
            HostKeyStatus::Trusted
        } else if same_algorithm.is_empty() {
            HostKeyStatus::Unknown { other_algorithms }
        } else {
            HostKeyStatus::Changed {
                known: same_algorithm,
            }
        }
    }

    /// Appends an entry trusting `key` for `name`.
    ///
    /// # Errors
    ///
    /// [`SshError::Random`] if hashing and the OS can't supply a salt, or
    /// [`SshError::KeyEncoding`] if the key can't be written in OpenSSH format.
    pub fn add(
        &mut self,
        name: &KnownHostName,
        key: &PublicKey,
        hashing: HostHashing,
    ) -> Result<(), SshError> {
        let line = new_line(name, key, hashing)?;
        self.lines.push(line);
        Ok(())
    }

    /// Trusts `key` for `name` after the user accepted a changed key: removes the old entries of
    /// the same algorithm for `name`, then adds the new one. Returns how many entries it removed.
    ///
    /// Only hashed entries and literal patterns are removed (a literal is dropped from a
    /// multi-host line, leaving the other hosts). Wildcard lines are left alone because they
    /// cover other hosts too; the new entry still wins, since a matching key is trusted.
    ///
    /// # Errors
    ///
    /// As [`KnownHosts::add`]. Nothing is removed if adding fails.
    pub fn replace(
        &mut self,
        name: &KnownHostName,
        key: &PublicKey,
        hashing: HostHashing,
    ) -> Result<usize, SshError> {
        // Built first so a failure leaves the file untouched.
        let new = new_line(name, key, hashing)?;
        let algorithm = key.algorithm();
        let mut removed = 0;
        self.lines.retain_mut(|line| {
            let Some(entry) = line.entry.as_mut() else {
                return true;
            };
            if !entry.is_plain_key()
                || entry.key.algorithm() != algorithm
                || entry.key.key_data() == key.key_data()
                || !entry.hosts.matches(name)
            {
                return true;
            }
            match &mut entry.hosts {
                Hosts::Hashed { .. } => {
                    removed += 1;
                    false
                }
                Hosts::Patterns(patterns) => {
                    let before = patterns.len();
                    patterns.retain(|p| p.negated || !p.is_literal() || p.glob != name.as_str());
                    if patterns.len() == before {
                        return true;
                    }
                    removed += 1;
                    if patterns.iter().any(|p| !p.negated) {
                        line.text = entry.to_text();
                        true
                    } else {
                        false
                    }
                }
            }
        });
        self.lines.push(new);
        Ok(removed)
    }

    fn entries(&self) -> impl Iterator<Item = (usize, &Entry)> {
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| Some((i, line.entry.as_ref()?)))
    }

    fn location(&self, index: usize) -> KnownKeyLocation {
        KnownKeyLocation {
            path: self.path.clone(),
            line: index + 1,
        }
    }

    fn unparsed_entry_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| {
                let t = line.text.trim_start();
                line.entry.is_none() && !t.is_empty() && !t.starts_with('#')
            })
            .count()
    }
}

/// A line trusting `key` for `name`.
fn new_line(name: &KnownHostName, key: &PublicKey, hashing: HostHashing) -> Result<Line, SshError> {
    // Host keys from a server carry no comment; drop any so the line is just `type base64`.
    let bare = PublicKey::new(key.key_data().clone(), "");
    let rest = bare.to_openssh().map_err(|source| SshError::KeyEncoding {
        algorithm: key.algorithm().to_string(),
        source,
    })?;
    let hosts = match hashing {
        HostHashing::Plain => Hosts::Patterns(vec![Pattern::parse(name.as_str())]),
        HostHashing::Hashed => {
            let mut salt = vec![0_u8; SALT_LEN];
            rand::rngs::SysRng.try_fill_bytes(&mut salt)?;
            let hash = hmac_sha1(&salt, name.as_str()).unwrap_or_default();
            Hosts::Hashed { salt, hash }
        }
    };
    let entry = Entry {
        marker: None,
        hosts,
        rest,
        key: bare,
    };
    Ok(Line {
        text: entry.to_text(),
        entry: Some(entry),
    })
}

/// Splits off the first whitespace-separated field; the remainder keeps its inner spacing.
fn split_field(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.find([' ', '\t']) {
        Some(end) => (
            text.get(..end).unwrap_or(text),
            text.get(end..).unwrap_or(""),
        ),
        None => (text, ""),
    }
}

fn hmac_sha1(salt: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut mac = HmacSha1::new_from_slice(salt).ok()?;
    mac.update(name.as_bytes());
    Some(mac.finalize().into_bytes().to_vec())
}

/// OpenSSH's `match_pattern`: `*` matches any run of characters, `?` exactly one. Both sides
/// are expected lower-cased already.
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0, 0);
    // Where to resume after the last `*`: (pattern index after it, text index it now covers to).
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some(b'*') => {
                star = Some((pi + 1, ti));
                pi += 1;
            }
            Some(&c) if c == b'?' || Some(&c) == t.get(ti) => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((after, covered)) => {
                    pi = after;
                    ti = covered + 1;
                    star = Some((after, covered + 1));
                }
                None => return false,
            },
        }
    }
    p.get(pi..)
        .is_some_and(|rest| rest.iter().all(|&c| c == b'*'))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    use super::*;

    const ED25519: &str = include_str!("../tests/fixtures/id_ed25519.pub");
    const ECDSA: &str = include_str!("../tests/fixtures/id_ecdsa.pub");
    const OTHER_ED25519: &str = include_str!("../tests/fixtures/id_unauthorized.pub");
    /// Written by OpenSSH's `ssh-keygen -H` from `example.com <ED25519>` and
    /// `[example.com]:2222 <ECDSA>`.
    const OPENSSH_HASHED: &str = "\
|1|B7CLmEsOp/9cBaycYBl7IYqff8g=|MgOPpM/9fpqBX5WjmoJT5xZiQNE= ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILBQekpVMwEDyukQecYTrTG3LBSZr9QQcICL9MsKYn/6
|1|+RIv6a1w1p9yTSP1L43pGNdbS2k=|eZeu+OaKY1sBzZ7y+KZilOrKx+c= ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBM8ADWrJtwO8IKRvhV4ECkPlWLFjYrjSZocf0ruLJCFBqMUpTrhDC8L+XYTCu2i4xEhB9hjEG35cdLvCsDcWkP4=
";

    fn key(line: &str) -> PublicKey {
        PublicKey::from_openssh(line.trim()).unwrap()
    }

    /// `type base64` of a fixture `.pub` line, without its comment.
    fn bare(line: &str) -> String {
        line.split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn file(text: &str) -> KnownHosts {
        KnownHosts::parse("/kh", text)
    }

    fn name(host: &str, port: u16) -> KnownHostName {
        KnownHostName::new(host, port)
    }

    fn at(line: usize) -> KnownKeyLocation {
        KnownKeyLocation {
            path: PathBuf::from("/kh"),
            line,
        }
    }

    #[test]
    fn host_names() {
        for (host, port, expected) in [
            ("example.com", 22, "example.com"),
            ("Example.COM", 22, "example.com"),
            ("example.com", 2222, "[example.com]:2222"),
            ("192.0.2.1", 22, "192.0.2.1"),
            ("::1", 22, "::1"),
            ("::1", 2200, "[::1]:2200"),
        ] {
            assert_eq!(name(host, port).as_str(), expected, "{host}:{port}");
        }
    }

    #[test]
    fn globs() {
        for (pattern, text, expected) in [
            ("example.com", "example.com", true),
            ("example.com", "example.org", false),
            ("*", "anything", true),
            ("*", "", true),
            ("*.example.com", "a.example.com", true),
            ("*.example.com", "example.com", false),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("*a*b", "xxaxxb", true),
            ("*a*b", "xxaxxbx", false),
            ("192.0.2.*", "192.0.2.17", true),
            ("[*]:2222", "[example.com]:2222", true),
            ("[*]:2222", "[example.com]:22", false),
            ("", "", true),
            ("", "a", false),
        ] {
            assert_eq!(
                glob_match(pattern, text),
                expected,
                "{pattern:?} vs {text:?}"
            );
        }
    }

    #[test]
    fn check_outcomes() {
        let ed = bare(ED25519);
        let ec = bare(ECDSA);
        let other = bare(OTHER_ED25519);
        let cases: Vec<(String, (&str, u16), &str, HostKeyStatus)> = vec![
            (
                format!("example.com {ed}"),
                ("example.com", 22),
                ED25519,
                HostKeyStatus::Trusted,
            ),
            (
                format!("EXAMPLE.com {ed}"),
                ("example.COM", 22),
                ED25519,
                HostKeyStatus::Trusted,
            ),
            (
                format!("other.com,example.com {ed}"),
                ("example.com", 22),
                ED25519,
                HostKeyStatus::Trusted,
            ),
            (
                format!("*.example.com {ed}"),
                ("a.example.com", 22),
                ED25519,
                HostKeyStatus::Trusted,
            ),
            (
                format!("*.example.com,!bad.example.com {ed}"),
                ("bad.example.com", 22),
                ED25519,
                HostKeyStatus::Unknown {
                    other_algorithms: vec![],
                },
            ),
            (
                format!("[example.com]:2222 {ed}"),
                ("example.com", 2222),
                ED25519,
                HostKeyStatus::Trusted,
            ),
            (
                format!("example.com {ed}"),
                ("example.com", 2222),
                ED25519,
                HostKeyStatus::Unknown {
                    other_algorithms: vec![],
                },
            ),
            (
                format!("example.com {other}"),
                ("example.com", 22),
                ED25519,
                HostKeyStatus::Changed {
                    known: vec![KnownKey {
                        at: at(1),
                        key: key(&other),
                    }],
                },
            ),
            (
                format!("example.com {ec}"),
                ("example.com", 22),
                ED25519,
                HostKeyStatus::Unknown {
                    other_algorithms: vec![KnownKey {
                        at: at(1),
                        key: key(&ec),
                    }],
                },
            ),
            // Any matching key wins over a stale one, as in OpenSSH.
            (
                format!("example.com {other}\nexample.com {ed}"),
                ("example.com", 22),
                ED25519,
                HostKeyStatus::Trusted,
            ),
            (
                format!("example.com {ed}\n@revoked * {ed}"),
                ("example.com", 22),
                ED25519,
                HostKeyStatus::Revoked { at: at(2) },
            ),
            // A certificate authority isn't a host key.
            (
                format!("@cert-authority example.com {ed}"),
                ("example.com", 22),
                ED25519,
                HostKeyStatus::Unknown {
                    other_algorithms: vec![],
                },
            ),
            (
                format!("# example.com {ed}\n\n  garbage line\n@unknown example.com {ed}"),
                ("example.com", 22),
                ED25519,
                HostKeyStatus::Unknown {
                    other_algorithms: vec![],
                },
            ),
        ];
        for (text, (host, port), presented, expected) in cases {
            let status = file(&text).check(&name(host, port), &key(presented));
            assert_eq!(status, expected, "{text}\nchecking {host}:{port}");
        }
    }

    #[test]
    fn reads_hashed_lines_written_by_openssh() {
        let known = file(OPENSSH_HASHED);
        assert_eq!(
            known.check(&name("example.com", 22), &key(ED25519)),
            HostKeyStatus::Trusted
        );
        assert_eq!(
            known.check(&name("example.com", 2222), &key(ECDSA)),
            HostKeyStatus::Trusted
        );
        assert_eq!(
            known.check(&name("example.org", 22), &key(ED25519)),
            HostKeyStatus::Unknown {
                other_algorithms: vec![]
            }
        );
    }

    #[test]
    fn added_entries_are_trusted() {
        for hashing in [HostHashing::Hashed, HostHashing::Plain] {
            let mut known = KnownHosts::empty("/kh");
            known
                .add(&name("example.com", 2222), &key(ED25519), hashing)
                .unwrap();
            let reparsed = file(&known.to_text());
            assert_eq!(
                reparsed.check(&name("example.com", 2222), &key(ED25519)),
                HostKeyStatus::Trusted,
                "{hashing:?}"
            );
            assert_eq!(
                reparsed.check(&name("example.com", 22), &key(ED25519)),
                HostKeyStatus::Unknown {
                    other_algorithms: vec![]
                },
                "{hashing:?}"
            );
        }
    }

    #[test]
    fn added_lines_have_openssh_shape() {
        let mut known = KnownHosts::empty("/kh");
        known
            .add(&name("example.com", 22), &key(ED25519), HostHashing::Plain)
            .unwrap();
        known
            .add(&name("example.com", 22), &key(ED25519), HostHashing::Hashed)
            .unwrap();
        let text = known.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], format!("example.com {}", bare(ED25519)));
        let hashed: Vec<&str> = lines[1].split(' ').collect();
        assert_eq!(hashed.len(), 3, "{}", lines[1]);
        assert!(hashed[0].starts_with("|1|"), "{}", lines[1]);
        assert_eq!(format!("{} {}", hashed[1], hashed[2]), bare(ED25519));
        // Fresh salt per entry, so two hashed entries for the same host differ.
        known
            .add(&name("example.com", 22), &key(ED25519), HostHashing::Hashed)
            .unwrap();
        let text = known.to_text();
        let again: Vec<&str> = text.lines().collect();
        assert_ne!(again[1], again[2]);
    }

    #[test]
    fn replace_removes_only_this_hosts_stale_keys() {
        let ed = bare(ED25519);
        let ec = bare(ECDSA);
        let other = bare(OTHER_ED25519);
        let mut known = file(&format!(
            "# keep me\n\
             example.com,other.com {other}\n\
             example.com {ec}\n\
             *.com {other}\n\
             example.com {other}\n"
        ));
        let mut hashed = KnownHosts::empty("/kh");
        hashed
            .add(
                &name("example.com", 22),
                &key(OTHER_ED25519),
                HostHashing::Hashed,
            )
            .unwrap();
        let hashed_line = hashed.to_text();
        known.lines.push(Line {
            text: hashed_line.trim_end().to_owned(),
            entry: Entry::parse(hashed_line.trim_end()),
        });

        let removed = known.replace(&name("example.com", 22), &key(ED25519), HostHashing::Plain);
        assert_eq!(removed.unwrap(), 3);
        assert_eq!(
            known.to_text(),
            format!(
                "# keep me\n\
                 other.com {other}\n\
                 example.com {ec}\n\
                 *.com {other}\n\
                 example.com {ed}\n"
            )
        );
        assert_eq!(
            known.check(&name("example.com", 22), &key(ED25519)),
            HostKeyStatus::Trusted
        );
        assert_eq!(
            known.check(&name("other.com", 22), &key(OTHER_ED25519)),
            HostKeyStatus::Trusted
        );
    }

    #[test]
    fn unparsed_lines_survive_a_round_trip() {
        let text = format!(
            "# comment\n\n@weird marker line\nnot a key at all\nexample.com {}\n\tindented # odd\n",
            bare(ED25519)
        );
        let mut known = file(&text);
        assert_eq!(known.to_text(), text);
        assert_eq!(known.unparsed_entry_count(), 3);
        known
            .add(
                &name("new.example.com", 22),
                &key(ECDSA),
                HostHashing::Plain,
            )
            .unwrap();
        assert!(known.to_text().starts_with(&text));
    }

    #[tokio::test]
    async fn load_and_save() {
        let dir = std::env::temp_dir().join(format!("ssherlock-kh-{}", std::process::id()));
        let path = dir.join("nested/known_hosts");
        let mut known = KnownHosts::load(&path).await.unwrap();
        assert_eq!(known.to_text(), "");
        known
            .add(&name("example.com", 22), &key(ED25519), HostHashing::Hashed)
            .unwrap();
        known.save().await.unwrap();
        let reloaded = KnownHosts::load(&path).await.unwrap();
        assert_eq!(reloaded.to_text(), known.to_text());
        assert_eq!(
            reloaded.check(&name("example.com", 22), &key(ED25519)),
            HostKeyStatus::Trusted
        );

        // OpenSSH agrees that our hashed entry is for example.com.
        if let Ok(out) = std::process::Command::new("ssh-keygen")
            .args(["-F", "example.com", "-f"])
            .arg(&path)
            .output()
        {
            assert!(out.status.success(), "ssh-keygen -F did not find our entry");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    proptest! {
        #[test]
        fn parsing_never_panics_and_keeps_every_line(text in "[ -~\t\n|@!*?,\\[\\]]{0,400}") {
            let known = file(&text);
            let expected: String = text.lines().flat_map(|l| [l, "\n"]).collect();
            prop_assert_eq!(known.to_text(), expected);
            let _ = known.check(&name("example.com", 22), &key(ED25519));
        }

        #[test]
        fn glob_star_matches_everything(text in "[a-z0-9.:\\[\\]-]{0,40}") {
            prop_assert!(glob_match("*", &text));
            prop_assert!(glob_match(&text, &text));
        }
    }
}
