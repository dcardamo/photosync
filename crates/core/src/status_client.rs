use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubFile {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSize {
    pub path: String,
    pub size: u64,
    pub files: u64,
    #[serde(default)]
    pub mtime_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeResponse {
    pub entries: Vec<HubFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizesResponse {
    pub projects: Vec<ProjectSize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseIgnores {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BackupStatus {
    pub last_success: Option<DateTime<Utc>>,
    pub marker_mtime_ns: Option<i64>,
}

#[derive(Clone)]
pub struct StatusClient {
    base: Url,
    token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for StatusClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusClient")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

pub fn validate_url(value: &str) -> Result<()> {
    let url = Url::parse(value)?;
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = url.host_str().unwrap_or_default();
            if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
                Ok(())
            } else {
                Err(Error::StatusServer(
                    "plain HTTP is allowed only on loopback; use HTTPS over the tailnet".into(),
                ))
            }
        }
        _ => Err(Error::StatusServer(
            "status server URL must be http or https".into(),
        )),
    }
}

impl StatusClient {
    pub fn new(base: &str, token: &str) -> Result<Self> {
        let base = base.trim();
        validate_url(base)?;
        let token = token.trim();
        if token.is_empty() {
            return Err(Error::StatusServer(
                "status server token is required".into(),
            ));
        }
        let mut base = Url::parse(base)?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            base,
            token: token.to_string(),
            http,
        })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.base.join(path.trim_start_matches('/'))?;
        let response = self.http.get(url).bearer_auth(&self.token).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::StatusServer(format!(
                "GET {path} returned {status}: {}",
                body.trim()
            )));
        }
        serde_json::from_str(&body).map_err(Error::from)
    }

    pub async fn tree(&self, prefix: &str) -> Result<Vec<HubFile>> {
        let response: TreeResponse = self
            .get(&format!(
                "v1/tree?prefix={}",
                super::syncthing::encode(prefix.trim_matches('/'))
            ))
            .await?;
        Ok(response.entries)
    }

    pub async fn sizes(&self, prefix: &str) -> Result<Vec<ProjectSize>> {
        let response: SizesResponse = self
            .get(&format!(
                "v1/sizes?prefix={}",
                super::syncthing::encode(prefix.trim_matches('/'))
            ))
            .await?;
        Ok(response.projects)
    }

    pub async fn base_ignores(&self) -> Result<Vec<String>> {
        let response: BaseIgnores = self.get("v1/base-ignores").await?;
        Ok(response.lines)
    }

    pub async fn backup(&self) -> Result<BackupStatus> {
        self.get("v1/backup").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cleartext_off_loopback() {
        assert!(validate_url("https://hub.example.ts.net").is_ok());
        assert!(validate_url("http://localhost:8787").is_ok());
        assert!(validate_url("http://127.0.0.1:8787").is_ok());
        assert!(validate_url("http://[::1]:8787").is_ok());
        assert!(validate_url("http://hub.example.ts.net").is_err());
        assert!(validate_url("ftp://hub.example.ts.net").is_err());
    }

    #[test]
    fn parses_the_wire_shapes() {
        let tree: TreeResponse = serde_json::from_str(
            r#"{"entries":[{"path":"originals/Projects/A/a.jpg","size":3,"mtime_ns":10,"ctime_ns":11}]}"#,
        )
        .unwrap();
        assert_eq!(tree.entries[0].size, 3);
        let backup: BackupStatus = serde_json::from_str(
            r#"{"last_success":"2024-05-01T00:00:00Z","marker_mtime_ns":123}"#,
        )
        .unwrap();
        assert!(backup.last_success.is_some());
    }
}
