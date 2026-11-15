//! SSH connections on screen: the **view** layer of the SSH feature.
//!
//! [`RemoteTerminal`] shows a remote shell with its host key and login dialogs and a status bar;
//! [`QuickConnectBar`] turns typed `user@host:port` into a connection. The pane hosts both.
//! Connecting, logging in and reconnecting all happen in `ssherlock-ssh`.

pub mod actions;
pub mod dialogs;
pub mod quick_connect;
pub mod remote_terminal;
pub mod status;

use std::path::PathBuf;

use gpui::{App, Global};
pub use quick_connect::{QuickConnectBar, QuickConnectEvent};
pub use remote_terminal::RemoteTerminal;

/// Where SSH finds and keeps things on this machine. Set by `main` from the platform
/// directories; tests point it at temporary ones. Without it, remote tabs explain they can't
/// connect.
#[derive(Clone, Debug)]
pub struct SshEnvironment {
    /// SSHerlock's own `known_hosts`, where newly trusted keys are saved.
    pub known_hosts: PathBuf,
    /// Other `known_hosts` files honoured but never written (e.g. `~/.ssh/known_hosts`).
    pub read_only_known_hosts: Vec<PathBuf>,
    /// `~/.ssh`, for the default key files; `None` if there's no home directory.
    pub ssh_dir: Option<PathBuf>,
    /// The local user name, used when the quick connect text has no `user@`.
    pub local_user: Option<String>,
}

impl Global for SshEnvironment {}

/// Sets up the SSH feature. Its one action is handled by each pane and bound in
/// `keymap/default.json`, so there's no app-wide state to install.
pub fn init(_cx: &mut App) {}
