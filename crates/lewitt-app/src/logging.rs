//! Process-wide local tracing setup.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

const LOG_DIRECTORY: &str = "lewitt-ctl";
const LOG_FILE: &str = "lewitt-control-panel.log";
const DEFAULT_FILTER: &str = concat!(
    "warn,",
    "lewitt_app=info,",
    "lewitt_core=info,",
    "lewitt_backend_windows=info,",
    "lewitt_backend_linux=info"
);

/// Installs the global tracing subscriber.
///
/// Events are written to both stderr and a per-user state directory. If the
/// file cannot be created, stderr logging remains available and application
/// startup is allowed to continue.
#[must_use]
pub fn init() -> Option<PathBuf> {
    let filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    match open_log_file() {
        Ok((path, file)) => {
            let file_layer = fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_file(true)
                .with_line_number(true)
                .with_writer(Mutex::new(file));
            let console_layer = fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_file(true)
                .with_line_number(true)
                .with_writer(io::stderr);
            let installed = tracing_subscriber::registry()
                .with(filter())
                .with(file_layer)
                .with(console_layer)
                .try_init();
            if installed.is_ok() {
                return Some(path);
            }
            eprintln!("failed to install tracing subscriber: {installed:?}");
        }
        Err(error) => {
            eprintln!("failed to open local log file: {error}");
        }
    }

    let installed = tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_file(true)
        .with_line_number(true)
        .with_writer(io::stderr)
        .try_init();
    if let Err(error) = installed {
        eprintln!("failed to install stderr tracing subscriber: {error}");
    }
    None
}

fn open_log_file() -> io::Result<(PathBuf, fs::File)> {
    let directory = log_directory()?;
    fs::create_dir_all(&directory)?;
    let path = directory.join(LOG_FILE);
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok((path, file))
}

fn log_directory() -> io::Result<PathBuf> {
    platform_state_directory().map(|directory| directory.join(LOG_DIRECTORY).join("logs"))
}

#[cfg(target_os = "windows")]
fn platform_state_directory() -> io::Result<PathBuf> {
    environment_path("LOCALAPPDATA")
}

#[cfg(target_os = "linux")]
fn platform_state_directory() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path));
    }
    environment_path("HOME").map(|home| home.join(".local").join("state"))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn platform_state_directory() -> io::Result<PathBuf> {
    std::env::current_dir()
}

fn environment_path(name: &str) -> io::Result<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("environment variable {name} is unavailable"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_uses_a_fixed_non_identifying_filename() {
        let path = PathBuf::from("state")
            .join(LOG_DIRECTORY)
            .join("logs")
            .join(LOG_FILE);
        assert_eq!(path.file_name(), Some(std::ffi::OsStr::new(LOG_FILE)));
        assert!(!path.to_string_lossy().contains("serial"));
    }
}
