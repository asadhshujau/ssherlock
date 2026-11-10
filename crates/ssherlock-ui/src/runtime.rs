//! Access to the app's single Tokio runtime from gpui code.
//!
//! `main` creates the runtime and stores its handle here as a gpui global, so views can hand
//! network and disk futures to Tokio (CLAUDE.md §6) without owning the runtime themselves.

use gpui::Global;
use tokio::runtime::Handle;

/// Handle to the Tokio runtime that runs all network, disk and pty I/O.
#[derive(Clone, Debug)]
pub struct TokioHandle(Handle);

impl TokioHandle {
    /// Wraps the handle of the runtime created in `main`.
    pub fn new(handle: Handle) -> Self {
        Self(handle)
    }

    /// The runtime handle, for `spawn`ing futures onto Tokio.
    pub fn handle(&self) -> &Handle {
        &self.0
    }
}

impl Global for TokioHandle {}
