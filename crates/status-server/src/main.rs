use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use clap::Parser;
use photosync_core::{
    ignore::IgnoreSet,
    local::safe_join,
    normalize_relative,
    status_client::{BackupStatus, BaseIgnores, HubFile, ProjectSize, SizesResponse, TreeResponse},
    Error as CoreError,
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tracing::{info, warn};
use walkdir::WalkDir;

#[derive(Debug, Parser)]
#[command(about = "Read-only status server for the photosync release gate")]
struct Args {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    token_file: PathBuf,
    /// Override the bind address from the config, mainly for local testing.
    #[arg(long)]
    bind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct Config {
    bind: String,
    root: PathBuf,
    ignore_file: PathBuf,
    backup_marker: Option<PathBuf>,
    sizes_cache_seconds: u64,
    max_entries: usize,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".into(),
            root: PathBuf::from("/srv/photos"),
            ignore_file: PathBuf::from(".stignore"),
            backup_marker: None,
            sizes_cache_seconds: 60,
            max_entries: 100_000,
            tls_cert: None,
            tls_key: None,
        }
    }
}

#[derive(Clone)]
struct AppState {
    root: Arc<PathBuf>,
    ignore_file: Arc<PathBuf>,
    backup_marker: Arc<Option<PathBuf>>,
    max_entries: usize,
    token: Arc<String>,
    sizes: SizesCache,
    sizes_ttl: Duration,
}

type SizesCache = Arc<Mutex<HashMap<String, (Instant, Vec<ProjectSize>)>>>;

impl AppState {
    fn ignore_lines(&self) -> Result<Vec<String>, ApiError> {
        load_ignore_lines(&self.root, &self.ignore_file).map_err(ApiError::internal)
    }

    fn ignore_set(&self) -> Result<IgnoreSet, ApiError> {
        let lines = self.ignore_lines()?;
        IgnoreSet::parse(lines.iter().map(String::as_str)).map_err(ApiError::internal)
    }

    fn prefix_path(&self, prefix: &str) -> Result<PathBuf, ApiError> {
        let prefix = prefix.trim().trim_matches('/');
        if prefix.is_empty() {
            return Ok(self.root.as_ref().clone());
        }
        let relative = normalize_relative(prefix).map_err(ApiError::bad_request)?;
        safe_join(&self.root, &relative).map_err(ApiError::bad_request)
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: error.to_string(),
        }
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
struct PrefixQuery {
    #[serde(default)]
    prefix: String,
}

async fn require_token(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let expected = state.token.as_str();
    let authorized = presented.len() == expected.len()
        && bool::from(presented.as_bytes().ct_eq(expected.as_bytes()));
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    next.run(request).await
}

async fn tree(
    State(state): State<AppState>,
    Query(query): Query<PrefixQuery>,
) -> Result<Json<TreeResponse>, ApiError> {
    let base = state.prefix_path(&query.prefix)?;
    let meta = match fs::symlink_metadata(&base) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Json(TreeResponse {
                entries: Vec::new(),
            }));
        }
        Err(error) => return Err(ApiError::internal(error)),
    };
    if !meta.is_dir() {
        return Err(ApiError::bad_request("prefix is not a directory"));
    }
    let ignores = state.ignore_set()?;
    let mut entries = Vec::new();
    for entry in WalkDir::new(&base).follow_links(false).sort_by_file_name() {
        let entry = entry.map_err(ApiError::internal)?;
        let path = entry.path();
        let relative = relative_to(&state.root, path)?;
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if ignores.is_ignored(&relative, true) {
                // WalkDir's iterator has no skip here; filtering children is
                // still correct because each child is checked below.
            }
            continue;
        }
        if !file_type.is_file() || ignores.is_ignored(&relative, false) {
            continue;
        }
        let metadata = entry.metadata().map_err(ApiError::internal)?;
        entries.push(HubFile {
            path: relative,
            size: metadata.len(),
            mtime_ns: mtime_ns(&metadata).map_err(ApiError::internal)?,
            ctime_ns: ctime_ns(&metadata).map_err(ApiError::internal)?,
        });
        if entries.len() > state.max_entries {
            return Err(ApiError {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                message: format!("more than {} files under the prefix", state.max_entries),
            });
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(TreeResponse { entries }))
}

async fn sizes(
    State(state): State<AppState>,
    Query(query): Query<PrefixQuery>,
) -> Result<Json<SizesResponse>, ApiError> {
    let prefix = query.prefix.trim().trim_matches('/').to_string();
    if let Some((at, cached)) = state.sizes.lock().unwrap().get(&prefix) {
        if at.elapsed() < state.sizes_ttl {
            return Ok(Json(SizesResponse {
                projects: cached.clone(),
            }));
        }
    }
    let base = state.prefix_path(&prefix)?;
    let meta = match fs::symlink_metadata(&base) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Json(SizesResponse {
                projects: Vec::new(),
            }));
        }
        Err(error) => return Err(ApiError::internal(error)),
    };
    if !meta.is_dir() {
        return Err(ApiError::bad_request("prefix is not a directory"));
    }
    let ignores = state.ignore_set()?;
    let mut projects = Vec::new();
    for entry in fs::read_dir(&base).map_err(ApiError::internal)? {
        let entry = entry.map_err(ApiError::internal)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(ApiError::internal)?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let relative = relative_to(&state.root, &path)?;
        if ignores.is_ignored(&relative, true) {
            continue;
        }
        let mut size = 0u64;
        let mut files = 0u64;
        let mut newest = 0i64;
        for file in WalkDir::new(&path).follow_links(false).sort_by_file_name() {
            let file = file.map_err(ApiError::internal)?;
            let relative_file = relative_to(&state.root, file.path())?;
            let file_type = file.file_type();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                continue;
            }
            if !file_type.is_file() || ignores.is_ignored(&relative_file, false) {
                continue;
            }
            let metadata = file.metadata().map_err(ApiError::internal)?;
            size = size.saturating_add(metadata.len());
            files += 1;
            newest = newest.max(mtime_ns(&metadata).map_err(ApiError::internal)?);
        }
        projects.push(ProjectSize {
            path: relative,
            size,
            files,
            mtime_ns: newest,
        });
    }
    projects.sort_by(|a, b| a.path.cmp(&b.path));
    state
        .sizes
        .lock()
        .unwrap()
        .insert(prefix, (Instant::now(), projects.clone()));
    Ok(Json(SizesResponse { projects }))
}

async fn base_ignores(State(state): State<AppState>) -> Result<Json<BaseIgnores>, ApiError> {
    Ok(Json(BaseIgnores {
        lines: state.ignore_lines()?,
    }))
}

async fn backup(State(state): State<AppState>) -> Result<Json<BackupStatus>, ApiError> {
    let Some(marker) = state.backup_marker.as_ref() else {
        return Ok(Json(BackupStatus::default()));
    };
    let meta = match fs::symlink_metadata(marker) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Json(BackupStatus::default()));
        }
        Err(error) => return Err(ApiError::internal(error)),
    };
    let marker_mtime_ns = mtime_ns(&meta).map_err(ApiError::internal)?;
    let text = fs::read_to_string(marker).map_err(ApiError::internal)?;
    let text = text.trim();
    if text.is_empty() {
        return Ok(Json(BackupStatus {
            last_success: None,
            marker_mtime_ns: Some(marker_mtime_ns),
        }));
    }
    let last_success = parse_marker_time(text).map_err(ApiError::internal)?;
    Ok(Json(BackupStatus {
        last_success: Some(last_success),
        marker_mtime_ns: Some(marker_mtime_ns),
    }))
}

fn parse_marker_time(text: &str) -> Result<DateTime<Utc>, CoreError> {
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Ok(time.with_timezone(&Utc));
    }
    if let Ok(seconds) = text.parse::<i64>() {
        return DateTime::from_timestamp(seconds, 0)
            .ok_or_else(|| CoreError::StatusServer("backup timestamp out of range".into()));
    }
    Err(CoreError::StatusServer(format!(
        "backup marker is not RFC3339 or epoch seconds: {text}"
    )))
}

fn relative_to(root: &Path, path: &Path) -> Result<String, ApiError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| ApiError::internal("path escaped the root"))?
        .to_string_lossy()
        .replace('\\', "/");
    if relative.is_empty() {
        return Ok(String::new());
    }
    normalize_relative(&relative).map_err(ApiError::internal)
}

/// Read and expand the hub's `.stignore`. Includes are resolved recursively
/// so the device profile inherits real patterns, never a hub-specific include
/// line.
fn load_ignore_lines(root: &Path, ignore_file: &Path) -> Result<Vec<String>, CoreError> {
    let path = root.join(ignore_file);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    load_include(root, &path, &mut seen, &mut out, 0)?;
    Ok(out)
}

fn load_include(
    root: &Path,
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<String>,
    depth: usize,
) -> Result<(), CoreError> {
    if depth > 16 {
        return Err(CoreError::StatusServer(
            "ignore includes nested too deeply".into(),
        ));
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(canonical.clone()) {
        return Err(CoreError::StatusServer(format!(
            "ignore include loop at {}",
            path.display()
        )));
    }
    if !path.exists() {
        seen.remove(&canonical);
        return Ok(());
    }
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') && !line.starts_with("#include ") {
            continue;
        }
        if let Some(include) = line.strip_prefix("#include ") {
            let include = include.trim();
            if include.is_empty() {
                return Err(CoreError::StatusServer("empty #include".into()));
            }
            let relative = normalize_relative(include.trim_start_matches('/'))?;
            let include_path = root.join(relative);
            if !include_path.starts_with(root) {
                return Err(CoreError::StatusServer(format!(
                    "include escapes the folder: {include}"
                )));
            }
            load_include(root, &include_path, seen, out, depth + 1)?;
        } else {
            out.push(line.to_string());
        }
    }
    seen.remove(&canonical);
    Ok(())
}

#[cfg(unix)]
fn mtime_ns(meta: &fs::Metadata) -> Result<i64, CoreError> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.mtime())
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|s| s.checked_add(meta.mtime_nsec()))
        .ok_or_else(|| CoreError::StatusServer("mtime out of range".into()))
}

#[cfg(unix)]
fn ctime_ns(meta: &fs::Metadata) -> Result<i64, CoreError> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.ctime())
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|s| s.checked_add(meta.ctime_nsec()))
        .ok_or_else(|| CoreError::StatusServer("ctime out of range".into()))
}

#[cfg(not(unix))]
compile_error!("photosync-status reads mtime/ctime through Unix metadata and targets Linux");

fn app(state: AppState) -> Router {
    Router::new()
        .route("/v1/tree", get(tree))
        .route("/v1/sizes", get(sizes))
        .route("/v1/base-ignores", get(base_ignores))
        .route("/v1/backup", get(backup))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "photosync_status=info".into()),
        )
        .init();
    let args = Args::parse();
    let config_text = fs::read_to_string(&args.config)
        .with_context(|| format!("reading {}", args.config.display()))?;
    let mut config: Config = toml::from_str(&config_text)
        .with_context(|| format!("parsing {}", args.config.display()))?;
    if let Some(bind) = args.bind {
        config.bind = bind;
    }
    let token = fs::read_to_string(&args.token_file)
        .with_context(|| format!("reading {}", args.token_file.display()))?;
    let token = token.trim().to_string();
    if token.len() < 16 {
        anyhow::bail!("the token file must contain at least 16 bytes");
    }
    if !config.root.is_dir() {
        anyhow::bail!("root {} is not a directory", config.root.display());
    }
    let address: SocketAddr = config
        .bind
        .parse()
        .with_context(|| format!("bind address {}", config.bind))?;
    let state = AppState {
        root: Arc::new(config.root.clone()),
        ignore_file: Arc::new(config.ignore_file.clone()),
        backup_marker: Arc::new(config.backup_marker.clone()),
        max_entries: config.max_entries,
        token: Arc::new(token),
        sizes: Arc::new(Mutex::new(HashMap::new())),
        sizes_ttl: Duration::from_secs(config.sizes_cache_seconds),
    };
    let router = app(state);
    info!(%address, root = %config.root.display(), "photosync-status listening");
    match (config.tls_cert, config.tls_key) {
        (Some(cert), Some(key)) => {
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
                .await
                .context("loading TLS certificate")?;
            axum_server::bind_rustls(address, tls)
                .serve(router.into_make_service())
                .await?;
        }
        (None, None) => {
            warn!("TLS is not configured; bind this server to loopback or a trusted WireGuard interface");
            axum_server::bind(address)
                .serve(router.into_make_service())
                .await?;
        }
        _ => anyhow::bail!("tls_cert and tls_key must be set together"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::AUTHORIZATION;
    use tower::ServiceExt;

    const TOKEN: &str = "test-token-with-enough-bytes";

    fn state(root: &Path) -> AppState {
        AppState {
            root: Arc::new(root.to_path_buf()),
            ignore_file: Arc::new(PathBuf::from(".stignore")),
            backup_marker: Arc::new(Some(root.join("last-backup"))),
            max_entries: 100,
            token: Arc::new(TOKEN.into()),
            sizes: Arc::new(Mutex::new(HashMap::new())),
            sizes_ttl: Duration::from_secs(60),
        }
    }

    async fn request(state: AppState, path: &str) -> (StatusCode, serde_json::Value) {
        let response = app(state)
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("originals/Years/2024/A")).unwrap();
        fs::write(tmp.path().join("originals/Years/2024/A/a.jpg"), b"abc").unwrap();
        fs::write(tmp.path().join("originals/Years/2024/A/.DS_Store"), b"x").unwrap();
        fs::create_dir_all(tmp.path().join("originals/Years/2024/B")).unwrap();
        fs::write(tmp.path().join("originals/Years/2024/B/b.jpg"), b"12345").unwrap();
        fs::write(
            tmp.path().join(".stignore"),
            "# hub ignores\n(?d).DS_Store\n#include more-ignores\n",
        )
        .unwrap();
        fs::write(tmp.path().join("more-ignores"), "*.tmp\n").unwrap();
        fs::write(tmp.path().join("last-backup"), "2024-05-01T12:00:00Z\n").unwrap();
        tmp
    }

    #[tokio::test]
    async fn requires_a_bearer_token() {
        let tmp = fixture();
        let response = app(state(tmp.path()))
            .oneshot(
                Request::builder()
                    .uri("/v1/tree")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn tree_reads_disk_and_skips_ignored_files() {
        let tmp = fixture();
        let (status, body) =
            request(state(tmp.path()), "/v1/tree?prefix=originals/Years/2024/A").await;
        assert_eq!(status, StatusCode::OK);
        let entries = body["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["path"], "originals/Years/2024/A/a.jpg");
        assert_eq!(entries[0]["size"], 3);
        assert!(entries[0]["ctime_ns"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn sizes_cache_and_report_immediate_children() {
        let tmp = fixture();
        let (status, body) =
            request(state(tmp.path()), "/v1/sizes?prefix=originals/Years/2024").await;
        assert_eq!(status, StatusCode::OK);
        let projects = body["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 2);
        let a = projects
            .iter()
            .find(|p| p["path"] == "originals/Years/2024/A")
            .unwrap();
        assert_eq!(a["size"], 3);
        assert_eq!(a["files"], 1);
    }

    #[tokio::test]
    async fn base_ignores_expands_includes() {
        let tmp = fixture();
        let (status, body) = request(state(tmp.path()), "/v1/base-ignores").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["lines"], serde_json::json!(["(?d).DS_Store", "*.tmp"]));
    }

    #[tokio::test]
    async fn backup_accepts_rfc3339_and_reports_missing() {
        let tmp = fixture();
        let (status, body) = request(state(tmp.path()), "/v1/backup").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["last_success"], "2024-05-01T12:00:00Z");
        assert!(body["marker_mtime_ns"].as_i64().unwrap() > 0);
        fs::remove_file(tmp.path().join("last-backup")).unwrap();
        let (_, body) = request(state(tmp.path()), "/v1/backup").await;
        assert!(body["last_success"].is_null());
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let tmp = fixture();
        let (status, _) = request(state(tmp.path()), "/v1/tree?prefix=../etc").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
