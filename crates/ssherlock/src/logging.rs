//! Logging: a rolling daily file in the app's data directory plus human-readable stderr.
//!
//! The filter comes from `RUST_LOG` (e.g. `RUST_LOG=ssherlock_ssh=debug`) and defaults to
//! `info`. Logging never stops the app from starting: if the log file can't be opened, or
//! `RUST_LOG` is invalid, it falls back and says so in the log.

use std::path::Path;

use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{InitError, RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::dirs::AppDirs;

/// Daily log files kept before the oldest is deleted: one week of history.
const KEPT_LOG_FILES: usize = 7;

/// Keeps the background log writer alive; dropping it flushes buffered lines to disk.
#[must_use = "dropping the guard stops writing the log file"]
pub struct LogGuard {
    _file: Option<WorkerGuard>,
}

/// Installs the global `tracing` subscriber. Call once, first thing in `main`.
pub fn init(dirs: Option<&AppDirs>) -> LogGuard {
    let (filter, filter_error) = match EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()
    {
        Ok(filter) => (filter, None),
        Err(error) => (EnvFilter::new(LevelFilter::INFO.to_string()), Some(error)),
    };

    let log_dir = dirs.map(AppDirs::logs);
    let file = log_dir.as_deref().map(file_appender);
    let (file_layer, file_guard, file_error) = match file {
        Some(Ok(appender)) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer);
            (Some(layer), Some(guard), None)
        }
        Some(Err(error)) => (None, None, Some(error)),
        None => (None, None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .init();

    // Reported only now, once there is a subscriber to report to.
    if let Some(error) = filter_error {
        tracing::warn!(%error, "ignoring invalid RUST_LOG, logging at info");
    }
    match (&log_dir, file_error) {
        (Some(dir), Some(error)) => tracing::warn!(
            dir = %dir.display(),
            %error,
            "could not open the log file, logging to stderr only"
        ),
        (Some(dir), None) => tracing::info!(dir = %dir.display(), "writing logs"),
        (None, _) => tracing::warn!("no home directory found, logging to stderr only"),
    }

    LogGuard { _file: file_guard }
}

/// A file appender that starts a new `ssherlock.YYYY-MM-DD.log` each day in `dir`, creating
/// `dir` if needed.
fn file_appender(dir: &Path) -> Result<RollingFileAppender, FileError> {
    // Created up front: the appender prunes old files before creating the directory, and
    // prints an error of its own to stderr when it's missing.
    std::fs::create_dir_all(dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("ssherlock")
        .filename_suffix("log")
        .max_log_files(KEPT_LOG_FILES)
        .build(dir)?;
    Ok(appender)
}

/// Why the log file could not be opened.
#[derive(Debug, thiserror::Error)]
enum FileError {
    #[error("could not create the log directory")]
    CreateDir(#[from] std::io::Error),
    #[error("could not open the log file")]
    Open(#[from] InitError),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::io::Write as _;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};

    use super::*;

    #[test]
    fn file_appender_creates_the_directory_and_a_dated_log_file() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            env::temp_dir().join(format!("ssherlock-log-test-{}-{nanos}", std::process::id()));
        let dir = root.join("nested").join("logs");

        let mut appender = file_appender(&dir).unwrap();
        appender.write_all(b"hello\n").unwrap();
        appender.flush().unwrap();

        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(names.len(), 1, "expected one log file, got {names:?}");
        let name = names.first().unwrap();
        assert!(
            name.starts_with("ssherlock.") && Path::new(name).extension() == Some("log".as_ref()),
            "unexpected log file name {name}"
        );
    }
}
