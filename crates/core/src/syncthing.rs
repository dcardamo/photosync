use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FolderStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub state_changed: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub pull_errors: u64,
    #[serde(default)]
    pub need_files: u64,
    #[serde(default)]
    pub need_bytes: u64,
    #[serde(default)]
    pub global_files: u64,
    #[serde(default)]
    pub global_bytes: u64,
    #[serde(default)]
    pub local_files: u64,
    #[serde(default)]
    pub local_bytes: u64,
}

impl FolderStatus {
    pub fn is_idle(&self) -> bool {
        self.state == "idle" && self.error.is_empty() && self.errors == 0 && self.pull_errors == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Ignores {
    #[serde(default, rename = "ignore")]
    pub ignore: Vec<String>,
    #[serde(default, rename = "expanded")]
    pub expanded: Vec<String>,
}

impl Ignores {
    pub fn contains_line(&self, line: &str) -> bool {
        self.ignore
            .iter()
            .chain(self.expanded.iter())
            .any(|value| value.trim() == line.trim())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseEntry {
    pub name: String,
    #[serde(default)]
    pub mod_time: String,
    #[serde(default)]
    pub size: u64,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub children: Vec<BrowseEntry>,
}

impl BrowseEntry {
    pub fn flatten<'a>(&'a self, prefix: &str, out: &mut Vec<(String, &'a BrowseEntry)>) {
        let path = if prefix.is_empty() {
            self.name.clone()
        } else {
            format!("{prefix}/{}", self.name)
        };
        out.push((path.clone(), self));
        for child in &self.children {
            child.flatten(&path, out);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    #[serde(rename = "myID")]
    pub my_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Connections {
    #[serde(default)]
    pub connections: HashMap<String, ConnectionInfo>,
}

#[derive(Clone)]
pub struct SyncthingClient {
    base: Url,
    api_key: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for SyncthingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncthingClient")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl SyncthingClient {
    pub fn new(base: &str, api_key: &str) -> Result<Self> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(Error::Syncthing("API key is required".into()));
        }
        let mut base = Url::parse(base.trim())?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(Error::Syncthing(
                "Syncthing URL must be http or https".into(),
            ));
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            base,
            api_key: api_key.to_string(),
            http,
        })
    }

    fn url(&self, path: &str) -> Result<Url> {
        Ok(self.base.join(path.trim_start_matches('/'))?)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.url(path)?;
        let response = self
            .http
            .get(url)
            .header("X-API-Key", &self.api_key)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::Syncthing(format!(
                "GET {path} returned {status}: {}",
                body.trim()
            )));
        }
        serde_json::from_str(&body).map_err(Error::from)
    }

    async fn post(&self, path: &str) -> Result<()> {
        let url = self.url(path)?;
        let response = self
            .http
            .post(url)
            .header("X-API-Key", &self.api_key)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Syncthing(format!(
                "POST {path} returned {status}: {}",
                body.trim()
            )));
        }
        Ok(())
    }

    async fn put_json(&self, path: &str, value: &serde_json::Value) -> Result<()> {
        let url = self.url(path)?;
        let response = self
            .http
            .put(url)
            .header("X-API-Key", &self.api_key)
            .json(value)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Syncthing(format!(
                "PUT {path} returned {status}: {}",
                body.trim()
            )));
        }
        Ok(())
    }

    pub async fn folder_status(&self, folder: &str) -> Result<FolderStatus> {
        self.get(&format!("rest/db/status?folder={}", encode(folder)))
            .await
    }

    /// Trigger a scan of a folder or a sub-path.
    pub async fn scan(&self, folder: &str, sub: Option<&str>) -> Result<()> {
        let mut path = format!("rest/db/scan?folder={}", encode(folder));
        if let Some(sub) = sub {
            if !sub.trim().is_empty() {
                path.push_str("&sub=");
                path.push_str(&encode(sub.trim_matches('/')));
            }
        }
        self.post(&path).await
    }

    pub async fn ignores(&self, folder: &str) -> Result<Ignores> {
        self.get(&format!("rest/db/ignores?folder={}", encode(folder)))
            .await
    }

    pub async fn browse(&self, folder: &str, prefix: &str) -> Result<Vec<BrowseEntry>> {
        let mut path = format!("rest/db/browse?folder={}", encode(folder));
        if !prefix.trim().is_empty() {
            path.push_str("&prefix=");
            path.push_str(&encode(prefix.trim_matches('/')));
        }
        self.get(&path).await
    }

    pub async fn system_status(&self) -> Result<SystemStatus> {
        self.get("rest/system/status").await
    }

    pub async fn connections(&self) -> Result<Connections> {
        self.get("rest/system/connections").await
    }

    pub async fn config(&self) -> Result<serde_json::Value> {
        self.get("rest/config").await
    }

    pub async fn set_config(&self, config: &serde_json::Value) -> Result<()> {
        self.put_json("rest/config", config).await
    }

    pub async fn pause_folder(&self, folder: &str, paused: bool) -> Result<()> {
        let action = if paused { "pause" } else { "resume" };
        self.post(&format!("rest/system/{action}?folder={}", encode(folder)))
            .await
    }

    pub async fn db_file(&self, folder: &str, file: &str) -> Result<serde_json::Value> {
        self.get(&format!(
            "rest/db/file?folder={}&file={}",
            encode(folder),
            encode(file)
        ))
        .await
    }

    pub async fn completion(&self, folder: &str, device: &str) -> Result<serde_json::Value> {
        self.get(&format!(
            "rest/db/completion?folder={}&device={}",
            encode(folder),
            encode(device)
        ))
        .await
    }

    pub async fn remoteneed(&self, folder: &str, device: &str) -> Result<serde_json::Value> {
        self.get(&format!(
            "rest/db/remoteneed?folder={}&device={}",
            encode(folder),
            encode(device)
        ))
        .await
    }

    pub async fn scan_and_wait(
        &self,
        folder: &str,
        sub: Option<&str>,
        timeout: Duration,
        poll: Duration,
    ) -> Result<FolderStatus> {
        self.scan(folder, sub).await?;
        // POST /rest/db/scan is asynchronous. Without a short grace period the
        // first status poll can still say "idle" before the scan starts.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let status = self.folder_status(folder).await?;
            if status.state == "error" || !status.error.is_empty() || status.errors > 0 {
                return Err(Error::Syncthing(format!(
                    "folder {folder} is in an error state: {}",
                    if status.error.is_empty() {
                        format!("{} errors", status.errors)
                    } else {
                        status.error.clone()
                    }
                )));
            }
            if status.is_idle() {
                return Ok(status);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "folder {folder} did not become idle after {}s (state {})",
                    timeout.as_secs(),
                    status.state
                )));
            }
            tokio::time::sleep(poll).await;
        }
    }
}

pub fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_requires_no_errors() {
        let mut status = FolderStatus {
            state: "idle".into(),
            ..Default::default()
        };
        assert!(status.is_idle());
        status.pull_errors = 1;
        assert!(!status.is_idle());
        status.pull_errors = 0;
        status.error = "boom".into();
        assert!(!status.is_idle());
    }

    #[test]
    fn ignores_checks_both_views() {
        let ignores = Ignores {
            ignore: vec!["#include .stprofiles/pixel.txt".into()],
            expanded: vec!["!/originals/Projects/A".into()],
        };
        assert!(ignores.contains_line("!/originals/Projects/A"));
        assert!(!ignores.contains_line("!/originals/Projects/B"));
    }

    #[test]
    fn url_encoding_keeps_spaces_out_of_the_query() {
        assert_eq!(encode("a b/c"), "a+b%2Fc");
    }
}
