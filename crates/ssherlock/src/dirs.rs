//! Where SSHerlock keeps its files on each platform.
//!
//! Resolved once at startup and handed to whatever needs a path (logging, the database, SSH),
//! so no crate looks up directories on its own.

use std::path::PathBuf;

use directories::ProjectDirs;
use ssherlock_core::app;

/// The platform directories SSHerlock writes to.
///
/// On Linux these follow XDG (`~/.local/share/ssherlock`), on macOS
/// `~/Library/Application Support/io.github.asadhshujau.SSHerlock`.
#[derive(Debug, Clone)]
pub struct AppDirs {
    data_local: PathBuf,
}

impl AppDirs {
    /// Looks up the directories for the current user.
    ///
    /// Returns `None` when the platform reports no home directory, which happens only in
    /// unusual sandboxes; callers then run without files rather than failing to start.
    pub fn resolve() -> Option<Self> {
        ProjectDirs::from(app::QUALIFIER, app::ORGANISATION, app::NAME).map(|dirs| Self {
            data_local: dirs.data_local_dir().to_path_buf(),
        })
    }

    /// Directory for rolling log files.
    pub fn logs(&self) -> PathBuf {
        self.data_local.join("logs")
    }

    /// The database of saved hosts and everything else SSHerlock keeps.
    pub fn database(&self) -> PathBuf {
        self.data_local.join("ssherlock.db")
    }

    /// SSHerlock's own `known_hosts`, where host keys the user trusts are saved.
    pub fn known_hosts(&self) -> PathBuf {
        self.data_local.join("known_hosts")
    }
}

/// The user's `~/.ssh`, for OpenSSH's `known_hosts` and default keys, if there's a home directory.
pub fn ssh_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".ssh"))
}

/// The local user name, for connections typed without `user@`.
pub fn local_user() -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|name| !name.is_empty()))
}
