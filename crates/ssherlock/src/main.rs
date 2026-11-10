//! SSHerlock, a desktop SSH client.
//!
//! The binary only wires crates together: it resolves the app's directories, sets up logging,
//! starts the Tokio runtime and opens the main window. All behaviour lives in the library crates.
#![forbid(unsafe_code)]

mod dirs;
mod logging;

use anyhow::Context as _;

fn main() -> anyhow::Result<()> {
    let dirs = dirs::AppDirs::resolve();
    // Held until `main` returns so buffered log lines are flushed on exit.
    let _log_guard = logging::init(dirs.as_ref());

    // The one Tokio runtime for network, disk and pty I/O (CLAUDE.md §6). The next Phase 0 step
    // hands its handle to gpui as the `TokioHandle` global.
    let _runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ssherlock-tokio")
        .build()
        .context("could not start the Tokio runtime")?;

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "SSHerlock started");
    Ok(())
}
