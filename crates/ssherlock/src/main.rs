//! SSHerlock, a desktop SSH client.
//!
//! The binary only wires crates together: it resolves the app's directories, sets up logging,
//! starts the Tokio runtime, tells SSH where its files are and opens the main window. All behaviour lives in the library crates.
#![forbid(unsafe_code)]

mod dirs;
mod logging;

use anyhow::Context as _;

fn main() -> anyhow::Result<()> {
    let dirs = dirs::AppDirs::resolve();
    // Held until `main` returns so buffered log lines are flushed on exit.
    let _log_guard = logging::init(dirs.as_ref());

    // The one Tokio runtime for network, disk and pty I/O (CLAUDE.md §6). It must outlive the
    // gpui event loop, which only ever sees its handle.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ssherlock-tokio")
        .build()
        .context("could not start the Tokio runtime")?;
    let tokio = runtime.handle().clone();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "SSHerlock started");

    let ssh = dirs.as_ref().map(|dirs| {
        let ssh_dir = dirs::ssh_dir();
        ssherlock_ui::ssh::SshEnvironment {
            known_hosts: dirs.known_hosts(),
            read_only_known_hosts: ssh_dir.iter().map(|dir| dir.join("known_hosts")).collect(),
            ssh_dir,
            local_user: dirs::local_user(),
        }
    });

    gpui::Application::new().run(move |cx| {
        ssherlock_ui::init(tokio, cx);
        if let Some(ssh) = ssh {
            cx.set_global(ssh);
        } else {
            tracing::warn!("no home directory, so SSH connections are unavailable");
        }
        if let Err(error) = ssherlock_ui::open_main_window(cx) {
            tracing::error!(
                error = format!("{error:#}"),
                "could not open the main window"
            );
            cx.quit();
        }
    });

    tracing::info!("SSHerlock exited");
    Ok(())
}
