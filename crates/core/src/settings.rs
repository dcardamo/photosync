use crate::{
    capacity::DEFAULT_HEADROOM_PERCENT, profile::write_atomic, project::ProjectLayout, Error,
    Result,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Syncthing-Fork's local REST API.
    pub syncthing_url: String,
    /// Syncthing folder ID, not its display label.
    pub folder_id: String,
    /// Name used for `.stprofiles/<device>.txt`.
    pub device_name: String,
    /// Absolute path to the shared folder's root on the device.
    pub folder_root: String,
    /// HTTPS base URL of the hub's read-only status server.
    pub status_server_url: String,
    /// Refuse activation below this percentage of total pool capacity.
    pub headroom_percent: u8,
    /// Paths that are always synced, such as `inbox/PixelMob`.
    pub always_on: Vec<String>,
    pub layout: ProjectLayout,
    /// Hub base ignores last fetched from the status server. Activation can
    /// use this while offline; it is refreshed whenever the hub is reachable.
    pub cached_base_ignores: Vec<String>,
    /// Kept on the device, outside the synced folder.
    pub journal_dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            syncthing_url: "http://127.0.0.1:8384".into(),
            folder_id: "photos".into(),
            device_name: String::new(),
            folder_root: String::new(),
            status_server_url: String::new(),
            headroom_percent: DEFAULT_HEADROOM_PERCENT,
            always_on: Vec::new(),
            layout: ProjectLayout::default(),
            cached_base_ignores: Vec::new(),
            journal_dir: String::new(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if self.syncthing_url.trim().is_empty() {
            return Err(Error::InvalidPath("syncthing_url is required".into()));
        }
        let url = url::Url::parse(self.syncthing_url.trim())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::InvalidPath(
                "syncthing_url must be http or https".into(),
            ));
        }
        if self.folder_id.trim().is_empty() {
            return Err(Error::InvalidPath("folder_id is required".into()));
        }
        crate::profile::validate_device(&self.device_name)?;
        if self.folder_root.trim().is_empty() {
            return Err(Error::InvalidPath("folder_root is required".into()));
        }
        if self.headroom_percent > 100 {
            return Err(Error::Capacity("headroom must be 0-100 percent".into()));
        }
        self.layout.validate()?;
        for path in &self.always_on {
            crate::normalize_relative(path)?;
        }
        if !self.status_server_url.trim().is_empty() {
            crate::status_client::validate_url(self.status_server_url.trim())?;
        }
        Ok(())
    }

    pub fn root(&self) -> PathBuf {
        PathBuf::from(self.folder_root.trim())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        let settings: Self = serde_json::from_str(&text)?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        write_atomic(path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable_and_empty_until_configured() {
        let settings = Settings::default();
        assert_eq!(settings.syncthing_url, "http://127.0.0.1:8384");
        assert_eq!(settings.headroom_percent, 10);
        assert_eq!(settings.layout, ProjectLayout::default());
        assert!(settings.validate().is_err());
    }

    #[test]
    fn round_trips_and_rejects_bad_values() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let mut settings = Settings {
            folder_id: "photos".into(),
            device_name: "pixel".into(),
            folder_root: "/srv/photos".into(),
            status_server_url: "https://hub.example.ts.net".into(),
            always_on: vec!["inbox/PixelMob".into()],
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        settings.headroom_percent = 101;
        assert!(settings.validate().is_err());
        settings.headroom_percent = 10;
        settings.device_name = "../x".into();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn status_url_allows_cleartext_only_on_loopback() {
        assert!(crate::status_client::validate_url("https://hub.example.ts.net").is_ok());
        assert!(crate::status_client::validate_url("http://127.0.0.1:8787").is_ok());
        assert!(crate::status_client::validate_url("http://localhost:8787").is_ok());
        assert!(crate::status_client::validate_url("http://hub.example.ts.net").is_err());
    }
}
