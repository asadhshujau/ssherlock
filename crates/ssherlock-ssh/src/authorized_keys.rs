//! Adding a public key to the server's `~/.ssh/authorized_keys`, as `ssh-copy-id` does (KEY-07).
//!
//! **Service** layer. Runs one small POSIX `sh` script over an exec channel on a logged-in
//! [`Session`]. The key goes in on the script's stdin, never inside the command line, so nothing
//! in it can be read as shell syntax.

use russh::ChannelMsg;
use russh::keys::PublicKey;

use crate::SshError;
use crate::session::Session;
use crate::shell::expect_success;

/// Reads one key line on stdin; prints `present` if `authorized_keys` already has its type and
/// data, else appends it (after a newline if the file doesn't end with one) and prints `added`.
/// Wrapped in `exec sh -c` so it runs the same under any login shell (fish, csh, …). Contains no
/// single quotes, so it can sit inside them.
const SCRIPT: &str = r#"set -f
umask 077
IFS= read -r line || exit 3
set -- $line
[ -n "$2" ] || exit 3
mkdir -p "$HOME/.ssh" || exit 4
file="$HOME/.ssh/authorized_keys"
if [ -f "$file" ] && grep -qF -- "$1 $2" "$file"; then echo present; exit 0; fi
if [ -s "$file" ] && [ -n "$(tail -c 1 "$file")" ]; then printf "\n" >> "$file" || exit 4; fi
printf "%s\n" "$line" >> "$file" || exit 4
echo added"#;

/// What [`Session::install_public_key`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyInstall {
    /// The key was appended.
    Added,
    /// The key was already there; nothing changed.
    AlreadyPresent,
}

impl Session {
    /// Adds `public_line` (`ssh-ed25519 AAAA… comment`) to the logged-in user's
    /// `~/.ssh/authorized_keys`, creating the directory and file (private to the user) if
    /// needed, unless the key is already there.
    ///
    /// # Errors
    ///
    /// [`SshError::KeyUnreadable`] if `public_line` isn't a public key,
    /// [`SshError::RemoteCommand`] if the server's shell couldn't add it, or a channel error.
    pub async fn install_public_key(&self, public_line: &str) -> Result<KeyInstall, SshError> {
        // Re-encoded, so the line sent is exactly one well-formed key, comment included.
        let key =
            PublicKey::from_openssh(public_line.trim()).map_err(|e| SshError::KeyUnreadable {
                reason: e.to_string(),
            })?;
        let line = key.to_openssh().map_err(|source| SshError::KeyEncoding {
            algorithm: key.algorithm().to_string(),
            source,
        })?;
        let mut channel = self.handle().channel_open_session().await?;
        channel.exec(true, format!("exec sh -c '{SCRIPT}'")).await?;
        let mut stdout = Vec::new();
        for data in expect_success(&mut channel, "command").await? {
            stdout.extend(data);
        }
        channel.data_bytes(format!("{line}\n").into_bytes()).await?;
        channel.eof().await?;
        let mut stderr = Vec::new();
        let mut status = None;
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        let stdout = String::from_utf8_lossy(&stdout);
        match (status, stdout.trim()) {
            (Some(0), "added") => {
                tracing::info!(fingerprint = %crate::known_hosts::fingerprint(&key), "added the key to authorized_keys");
                Ok(KeyInstall::Added)
            }
            (Some(0), "present") => Ok(KeyInstall::AlreadyPresent),
            _ => Err(SshError::RemoteCommand {
                what: "add the key to ~/.ssh/authorized_keys",
                status,
                output: String::from_utf8_lossy(&stderr).trim().to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_can_sit_inside_single_quotes() {
        assert!(!SCRIPT.contains('\''));
    }
}
