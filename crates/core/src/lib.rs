//! Platform-neutral core for photosync.
//!
//! The Android app, the developer CLI and the hub status server share this
//! crate. Nothing in here may depend on Android.

pub mod capacity;
pub mod ignore;
pub mod local;
pub mod profile;
pub mod project;
pub mod release;
pub mod settings;
pub mod status_client;
pub mod syncthing;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("invalid project: {0}")]
    InvalidProject(String),
    #[error("invalid profile: {0}")]
    InvalidProfile(String),
    #[error("Syncthing: {0}")]
    Syncthing(String),
    #[error("status server: {0}")]
    StatusServer(String),
    #[error("capacity: {0}")]
    Capacity(String),
    #[error("release: {0}")]
    Release(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Relative paths in this codebase always use `/`, never a leading slash and
/// never `.` or `..` components.
pub fn normalize_relative(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::InvalidPath("empty path".into()));
    }
    if value.starts_with('/') || value.contains('\\') || value.contains('\0') {
        return Err(Error::InvalidPath(format!(
            "absolute or invalid path: {value}"
        )));
    }
    let mut parts = Vec::new();
    for part in value.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(Error::InvalidPath(format!("invalid component in {value}")));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

/// Validate a single file/directory name inside the synced folder.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.chars().any(|c| c.is_control())
        || name.len() > 255
    {
        return Err(Error::InvalidProject(format!("invalid name: {name:?}")));
    }
    Ok(())
}
